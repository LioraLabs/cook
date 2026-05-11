# Runtime: Two Lua VMs, Two Phases

## Overview

There is no single "runtime" any more. The Lua VM layer is split across two crates with very different roles, plus a third crate that supplies the API surfaces both VMs share:

| Crate | Phase | What it does |
|---|---|---|
| `cli/crates/cook-register` | **Register (capture)** | One short-lived `mlua::Lua` per recipe. Runs the generated Cookfile Lua with capture-mode `cook.*` so it records `CapturedUnit`s instead of doing the work. |
| `cli/crates/cook-luaotp` | **Execute** | Long-lived pool of N worker threads. Each thread owns one `mlua::Lua` and pulls `WorkItem`s off a shared queue: `Shell`, `Interactive` (rejected — never dispatched), `LuaChunk`, and `Test`. |
| `cli/crates/cook-lua-stdlib` | **Both** | The shared `fs.*`, `path.*`, and `cook.platform.*` tables, plus the CS-0045 sandbox policy and `os.execute` / `io.popen` escape-hatch guards. Installed into the register-phase VM by `cook-register` and into every worker VM by `cook-luaotp` so both phases see byte-identical behavior (CS-0044). |
| `cli/crates/cook-contracts` | (types) | Behavior-free shared types: `WorkPayload`, `CapturedUnit`, `CacheMeta`, `DepKind`, `StepKind`, `RecipeUnits`, `OutputStream`. Zero deps on other Cook crates. |

`cook-engine` (not described here) orchestrates: it calls `cook-register` once per recipe to obtain a `RecipeUnits`, assembles all units into a global DAG, then dispatches `WorkItem`s to the `cook-luaotp` pool and consumes `WorkResult`s off the result channel. The old monolithic `Runtime` struct and the legacy single-threaded `execute_recipe()` path are gone — every execution now flows through capture → DAG → worker pool.

---

## Register phase (`cook-register`)

Entry point: `Registry::register_recipe()` at `cli/crates/cook-register/src/engine.rs:113`. One call per recipe.

For each call the registry:

1. Creates a fresh `mlua::Lua` (`engine.rs:119`).
2. Threads a `CacheContext` in as Lua app data so `cook.add_unit` can compute real `context_hash` / `env_contribution` values (`engine.rs:124`).
3. Builds the capture-mode `cook` table via `register_cook_api_capture` (`engine.rs:132`, implementation at `cli/crates/cook-register/src/capture.rs:27`). That installs `cook.recipe`, `cook.exec`, `cook.interactive`, `cook.sh`, and `cook.env`.
4. Installs the shared stdlib: `fs.*` (`engine.rs:163`), `path.*` (`engine.rs:168`), `cook.platform.*` (`engine.rs:178`), and the `os.execute` / `io.popen` shell escape guards (`engine.rs:169`). Sandbox is always `Confined { project_root }` for the register VM (`engine.rs:166`).
5. Installs the recipe-API extensions: `cook.add_unit` / `cook.step_group` / `cook._enter_chore` / `cook._exit_chore` / `cook.passthrough` (`cli/crates/cook-register/src/unit_api.rs:84`), `cook.add_test` (`cli/crates/cook-register/src/test_api.rs:11`), `cook.dep_output` / `cook.dep_output_list` (`cli/crates/cook-register/src/dep_output_api.rs:42`), `cook.export` / `cook.import` (`cli/crates/cook-register/src/export_api.rs:12`), `cook.json_decode` / `cook.yaml_decode` (`cli/crates/cook-register/src/codec_api.rs:6`), `cook.require_env` (`cli/crates/cook-register/src/env_api.rs:64`), `cook.load_module` (`cli/crates/cook-register/src/module_loader.rs:144`), and `cook.resolve_ingredients` (`cli/crates/cook-register/src/context.rs:47`).
6. `lua.load(lua_source).exec()` (`engine.rs:206`) — runs the codegen output produced by `cook-luagen`. This registers all recipes; bodies are not yet called.
7. Dispatches any config blocks (`engine.rs:211`), then re-applies CLI `--set KEY=VALUE` overrides, then freezes the declared env keyset and snapshots `cook.env` back into the shared map.
8. Looks up the target recipe, calls `setup_recipe_context` to populate `recipe.ingredients` (`cli/crates/cook-register/src/context.rs:10`), then invokes the recipe body (`engine.rs:297`).
9. Flushes module caches, then assembles a `RecipeUnits` out of `CaptureState` and returns it (`engine.rs:322`).

