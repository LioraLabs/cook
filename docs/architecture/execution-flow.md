# Execution Flow: End-to-End Trace

This traces what happens when you run `cook build` from CLI to completion, following the data through every stage.

---

## 1. Overview

`cook build` is a recipe invocation. After clap dispatch the CLI hands the recipe target off to `cook-engine`, which parses the Cookfile (and any imported Cookfiles), assembles a per-namespace registry of compiled Lua sources, computes the recipe DAG, groups recipes into **waves**, and executes the waves in topological order. Within a wave, every recipe registers concurrently in its own `mlua::Lua` VM; the resulting work-unit DAGs are merged and driven by a worker pool that can interleave units from many recipes at once.

The chain at a glance:

```
cook-cli::main()              process entry, clap parse, exit-code mapping
  └─ cook-cli::dispatch()     Cmd enum → cmd_run / cmd_test / cmd_dag / ...
       └─ cook-cli::cmd_run()             thin CLI wrapper
            ├─ pipeline::read_and_parse()           AST + Lua source + warnings
            ├─ pipeline::validate_selected_config()
            ├─ (workspace? → pipeline::Workspace::load + workspace_* builders)
            │   (single?    → pipeline::resolve_env + single_* builders)
            ├─ pipeline::compute_*_inferred_deps()  {NAME} body refs → edges
            └─ cook_engine::run::run()
                 ├─ cache bootstrap (CloudConfig, CacheContext)
                 ├─ analyzer::dependency_edges_multi() → recipe DAG
                 ├─ wave_grouper::compute_waves()     → Vec<Wave>
                 └─ for each wave (in topo order):
                      ├─ register every recipe (concurrent, per-VM)
                      ├─ dag_builder::build_dag()    → work-unit DAG
                      └─ executor::execute_dag()     → run via WorkerPool
```

The "two-phase" model (register → execute) still exists, but it is now scoped **per wave** rather than per recipe: a whole wave's recipes are registered, their work-unit DAGs are merged, and the merged DAG runs as one unit before the next wave begins. Cross-wave ordering is preserved by edges in the recipe graph; within a wave there are no recipe boundaries that block execution — work units from different recipes can run interleaved.

---

## 2. CLI dispatch

### 2.1 Process entry — `cli/crates/cook-cli/src/main.rs:20`

`main()` builds a versioned clap `Command` (the version string is composed at runtime so it can append the Cook Standard version from `cook_lang::COOK_STANDARD_VERSION`), parses `argv`, and calls `dispatch(cli)`. On error it prints `cook: {e}` to stderr (suppressed for `CookError::TestFailure`, where the reporter already printed a summary) and exits with `e.exit_code()`.

**Data in:** `argv`
**Data out:** populated `Cli` value, or process exit with a `CookError` exit code

### 2.2 Subcommand routing — `cli/crates/cook-cli/src/cli.rs:69` (`enum Cmd`)

`Cli` is a clap-derive struct with a flattened `Globals` and an `Option<Cmd>`. The `Cmd` enum carries the reserved subcommands (`Init`, `Menu`, `List`, `Modules`, `Test`, `Dag`, `Logs`, `Serve`, `EmitLua`) plus a catch-all `Recipe(Vec<String>)` variant marked `#[command(external_subcommand)]`. Any first positional that does not match a reserved name lands in `Cmd::Recipe`. A bare invocation (`cook` with no args) leaves `cmd = None`.

`dispatch` in `main.rs:43` translates the variant:

| Cmd                | Handler                                            |
|--------------------|----------------------------------------------------|
| `None`             | `cmd_run(&globals, "build", None)`                 |
| `Cmd::Recipe(parts)` | `dispatch_recipe` → strip leading `+`, then `cmd_run` |
| `Cmd::EmitLua`     | `cmd_emit_lua` (note: `--emit-lua` is no longer a flag) |
| `Cmd::Test(args)`  | `cmd_test`                                         |
| `Cmd::Dag(args)`   | `cmd_dag` (feature-gated on `viewer`)              |
| `Cmd::Serve(args)` | `cmd_serve`                                        |
| `Cmd::Init/Menu/List/Logs/Modules` | their respective `cmd_*`            |

