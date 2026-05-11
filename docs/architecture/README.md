# Cook — Architecture Overview

## What Cook Is

Cook is a modern build system that combines Make's dependency-tracking power, Just's recipe clarity, and an embedded Lua scripting layer. It reads a `Cookfile` written in a custom DSL, transpiles it to Lua, and runs the result through a Lua VM backed by a parallel task scheduler with content-hash-based incremental caching. The result is a hybrid task runner and build system: you can describe file dependencies and get incremental rebuilds, or you can write pure task pipelines and get parallelism for free.

> **Language definition.** This directory documents how the implementation works. For the definition of the Cookfile language itself — syntax, semantics, Cook Lua API, modules — see `standard/`.

---

## System Pipeline

```
Cookfile (text)
  → Lexer (cook-lang::lexer)
    → Tokens
      → Parser (cook-lang::recipe, cook_line, lua_block)
        → AST (cook-lang::ast)
          → Codegen (cook-luagen::recipe, cook_step, plate_step)
            → Lua source
              → Workspace load + Registry assembly (cook-engine::pipeline)
                → Per-wave Registration in capture mode (cook-register)
                  → RecipeUnits (cook-contracts::{CapturedUnit, WorkPayload})
                    → DAG Builder (cook-engine::dag_builder, recipe_dag, wave_grouper)
                      → Cache check (cook-fingerprint + cook-cache)
                        → Wave-parallel execution (cook-engine::executor)
                          → Worker pool with Lua VMs (cook-luaotp)
                            → Shell processes / Lua chunks
```

Every `cook` invocation follows this path from left to right. The split between the **register phase** (capture, single-threaded, no side effects) and the **execute phase** (parallel, real I/O) is the key architectural seam: the engine registers each wave of recipes through `cook-register` to discover what work exists, then feeds the captured units to the executor which dispatches them through `cook-luaotp`'s worker pool.

---

## Crate Layout

The `cook` binary is a multi-crate workspace under `cli/crates/`. There is no top-level `src/` — every module is a sibling crate. Module boundaries are real (each crate has its own `Cargo.toml`), and cross-crate types live in `cook-contracts`.

```
cli/crates/
├── cook-cli              # binary entry point, CLI dispatch, progress bridge, watcher
├── cook-engine           # orchestration: pipeline, recipe DAG, work-unit DAG, executor
├── cook-lang             # Cookfile lexer + parser → AST (cook-contracts-free)
├── cook-luagen           # AST → Lua source string (codegen)
├── cook-register         # capture-mode Lua VM that runs generated source to discover units
├── cook-luaotp           # worker pool: N threads, one Lua VM per thread, executes WorkPayloads
├── cook-lua-stdlib       # shared fs.*/path.*/cook.platform.* APIs (used by register + workers)
├── cook-cache            # filesystem cache backend, RecipeCache file format, cloud backend
├── cook-fingerprint      # pure hashing, env contribution, machine identity, rebuild logic
├── cook-contracts        # behaviour-free shared types (WorkPayload, CapturedUnit, CacheMeta, …)
├── cook-dag              # generic Dag<T> with atomic remaining_deps and cycle detection
├── cook-dag-viewer       # TUI for inspecting a recipe DAG (cook dag)
└── cook-progress         # event-driven renderer (Inline/Plain/JSON) + on-disk log store
```

---

## Module Map

| Crate | Role | Key dependencies |
|---|---|---|
| `cook-cli` | Binary: clap parsing, dispatches each subcommand to engine entry points, bridges `EngineEvent` → `cook_progress::ProgressEvent`, owns `cook serve` watcher | `cook-engine`, `cook-progress`, `cook-lang`, `cook-cli` (lib) |
| `cook-engine::pipeline` | Parse Cookfile, walk imports (`Workspace`), assemble `RegistryEntry` map, compute name-reference deps | `cook-lang`, `cook-luagen`, `cook-register` |
| `cook-engine::{run,executor,...}` | Wave-parallel orchestration: registers each wave through `cook-register`, builds work-unit DAG, runs cache lookups, schedules through `cook-luaotp` | all other engine submodules, `cook-cache`, `cook-fingerprint`, `cook-luaotp` |
| `cook-engine::analyzer` | Recipe-graph adjacency build and topological sort over `BTreeMap<String, RecipeInfo>` | (none) |
| `cook-engine::dag_builder` | Converts `RecipeUnits` → work-unit DAG nodes; wires barriers and step-group parallelism | `cook-contracts`, `cook-dag` |
| `cook-lang` | Lexer + parser → `Cookfile` AST (`ast`, `lexer`, `recipe`, `cook_line`, `lua_block`, `shell_block`, `brace_scan`) | (none) |
| `cook-luagen` | Walks AST, emits Lua source; resolver, template/sigil expansion, `dep_ref` validation | `cook-lang`, `cook-contracts` |
| `cook-register` | Capture-mode Lua VM: `cook.*`, `fs.*`, `path.*`, module loader, `add_unit`/`add_test`/`dep_output`/`export` APIs; produces `RecipeUnits` | `cook-contracts`, `cook-lua-stdlib`, `mlua` |
| `cook-luaotp` | Worker pool (`WorkerPool`, `WorkItem`, `WorkResult`); each thread owns one Lua VM and executes Shell/Interactive/LuaChunk/Test payloads | `cook-contracts`, `cook-lua-stdlib`, `mlua` |
| `cook-cache` | `LocalBackend` (v3 filesystem CAS), `RecipeCache` on-disk format, `ThreadSafeCacheManager`, `CacheContext`, cloud backend, `.cook/cloud.toml`, `TestCache` | `cook-fingerprint`, `cook-contracts` |
| `cook-fingerprint` | Pure: `hash_file`/`hash_env`/`stat_mtime`, `EnvDenylist`, `ExecutionContext`, `MachineIdentity`, `compute_test_fingerprint`, `needs_rebuild_*`, `CacheBackend` trait, `CloudKey` derivation | `cook-contracts` |
| `cook-contracts` | `WorkPayload`, `CapturedUnit`, `CacheMeta`, `DepKind`, `StepKind`, `RecipeUnits`, `OutputStream`, `ACCESSORS` | (none) |
| `cook-dag` | Generic `Dag<T>` with `add_node`, atomic `complete`, `initial_ready`, cycle detection via Kahn's algorithm | (none) |
| `cook-progress` | `ProgressEvent` ingestion, `BuildState` pure state machine, `InlineRenderer`/`PlainRenderer`/`JsonWriter`, `LogStore` (`.cook/logs/`) | (none) |
| `cook-lua-stdlib` | `fs.*`, `path.*`, `cook.platform.*`, `sandbox`, `shell_guard` — shared by register + workers | `mlua` |