### Capture-mode `cook.*` semantics

| API | Register-phase behavior | Execute-phase behavior |
|---|---|---|
| `cook.recipe(name, meta, fn)` | Records `(name, ingredients, excludes, requires, fn)` in the registry (`capture.rs:39`). Body not called until the target recipe is selected. | Register-only — guarded with a §6.3.2 diagnostic in worker VMs (`cli/crates/cook-luaotp/src/pool.rs:535`). |
| `cook.exec(cmd, line)` | Pushes a `CapturedUnit { payload: WorkPayload::Shell, dep_kind }` onto `CaptureState.units` (`capture.rs:85`). Returns `""`. No subprocess. | Register-only — guarded with a §6.3.2 diagnostic (`pool.rs:504`). |
| `cook.interactive(cmd, line)` | Pushes a `CapturedUnit { payload: WorkPayload::Interactive, ... }` (`capture.rs:106`). Always sequential. | Register-only — guarded (`pool.rs:511`). The engine routes captured `Interactive` units through a dedicated foreground window before dispatch; the worker pool will surface a "BUG: interactive step dispatched" error if one ever reaches it (`pool.rs:776`). |
| `cook.sh(cmd)` | **Executes immediately** (`capture.rs:138`). `cook.sh` is the both-phase shell-out helper: its return value drives Lua control flow during capture (e.g. computing a version string used in subsequent `cook.add_unit` calls). | Executes via `run_shell_in_worker` in the worker VM (`pool.rs:341`). Phase: **Both** (Standard §6.3.1). |
| `cook.add_unit(table)` | Appends a `CapturedUnit` from a Lua table (see below) (`unit_api.rs:118`). | Register-only — guarded (`pool.rs:519`). |
| `cook.step_group(fn)` | Opens a parallel group, calls `fn()`, closes the group, and drains `current_step_outputs` into `last_cook_step_outputs` (`unit_api.rs:432`). | Register-only — guarded (`pool.rs:527`). |
| `cook.add_test(table)` | Appends a `CapturedUnit` with `WorkPayload::Test` and `DepKind::TestSibling(group)` so failures don't cancel siblings (`test_api.rs:15`). | Register-only (executed in the worker pool by `execute_test`, `pool.rs:944`). |
| `cook.dep_output(name)` / `cook.dep_output_list(name)` | Looks up `name`'s terminal outputs in the workspace-shared `SharedTerminalOutputs` map, accumulates a dep edge and the rewritten importer-relative paths into `CaptureState.step_group_dep_refs` / `step_group_dep_input_paths` so the next `cook.add_unit` picks them up (`dep_output_api.rs:64`). | Not present. |
| `cook.export(name, table)` / `cook.import(name)` | Cross-recipe data pass via a shared `BTreeMap<String, serde_json::Value>` (`export_api.rs:12`). | Not present. |
| `cook.env` | Lua table seeded from the resolved env vars; mutable while config blocks run, then snapshotted back into the host `HashMap` and frozen (`capture.rs:144`, `engine.rs:235`). | Lua table backed by an `__index` metatable that reads the worker's per-item `current_env_vars` slot at call time (`pool.rs:356`). |
| `cook.require_env(name)` | Returns `cook.env[name]` if `name` is in the frozen keyset; otherwise raises a diagnostic listing the declared keys (`env_api.rs:72`). | Not present. |
| `cook.platform.os` / `cook.platform.arch` | Set from `std::env::consts` at registration time (`cli/crates/cook-lua-stdlib/src/platform_api.rs:16`). | Same, registered once per worker. |
| `cook.json_decode(s)` / `cook.yaml_decode(s)` | Parses to a Lua table via `serde_json` / `serde_yml` (`codec_api.rs:6`). | Not present. |
| `cook.load_module(name)` | Resolves `cook_modules/<name>.lua` or `cook_modules/<name>/init.lua` relative to the recipe's working dir, evaluates it once, runs `init()` if present, memoizes the result, and detects cycles (`module_loader.rs:144`). | A worker-side counterpart exists for `use foo` modules referenced from execute-phase Lua chunks (`pool.rs:391`). Each worker VM keeps its own `_cook_module_cache` keyed by `(cwd, name)`. |
| `cook.passthrough(list)` | Pushes paths into `current_step_outputs` without recording an emitting unit. Used by codegen for plate/test/bare-shell steps so the recipe's terminal output list is still well-defined (`unit_api.rs:418`). | Not present. |
| `cook.taste()` | Removed. The DAG codegen path no longer emits it; the old debugger placeholder is gone. | — |