`dispatch_recipe` (`main.rs:68`) is the recipe escape path: a single leading `+` is stripped (`cook +test` runs the recipe named `test`, sidestepping the reserved `Cmd::Test`), and the optional second positional becomes the named-config argument. So `cook build` and `cook +build` both end up at `cmd_run(&globals, "build", None)`.

**Globals** (defined in `cli.rs:33`, all `global = true`): `-f / --file`, `--root`, `-q / --quiet`, `-v / --verbose`, `-j / --jobs`, `--color`, `--output`, `--set KEY=VALUE` (repeatable).

**Data in:** parsed `Cli`
**Data out:** a single `cmd_*` call

### 2.3 CLI glue — `cli/crates/cook-cli/src/pipeline.rs`

`pipeline.rs` is intentionally thin: it does not consume any `cook_lang::ast::Cookfile` directly. Its job is

1. forward to `cook_engine::pipeline::*` for parse / workspace / env / registry / inferred-dep work;
2. spin up a `cook-progress` renderer thread plus a bridge thread that translates `cook_engine::EngineEvent`s into `cook_progress::ProgressEvent`s (`bridge_engine_to_progress_events`, line 84);
3. drive `cook_engine::run::run`;
4. map `PipelineError` → `CookError` (`pipeline_error_to_cook_error`, line 31) and `EngineError` → `CookError` (`engine_error_to_cook_error`, line 344) for exit-code classification.

`cmd_run` (`pipeline.rs:447`) is the entry for `cook build`. It branches on whether the Cookfile has imports: workspace builds load through `Workspace::load`, single-file builds skip that and use the `single_*` builders. Both paths converge on `run_with_progress` (line 396), which is the only place that calls `cook_engine::run::run`.

**Data in:** `Globals`, recipe name, optional named-config name
**Data out:** `Ok(())` or `CookError`

---

## 3. Parse + workspace load

### 3.1 `read_and_parse` — `cli/crates/cook-engine/src/pipeline/parse.rs:36`

```text
path → ParsedCookfile { cookfile, lua_source, warnings }
```

- `cook_lang::parse(&source)` produces the AST (`PipelineError::Parse` on failure).
- `cook_luagen::dep_ref::extract_recipe_names` pre-scans recipe names for codegen disambiguation.
- `cook_luagen::generate_with_names_checked` runs codegen with § 5.4 placement validation enabled (hard error → `PipelineError::Codegen`).
- `cook_luagen::generate_with_names_and_warnings` runs codegen again under the § 5.5 policy to collect warnings — the Lua source is byte-identical; only the diagnostic policy differs.

The CLI wraps this in `cook-cli/src/pipeline.rs:49`, which prints each warning to stderr (`cook: warning: …`) before returning the `ParsedCookfile`.

`pipeline::validate_selected_config` (`parse.rs:65`) rejects an unknown `--config NAME` argument up front, listing available names.

**Data in:** path to Cookfile
**Data out:** AST, generated Lua source, warnings

### 3.2 Workspace load (when the root Cookfile has imports) — `cli/crates/cook-engine/src/pipeline/workspace.rs:39`

When `parsed.cookfile.imports` is non-empty, `cmd_run` resolves the workspace root (either `--root` or the discovered project root) via `pipeline::resolve_workspace_root` and then calls `Workspace::load`. The loader:

- canonicalises the root Cookfile path and the workspace-root path,
- parses + codegens the root,
- recursively follows `import` directives via `Self::load_imports`, anchoring `//path/from/root` sigil imports at `workspace_root` and `./path` imports at the importer's directory,
- detects cycles by tracking the set of visited canonical directories,
- deduplicates the same canonical target reached via two aliases.