---

## Key Design Decisions

- **Two-phase execution (register then execute).** The Lua script is run once in capture mode (in `cook-register`); Cook API calls record work rather than performing it. The engine then executes the captured `WorkPayload`s through the worker pool. This is what makes parallel scheduling possible. → see [runtime.md](runtime.md)

- **Step groups for parallelism within recipes.** `cook.step_group(...)`-wrapped units can run in parallel with each other; units in different groups are barriered. This gives per-recipe parallelism without requiring explicit dependency declarations. → see [scheduler.md](scheduler.md)

- **Capture-mode API semantics.** During registration, Lua calls like `cook.sh()` and `cook.exec()` inside layer bodies record their arguments instead of performing the work. The body never runs against real I/O. → see [runtime.md](runtime.md)

- **Content-hash caching with mtime fast-path.** Cook stats file mtimes first (cheap); only if mtimes diverge does it re-hash contents. If the hash matches the cached value despite a touched mtime, the cache entry's mtime is refreshed and the rebuild is skipped. → see [cache.md](cache.md)

- **Interactive steps and chores drain the pool.** Steps marked interactive (with `@`), and chore windows, cannot share the process with other concurrent work. Before running one, the executor drains all in-flight work and runs it on the main thread. → see [scheduler.md](scheduler.md)

- **Dependencies are explicit, not ingredient-derived.** Cross-recipe edges come from explicit `requires` and from name-reference placeholders (`{lib}`, `{lib.accessor}`) resolved by `cook-luagen`. Path-string equality between an ingredient and another recipe's cook-output is *not* a dependency edge — see Cook Standard § 5.6 and rationale B.5.N. → see [supporting-modules.md](supporting-modules.md)

- **Workspace-aware imports.** A Cookfile may `import "path/to/sub" as alias` to mount another Cookfile under a namespace. Imports are resolved by `cook-engine::pipeline::workspace`; tree-relative paths are validated, sigil-anchored paths (`//path/from/root`) jump to the workspace root. → see [supporting-modules.md](supporting-modules.md)

- **Cloud cache is opt-in via `.cook/cloud.toml`.** When a project ID is configured, `cook-cache::CloudBackend` participates in the same `CacheBackend` trait surface as the local backend. Cache keys are derived from `project_id` + Cookfile path + `cache_key` + machine identity + post-denylist env contribution. → see [cache.md](cache.md)

---

## Reading Order

If you are new to the codebase, the following order lets each document build on the last:

1. **This file** — the map; read it first to orient yourself.
2. [**execution-flow.md**](execution-flow.md) — end-to-end trace of what happens when you run `cook build`.
3. [**parser.md**](parser.md) — the Cookfile lexer, parser, and AST (`cook-lang`); everything else consumes these types.
4. [**codegen.md**](codegen.md) — how the AST becomes Lua (`cook-luagen`); short but central to understanding what the runtime executes.
5. [**runtime.md**](runtime.md) — the register-phase Lua VM (`cook-register`) and the worker-pool Lua VMs (`cook-luaotp`); the two-phase model in detail.
6. [**scheduler.md**](scheduler.md) — recipe DAG, wave grouping, work-unit DAG, executor, interactive/chore handling.
7. [**cache.md**](cache.md) — incremental rebuild: what gets hashed, how the fast-path works, when cache entries are invalidated; local + cloud backends.
8. [**supporting-modules.md**](supporting-modules.md) — analyzer, watcher (`cook serve`), env resolution, workspace/imports, progress renderer.
9. [**dynamic-linking-and-rocks.md**](dynamic-linking-and-rocks.md) — how C-extension LuaRocks resolve symbols against the statically-linked Lua inside `cook`.