The register-only diagnostics on the worker VM cite Standard §6.3.2 and name the offending function so users get a focused message instead of `attempt to call a nil value`.

---

## Execute phase (`cook-luaotp`)

Entry point: `WorkerPool::spawn(n)` at `cli/crates/cook-luaotp/src/pool.rs:81`. Returns the pool and a single `mpsc::Receiver<WorkResult>` shared by all workers.

Each worker thread is a `worker_loop` (`pool.rs:153`) that:

1. Creates its own VM with `unsafe { mlua::Lua::unsafe_new() }` (`pool.rs:159`). `mlua::Lua` is `!Send`, but the VM is constructed on and never leaves the worker thread, so the `unsafe_new` constructor (which forgoes the cross-thread safety harness) is sound here. Each worker has exactly one VM for its lifetime — VM creation is amortized across many `WorkItem`s.
2. Registers `path.*` once at startup (`pool.rs:162`) — pure string manipulation, no per-item state.
3. Registers the `cook` table once via `register_worker_cook_table` (`pool.rs:329`) with closures that read per-item state out of `Arc<Mutex<_>>` slots: `current_recipe`, `current_working_dir`, `current_env_vars`, `current_sandbox`. This is what `cook.sh`, the `cook.env` metatable, and the live `fs.*` / sandbox sources read at call time.
4. Registers `fs.*` with `WorkingDirSource::Live` and `SandboxSource::Live` so a single VM serving items from multiple Cookfiles (CS-0017 multi-Cookfile imports) resolves each call against the active item's cwd and the active item's sandbox policy (`pool.rs:193`).
5. Installs the `os.execute` / `io.popen` escape-hatch guards with the same live sandbox source (`pool.rs:203`).
6. Installs register-only guards on `cook.exec` / `cook.interactive` / `cook.add_unit` / `cook.step_group` / `cook.recipe` (`pool.rs:504`–`pool.rs:542`).
7. Enters the work loop. Each iteration: pop a `QueueItem` (`pool.rs:210`); on `Shutdown`, break; on `Work(item)`, update the per-item slots, pick a `SandboxPolicy` from the payload's `StepKind` (`pool.rs:250`), refresh `package.path` / `package.cpath` for the unit's `cook_modules/` directory (`pool.rs:275`, `pool.rs:587`), run `execute_work_item` under `catch_unwind` (`pool.rs:287`), and send the `WorkResult` on the channel.

The `catch_unwind` boundary converts Rust panics into failure `WorkResult`s so the engine never hangs on `rx.recv()`. The Lua VM is reused after the panic — mlua catches panics raised inside Lua callbacks and converts them to Lua errors, so VM state stays sane.

### `WorkPayload` dispatch

`execute_work_item` (`pool.rs:735`) matches on the payload:

| Payload | Handler | Notes |
|---|---|---|
| `Shell { cmd, line }` | `execute_shell` (`pool.rs:804`) | `/bin/sh -c cmd` in `working_dir` with merged env. Output is line-split and tagged `(OutputStream::Stdout, _)` or `(_, Stderr)` so downstream renderers preserve fd-of-origin (CS-0035). Failure shapes a `COOK_CMD_FAILED:line:code:cmd` string with truncated captured streams. |
| `LuaChunk { code, inputs, outputs, ingredient_groups, step_kind, is_chore }` | `execute_lua_chunk` (`pool.rs:883`) | Sets `inputs` / `outputs` / `input` / `output` / `input_1`..`input_N` Lua globals, then `lua.load(code).exec()`. Sandbox policy was already installed into the per-item slot by the loop. |
| `Interactive { .. }` | Surfaces `"BUG: interactive step dispatched to worker pool"` (`pool.rs:776`) — the engine drains interactive units through a dedicated foreground window before dispatch. |
| `Test { cmd, line, timeout, should_fail, suite_name, test_name, .. }` | `execute_test` (`pool.rs:944`) | Spawns `/bin/sh -c cmd` with piped stdio, drains stdout/stderr in separate threads (to avoid pipe-buffer deadlocks), polls for completion against `timeout_secs`, and produces a `TestOutput` carrying duration, timed_out, exit_success, exit_code, and the `should_fail` inversion. |

### Per-worker `package.path` refresh

`refresh_package_search_paths` (`pool.rs:587`) is called before every work item. It stashes the original Lua `package.path` / `package.cpath` once, then for each unit prepends entries for the unit's `<cwd>/cook_modules/`:

- `package.path`: `<cwd>/cook_modules/?.lua`, `<cwd>/cook_modules/?/init.lua`, `<cwd>/cook_modules/share/lua/5.4/?.lua`, `<cwd>/cook_modules/share/lua/5.4/?/init.lua`, then the original.
- `package.cpath`: `<cwd>/cook_modules/?.<so-ext>`, `<cwd>/cook_modules/lib/lua/5.4/?.<so-ext>`, then the original. (`<so-ext>` is `dll` on Windows, `so` everywhere else.)

The original suffixes are stashed exactly once so per-unit refresh is idempotent across many calls.

---

## Shared APIs (`cook-lua-stdlib`)

The Standard tags `fs.*`, `path.*`, and `cook.platform.*` as **Phase: Both**. CS-0044 realizes that contract by giving each table a single implementation that both VMs install. Bug fixes to these surfaces MUST land here, not in `cook-register` or `cook-luaotp`.

### `WorkingDirSource`

`cli/crates/cook-lua-stdlib/src/lib.rs:55`. Abstracts how `fs.*` learns the cwd at call time:

- `Static(PathBuf)` — captured once at registration. Used by `cook-register` (one VM per recipe, cwd never changes).
- `Live(Arc<Mutex<PathBuf>>)` — resolved on every call. Used by `cook-luaotp`'s reusable workers, which serve items from possibly many Cookfiles within a single build.

### `SandboxSource` / `SandboxPolicy`

`cli/crates/cook-lua-stdlib/src/sandbox.rs:40`. CS-0045 project-root sandbox for `fs.*` and the Lua shell escape hatches. Two policies:

- `Off` — no confinement. Used by `plate` step Lua bodies (the explicit "ship outside the project" surface, Standard §{recipes.plate-step}).
- `Confined { project_root }` — relative paths get joined to `working_dir`; absolute paths are admitted only if their lexically-normalized form lies under `project_root`; relative paths with `..` segments that escape the root are rejected. Canonicalization is **lexical** (not `std::fs::canonicalize`) so `fs.write` / `fs.mkdir_p` can succeed against paths that don't yet exist.

The register-phase VM is always `Confined`. The execute-phase worker selects `Off` for `StepKind::Plate` and `Confined` for `Cook` / `Test` / `Chore` (`pool.rs:255`). Future `StepKind` variants default to `Confined` until a CS classifies them explicitly.

### `fs.*` (`cli/crates/cook-lua-stdlib/src/fs_api.rs`)

| Function | Returns | Notes |
|---|---|---|
| `fs.exists(path)` | bool | Returns true for files or directories. |
| `fs.size(path)` | u64 | Errors on missing path. |
| `fs.read(path)` | string | Full contents, UTF-8 lossy. |
| `fs.glob(pattern)` | table | Array of absolute paths; entries whose canonical form fails the sandbox check are silently filtered out. |
| `fs.mtime(path)` | f64 | Seconds since UNIX epoch. |
| `fs.write(path, content)` | nil | Creates parent directories with `mkdir -p` semantics. |
| `fs.mkdir_p(path)` | nil | `std::fs::create_dir_all`. |

Every entry runs `check_path` first (`fs_api.rs:176`), which resolves the user path against the active `WorkingDirSource` and validates it against the active `SandboxSource`.

### `path.*` (`cli/crates/cook-lua-stdlib/src/path_api.rs`)

Pure string manipulation using `std::path`. No I/O, no cwd dependency. Entries: `path.stem`, `path.name`, `path.ext`, `path.dir`, `path.replace_ext(p, ext)` (leading dot optional), `path.join(a, b)`.

### `cook.platform.*` (`cli/crates/cook-lua-stdlib/src/platform_api.rs`)

`cook.platform = { os = std::env::consts::OS, arch = std::env::consts::ARCH }`. Frozen at VM registration; never changes for the life of the VM.

### Shell escape guards (`cli/crates/cook-lua-stdlib/src/shell_guard.rs`)

Installs Lua-side guards on `os.execute` and `io.popen` so a confined step body can't use raw Lua I/O to escape the project root. The guards read from the same `SandboxSource` as `fs.*`, so plate bodies (which run with `Off`) keep the unguarded behavior.

---

## Capture-mode data structures

### `CaptureState` (`cli/crates/cook-register/src/lib.rs:55`)

Mutable state accumulated during a single `register_recipe()` call. Wrapped in `Rc<RefCell<_>>` and shared between every capture-mode closure. Intentionally `!Send` — registration is single-threaded with a single VM.