The result is a `Workspace { root: LoadedCookfile, imports: BTreeMap<PathBuf, LoadedCookfile>, namespace_map, workspace_root }`.

**Data in:** root Cookfile path, workspace root, `--set` overrides (forwarded)
**Data out:** fully-resolved `Workspace`

For single-Cookfile builds this step is skipped: `cmd_run` operates directly on `parsed.cookfile` and uses `pipeline::build_single_recipe_infos` / `pipeline::build_single_registries`.

---

## 4. Registry and recipe-info assembly

### 4.1 `RecipeInfo` map — `cli/crates/cook-engine/src/pipeline/recipe_info.rs`

For each recipe across the workspace, `pipeline::build_workspace_recipe_info` (or `build_single_recipe_infos` for the single-file case) produces a `BTreeMap<String, RecipeInfo>` keyed by fully-qualified recipe name (e.g. `apps.web.build` for an imported recipe under alias `apps.web`). `RecipeInfo` (`analyzer.rs:37`) is the pure-data graph node used by the analyzer:

```rust
pub struct RecipeInfo {
    pub ingredients: Vec<String>,
    pub serves: Vec<String>,
    pub requires: Vec<String>,
}
```

`ingredients` / `serves` are recorded for introspection (`cook menu`, `cook dag`) but **do not produce dependency edges** — Cook Standard § 5.6 and rationale B.5.N removed ingredient-serves matching. Only `requires` (explicit `: dep`) and inferred-dep edges (next step) create edges.

### 4.2 `RegistryEntry` map — `cli/crates/cook-engine/src/pipeline/registries.rs`

`pipeline::build_workspace_registries` / `build_single_registries` build a `BTreeMap<String, RegistryEntry>` keyed by **namespace prefix**: `""` for the root, `apps.web` for an import aliased that way, etc. Each `RegistryEntry` carries the compiled Lua source plus a `cook_register::Registry` configured with the namespace's working directory, env vars, `--set` overrides, and the optionally-selected config-block name. The engine looks up the right entry per recipe at registration time (see § 6).

**Data in:** parsed AST(s), env vars, named-config name, `--set` overrides
**Data out:** `BTreeMap<String, RegistryEntry>` plus `BTreeMap<String, RecipeInfo>`

### 4.3 Inferred deps — `cli/crates/cook-engine/src/pipeline/inferred_deps.rs:29`

`compute_workspace_inferred_deps` walks every recipe body looking for `{NAME}` body references (Cook Standard § 5.3 / App. E.10), resolving them through any import aliases. The output is a `BTreeMap<String, Vec<String>>` from consumer recipe → referenced recipes. There is no separate single-Cookfile helper: a Cookfile with no imports loads as a workspace of one member (prefix `""`), so the workspace walk covers it (the former `compute_single_inferred_deps` twin is deleted).

These are **codegen-time** dependencies. Unlike explicit `requires` (which become wave boundaries), inferred deps cause **same-wave merging** in the wave grouper: a recipe and any recipe it body-references end up in the same wave so the referencing recipe sees the referent's outputs when it registers.

`pipeline::workspace_dep_conflicts` reports the cases where a `{NAME}` reference conflicts with an explicit dep declaration (its former `single_dep_conflicts` twin is deleted with the single-Cookfile path).

**Data in:** Cookfile AST(s)
**Data out:** `BTreeMap<String, Vec<String>>` inferred edges, plus diagnostic warnings

---

## 5. Environment resolution

`cli/crates/cook-engine/src/pipeline/env.rs:33` defines the layered merge. Layer order (later wins):

1. **System env** — `std::env::vars()`.
2. **`.env` file** — loaded by `pipeline::load_env(cookfile_dir)` (`env.rs:20`), parsed with `dotenvy`.
3. **CLI `--set KEY=VALUE` overrides** — parsed by `pipeline::parse_cli_overrides` (`env.rs:60`), split on the first `=`. Missing `=` is rejected with `PipelineError::InvalidSet`.