| Field | Type | Purpose |
|---|---|---|
| `inside_layer` | `bool` | True while the dry-run body of a legacy `cook.layer()` call is executing. (`cook.layer` is no longer emitted by codegen, but the flag is kept for any remaining call sites.) |
| `layer_commands` | `Vec<(String, usize)>` | Buffer for commands captured during a layer dry-run. |
| `units` | `Vec<CapturedUnit>` | All captured work units, in source order. |
| `current_group` | `Option<usize>` | Index of the currently-open step group, or `None`. |
| `step_groups` | `Vec<Vec<usize>>` | Parallel tiers: each inner vec contains indices into `units`. |
| `current_step_outputs` | `Vec<String>` | Outputs collected during the current `cook.step_group` call; drained into `last_cook_step_outputs` when the group closes. |
| `last_cook_step_outputs` | `Vec<String>` | Terminal outputs — the outputs from the last completed cook step group. Last one wins. |
| `dep_edges` | `Vec<(usize, String)>` | Fine-grained cross-recipe edges `(unit_index, dep_recipe_name)`, recorded when `cook.add_unit` runs after one or more `cook.dep_output(...)` calls in the same step group. |
| `step_group_dep_refs` | `Vec<String>` | Dep refs accumulated by `cook.dep_output*` calls within the current `cook.step_group`; cleared at group close. |
| `step_group_dep_input_paths` | `Vec<String>` | Importer-relative rewritten output paths returned by `cook.dep_output*`; surface as `cache_meta.input_paths` entries on the next `cook.add_unit`. |
| `current_chore_active` | `bool` | True between `cook._enter_chore()` and `cook._exit_chore()`. `cook.add_unit` rejects `cache = true` while this is set (Standard §{chores.no-caching}). |
| `current_recipe` | `Option<String>` | Fully-qualified name of the currently-executing recipe; used by `cook.add_test` to default `suite` to the enclosing recipe's name (CS-0061 §3.2). |

### `CapturedUnit` and `WorkPayload` (`cli/crates/cook-contracts/src/lib.rs`)

```rust
pub struct CapturedUnit {
    pub payload: WorkPayload,
    pub cache_meta: Option<CacheMeta>,
    pub dep_kind: DepKind,
}
```

`WorkPayload` (`cook-contracts/src/lib.rs:58`) is `#[non_exhaustive]` with four variants:

- `Shell { cmd, line }` — a `/bin/sh -c` command.
- `Interactive { cmd, line, is_chore }` — runs with inherited stdio in the engine's foreground window. `is_chore` distinguishes a unit emitted between `_enter_chore` / `_exit_chore` from a legacy `interactive = true` shell step in a regular recipe.
- `LuaChunk { code, inputs, outputs, ingredient_groups, step_kind, is_chore }` — raw Lua source executed by a worker. `step_kind` is what the execute-phase sandbox picker reads; missing/unknown values default to `StepKind::Cook` (the strictest contract, so a misclassified plate body degrades to a Lua runtime error instead of silently writing outside the project).
- `Test { cmd, line, timeout, should_fail, suite_name, test_name, iteration_item }` — handled by `execute_test`.

`cache_meta` is `Some(CacheMeta)` for cacheable units (cook-step bodies and explicit `cache = true` shells) and `None` for plate / test / chore / `cache = false` units. The `CacheMeta` carries `cache_key`, `input_paths`, `output_paths`, `command_hash`, `context_hash`, `env_contribution`, `consulted_env`, an optional `DiscoveredInputs`, and the project / Cookfile identity (`project_id`, `cookfile_path`).

### `DepKind` (`cook-contracts/src/lib.rs:170`)

```rust
#[non_exhaustive]
pub enum DepKind {
    StepGroup(usize),     // parallel sibling within group `i`
    Sequential,           // sequential barrier after all prior units
    TestSibling(usize),   // like StepGroup, but failures don't cancel siblings
}
```

The DAG builder maps `StepGroup(i)` to a parallelism tier, `Sequential` to a barrier edge after the most recent tier, and `TestSibling(i)` to a tier whose failure mode is reported-but-not-cancelling.

### `RecipeUnits` (`cook-contracts/src/lib.rs:182`)

```rust
pub struct RecipeUnits {
    pub recipe_name: String,
    pub deps: Vec<String>,                 // from metadata.requires
    pub units: Vec<CapturedUnit>,
    pub step_groups: Vec<Vec<usize>>,      // indices into `units`
    pub working_dir: PathBuf,
    pub env_vars: BTreeMap<String, String>,
    pub terminal_outputs: Vec<String>,     // last cook step's outputs
    pub dep_edges: Vec<(usize, String)>,   // (unit index, dep recipe name)
}
```

The complete output of one `register_recipe` call. Consumed by `cook-engine` to assemble the global DAG. `BTreeMap` is mandatory for `env_vars` — deterministic order is what makes cache fingerprints reproducible.

---

## Recipe context setup

`setup_recipe_context` (`cli/crates/cook-register/src/context.rs:10`) runs just before the recipe body is called. It builds a Lua `recipe` global with:

- `recipe.name` — the recipe's bare name.
- `recipe.ingredients` — a nested table: `recipe.ingredients[i]` is the sorted array of relative paths matching the i-th `ingredients` glob, with the recipe's `excludes` patterns subtracted.

Glob expansion happens once per registration (not on every Lua access); patterns are joined to `working_dir`, expanded with `glob::glob`, stripped back to relative paths, and stored in a `BTreeSet` for sorted/dedup-by-construction output. The Cookfile-level cache invalidation that the old monolithic `Runtime` did here is no longer the runtime's job — that work moved to `cook-cache` / `cook-engine`, which compute `context_hash` and consult `CacheMeta.input_paths` directly.

`cook.resolve_ingredients(includes, excludes)` (`context.rs:47`) exposes the same glob+exclude pipeline as a Lua function for codegen's iteration patterns.

---

## Sandboxing recap (`StepKind` → policy)

CS-0045 makes the sandbox an attribute of the captured unit, not of the VM:

| Step kind | `WorkPayload.step_kind` | Worker policy | Rationale |
|---|---|---|---|
| `cook` | `StepKind::Cook` | `Confined { project_root }` | Cacheable, hermetic. Must be derivable from the cache fingerprint. |
| `test` | `StepKind::Test` | `Confined` | Non-cacheable but hermetic-by-intent. |
| `chore` | `StepKind::Chore` | `Confined` | Non-cacheable, hermetic-by-intent. |
| `plate` | `StepKind::Plate` | `Off` | Explicit "ship outside the project" surface (deploys, uploads). |

Register-phase Lua is always `Confined { project_root }` (`engine.rs:166`) — captured `lua_code` strings will replay at execute time with their own per-step-kind policy applied there, so register-time helper I/O (e.g. files read while resolving a `cook.dep_output(...)`) just needs to stay inside the project.

The sandbox is enforced by `check_path` inside every `fs.*` entry and by the `os.execute` / `io.popen` guards. Both consult the same `SandboxSource`, so a plate body that hands off to `os.execute` keeps the same `Off` policy as its `fs.write` calls.

---

## What changed since the old doc

- The monolithic `Runtime` struct at `src/runtime/mod.rs` is gone. Register and execute live in separate crates with no shared state at the type level.
- The legacy single-threaded `execute_recipe()` path is gone. Every recipe is captured first; all execution happens in the worker pool.
- The Lua VM is no longer "fresh per recipe" on the execute side — workers reuse a single VM across many `WorkItem`s. The register side still creates one VM per recipe.
- `cook.layer()` / `cook.begin_step()` / `cook.end_step()` / `cook.taste()` are no longer emitted by codegen. Codegen now emits `cook.step_group(function() ... end)` containing one or more `cook.add_unit({...})` calls; chores wrap their body with `cook._enter_chore()` / `cook._exit_chore()`.
- New capture-mode surface: `cook.add_unit`, `cook.step_group`, `cook.passthrough`, `cook.add_test`, `cook.dep_output` / `cook.dep_output_list`, `cook.export` / `cook.import`, `cook.require_env`, `cook.load_module`, `cook.json_decode` / `cook.yaml_decode`, `cook.platform.os` / `cook.platform.arch`.
- `cook.dep_output` resolution is workspace-global. Importer aliases rewrite output paths into importer-relative form; diamond imports are resolved through `alias_qualified_prefixes` so two import chains reaching the same importee see one canonical storage key.
- The `fs.*` / `path.*` / `cook.platform.*` tables are now defined once in `cook-lua-stdlib` and registered into both VMs (CS-0044). Bug fixes must land there.
- Project-root sandbox (CS-0045) is enforced inside `fs.*` and on the Lua shell escape hatches, with the policy picked per work item from `WorkPayload.step_kind`.