Note the change from earlier versions: **bare cookfile vars (`CC "gcc"`) are gone**. Cookfile-defined variables now live inside `config NAME ... end` blocks and are applied by the registry's Lua VM at registration time — not by `resolve_env`. CLI `--set` overrides are also re-applied on top of `cook.env` after the config block runs, so explicit CLI overrides win over config-block defaults regardless of how the block was authored. `resolve_env` takes a `selected_config` parameter but ignores it: config-block dispatch is the registry's job.

For workspace builds, `cmd_run` does not call `resolve_env` directly — env layering is folded into `build_workspace_registries` per namespace, so each imported Cookfile sees its own `.env` (loaded relative to that Cookfile's directory).

**Data in:** optional `--config` name, `.env` map, `--set` strings
**Data out:** `HashMap<String, String>` merged env (single-file path), or per-namespace `RegistryEntry`s (workspace path)

---

## 6. Recipe DAG and wave grouping

Inside `cook_engine::run::run` (`cli/crates/cook-engine/src/run.rs:314`):

### 6.1 Cache bootstrap (`run.rs:354–432`)

Load `.cook/cloud.toml` (default if absent), build an env-denylist, probe `ExecutionContext` (machine identity, declared-tool binary hashes), pick a backend (`CloudBackend` when `cloud.enabled` is true, else `LocalBackend` at `cache_dir()`), and assemble an `Arc<CacheContext>` that flows to every worker. Backend health is probed once; failure is logged but does not abort the build (the backend is treated as disabled).

### 6.2 Recipe DAG — `analyzer::dependency_edges_multi` (`analyzer.rs:103`)

`dependency_edges_multi` builds an adjacency map from `requires` declarations only (`build_adjacency`, `analyzer.rs:55`) and merges per-target reachability sets. It performs a DFS topological reachability check; `GraphError::UnknownRecipe` and `GraphError::CycleDetected` surface up as `EngineError::UnknownRecipe` / `EngineError::CycleDetected`.

The edges map is `BTreeMap<String, Vec<String>>`: recipe name → recipes it depends on. Only `requires` and codegen-emitted name-reference edges feed this map; ingredient-serves matching is gone (see § 4.1).

### 6.3 Wave grouping — `wave_grouper::compute_waves` (`run.rs:449`)

`compute_waves` takes three inputs: `edges` (explicit deps; produce wave boundaries), `inferred_deps` (`{NAME}` refs; merge into the same wave), and the set of all reachable recipe names. It returns `Vec<Wave>` where each `Wave` carries a sorted list of recipe names that can register and execute in the same wave.

After waves are computed the engine emits a single `EngineEvent::BuildStarted` describing the topology, followed by an `EngineEvent::RecipeQueued` per recipe (`run.rs:466`–`476`).

**Data in:** `recipe_infos`, `targets`, `inferred_deps`
**Data out:** `Vec<Wave>` plus initial topology events

---

## 7. Per-wave register / build / execute loop

The main loop is `for wave in &waves { ... }` (`run.rs:481`).

### 7.1 Registration (`run.rs:482`–`519`)

For each recipe in the wave:

1. `split_recipe_name(name)` splits a fully-qualified name into `(prefix, local_name)` — e.g. `apps.web.build` → `("apps.web", "build")`.
2. The matching `RegistryEntry` is looked up by prefix. A missing entry is a hard error (`EngineError::RegistrationFailed`).
3. `registry.register_recipe(lua_source, local_name, Some(cache_ctx))` runs the recipe in **capture mode** inside a fresh `mlua::Lua` VM. The Cook API is registered with capture semantics: `cook.exec`, `cook.layer`, `cook.add_unit`, etc. record `CapturedUnit`s into the registry's `CaptureState` instead of executing anything. See `cli/crates/cook-register/src/lib.rs` and `engine.rs` for the API surface.
4. The recipe's qualified name is written back into `units.recipe_name`, cross-recipe deps from `edges` are copied into `units.deps`, and a `ThreadSafeCacheManager` rooted at `<registry-working-dir>/.cook/cache` is created.

Recipes inside a wave can register **concurrently** in principle (one thread per recipe, since `mlua::Lua` is `!Send`). In the current implementation the loop is sequential but the structure (one VM per registration, distinct `CaptureState`s per recipe) is what enables that concurrency.

The output of the wave registration is a `Vec<RecipeUnits>`. Each `RecipeUnits` carries the captured units, recipe-local dep info, and cross-recipe `deps` derived from the edge map.

**Data in:** wave's recipe names, registries, `cache_ctx`, `edges`
**Data out:** `Vec<RecipeUnits>` for the wave, plus per-recipe `ThreadSafeCacheManager`s

### 7.2 Work-unit DAG — `dag_builder::build_dag` (`run.rs:524`)

`build_dag` merges every `RecipeUnits` in the wave into one `Dag<WorkNode>`. It wires:

- intra-recipe ordering (the `Sequential` / `StepGroup` / explicit-dep relations the capture API recorded),
- cross-recipe edges (the `deps` field copied from `edges`),
- inferred-dep edges (`{NAME}` refs, threaded through during workspace recipe-info assembly).

The builder cannot introduce cycles by construction (deps only point to already-emitted node ids); a defensive `dag.validate()` in `execute_dag` catches any future regression with `EngineError::CycleDetected`.

If two unrelated recipes both declare the same canonical output path, `build_dag` returns `EngineError::OutputCollision { path, recipes }`. This is a plan-time error — the wave loop bails before any work runs.

Zero-work recipes (meta-targets whose body only declares `: dep` edges) never produce a node in the DAG. `run.rs:529`–`548` emits synthetic `RecipeStarted` + `RecipeCompleted` events for them so they don't get stuck in the renderer's Waiting state.

**Data in:** `Vec<RecipeUnits>`
**Data out:** `Dag<WorkNode>` for this wave (possibly empty)

### 7.3 Execution — `executor::execute_dag` (`cli/crates/cook-engine/src/executor.rs:280`)

`execute_dag` drives the wave's work-unit DAG. Briefly:

1. **Empty / cycle checks.** Empty DAG returns `Ok(vec![])`. `dag.validate()` defensively guards against cycles.
2. **Worker pool.** `cook_luaotp::WorkerPool::spawn(num_workers)` starts `N` threads. Each worker owns its own `mlua::Lua` VM and pulls `WorkItem`s off a `(Mutex<VecDeque>, Condvar)` queue; results return on an mpsc channel.
3. **Seed.** `dag.initial_ready()` (`executor.rs:978`) returns every node with zero remaining deps; each goes through `process_ready` which dispatches by payload kind: `None` (presatisfied / cache hit) is completed inline; `Interactive` is queued for main-thread execution; anything else is submitted to the pool.
4. **Main loop** (`executor.rs:1001`). The thread blocks on the result channel. On success it calls `dag.complete(id)` and dispatches newly-ready nodes; on failure it accumulates the failure and calls `cancel_subtree` to mark transitive dependents as cancelled (and synthesize `Blocked` test results for `cook test`).
5. **Interactive / chore window.** When the pool is drained and the interactive queue is non-empty, the main thread runs the queued node directly with stdin attached. Chore bodies are emitted as a linear chain of interactive units bracketed by `_enter_chore` / `_exit_chore` and drain together as a single window with one `InteractiveStart` / `InteractiveEnd` pair (CS-0051).
6. **Recipe tracking.** `RecipeTracker`s aggregate per-recipe progress and emit `RecipeStarted` / `RecipeCompleted` / `RecipeFailed` once a recipe's node count hits zero.
7. **Cache.** Each successful node updates its recipe's `ThreadSafeCacheManager`; the cache flushes at the end of the wave.
8. **Termination.** The loop exits when every node is accounted for (`finished == total`) and the interactive queue is empty. The pool is shut down and joined.

Failures are returned as `EngineError::TaskFailures { failures: Vec<(node_id, recipe_name, message)>, partial_test_results }`. Test mode unwraps `partial_test_results` (Blocked rows from cancellation) and treats it as a successful run with failing rows; build mode propagates the failure upward.

Per-wave parallelism is therefore **all work units across all recipes in the wave**, capped at `num_workers`. The within-recipe DAG structure (sequential barriers vs. step groups) still constrains ordering inside a recipe; the wave boundary just adds the cross-recipe edges on top.

**Data in:** work-unit DAG, worker count, per-recipe cache managers, `cache_ctx`, event channel
**Data out:** `Vec<TestResult>` (empty for build mode) or `EngineError::TaskFailures`

---

## 8. Cache update

`ThreadSafeCacheManager` is per-recipe; `execute_dag` writes to it as each `cache_meta`-carrying node finishes successfully and flushes at end-of-wave. The cache directory layout is `<recipe-working-dir>/.cook/cache/`. The cloud backend (when `cloud.enabled` in `.cook/cloud.toml`) sees the same writes via the `CacheBackend` trait through `CacheContext::backend`; a failed backend is logged and the build continues with the backend treated as disabled.

`cook-fingerprint` installs a depfile-parser shim once per process (`executor.rs:296`) so the precheck augmentation can resolve `.d` files without a runtime dep cycle.

**Data in:** completed `WorkNode`s with cache metadata
**Data out:** updated on-disk cache + (optionally) cloud cache

---

## 9. Output and error handling

### 9.1 What the user sees on success

There is no longer any direct `println!` from the engine. All progress flows through `EngineEvent`s emitted from `run::run` and `executor::execute_dag` over an mpsc channel. `cook-cli/src/pipeline.rs:84` (`bridge_engine_to_progress_events`) translates each event into a `cook_progress::ProgressEvent` with stable `RecipeId` / `NodeId` interning, and `cook-cli/src/progress.rs::spawn_new_renderer` runs the renderer on a separate thread.

The renderer respects `--output` (`auto` → inline TTY, `plain` → line-prefixed, `json` → newline-delimited JSON) and `--verbose` (per-node output streamed under `[recipe/node]`). The old `SharedWriter` is gone.

On success the engine emits `EngineEvent::Finished { success: true }`, the renderer prints its summary, and `run_with_progress` returns `Ok(())`. `dispatch` returns `Ok(())` and the process exits 0.

### 9.2 What the user sees on failure

A failed work unit in `execute_dag` produces an `EngineError::TaskFailures`. `engine_error_to_cook_error` (`cook-cli/src/pipeline.rs:344`) inspects the first failure: if the message contains the `COOK_CMD_FAILED:<line>:<code>:<cmd>` sentinel emitted by the cook runtime, it decodes it into `Cookfile:<line>: command failed (exit <code>): <cmd>`; otherwise it forwards the raw message. Cycle / unknown-recipe / registration / cache / output-collision errors map to dedicated `CookError` variants.

`main.rs:33` prints `cook: {e}` to stderr (unless the variant is `TestFailure`) and calls `process::exit(e.exit_code())`.

### 9.3 Exit codes

`CookError::exit_code` (`cli/crates/cook-cli/src/error.rs:17`):

| Variant            | Exit code | Example message                                                  |
|--------------------|-----------|------------------------------------------------------------------|
| `CommandFailed`    | 1         | `Cookfile:42: command failed (exit 1): gcc main.c`               |
| `TestFailure`      | 1         | (summary printed by reporter; the error message is suppressed)   |
| `Other`            | 1         | `dependency cycle involving: a`, `output collision: ...`, etc.   |
| `ParseError`       | 2         | `parse error: unexpected token at line 5`                        |
| `RecipeNotFound`   | 3         | `recipe not found: myrecipe`                                     |

The mapping is the only place exit codes are decided; engine and pipeline errors carry no exit-code information of their own.
