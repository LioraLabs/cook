# Cook Monorepo DDD Rewrite Design

**Date:** 2026-03-20
**Status:** Approved (revised after DDD review)

## Overview

Restructure the Cook project from a single-repo monolithic Rust binary into a proper monorepo at `~/dev/cook/` with strict Domain-Driven Design. The Cook CLI's Rust code is split into single-purpose crates following the Unix philosophy: each crate does one thing extremely well.

The monorepo cleanly separates the CLI tool, marketing site, example projects, and future ecosystem projects (cloud infrastructure, SSH servers, etc.).

## Monorepo Top-Level Layout

```
~/dev/cook/
├── cli/                    # The Cook CLI tool (Rust workspace)
│   ├── crates/
│   │   ├── cook-cli/       # Binary entry, CLI args, command dispatch, progress UI
│   │   ├── cook-lang/      # Cookfile lexer, parser, AST
│   │   ├── cook-luagen/    # AST → Lua code (targeting step_group/add_unit API)
│   │   ├── cook-register/  # Capture-mode Lua VM, Cook API bindings, produces RecipeUnits
│   │   ├── cook-dag/       # Generic DAG data structure + topological traversal
│   │   ├── cook-luaotp/    # Worker pool of Lua VMs, executes work items
│   │   ├── cook-cache/     # Hash computation, cache storage, hit/miss logic
│   │   ├── cook-engine/    # Orchestrator: recipe DAG, wires register→dag→luaotp→cache
│   │   ├── cook-contracts/ # Shared types — behavior-free structs only
│   │   └── cook-progress/  # Terminal progress rendering primitives
│   ├── tests/              # Integration tests
│   ├── Cargo.toml          # Workspace manifest
│   └── Cargo.lock
├── examples/               # Example Cookfile projects (C, C++, monorepo)
├── marketing/              # Landing page (Vite/JS)
├── docs/                   # Architecture docs, specs, plans
└── CLAUDE.md
```

Future ecosystem projects (e.g., `cook-cloud/`, `cook-ssh/`) will be added as top-level siblings to `cli/`.

## Crate Responsibilities & Dependency Graph

Each crate has a single, well-defined purpose:

| Crate | Single Purpose | Depends On |
|-------|---------------|------------|
| **cook-contracts** | Behavior-free shared types: `WorkPayload`, `CacheMeta`, `CapturedUnit`, `DepKind`, `RecipeUnits` | nothing |
| **cook-lang** | Cookfile text → AST (lexer, parser, AST types) | nothing |
| **cook-luagen** | AST → Lua code targeting `step_group`/`add_unit` API | cook-lang (for AST types) |
| **cook-cache** | Hash computation (`hash_str`), file-based cache storage, hit/miss checks, glob resolution | cook-contracts |
| **cook-dag** | Generic DAG data structure, cycle detection, topological ordering, wave-based traversal | nothing (fully generic, no executor) |
| **cook-luaotp** | Worker pool of Lua VMs, supervised chunk/shell/test execution | cook-contracts |
| **cook-register** | Capture-mode Lua VM, binds Cook APIs, runs generated Lua, produces `RecipeUnits` (all units, no cache filtering) | cook-contracts |
| **cook-engine** | Orchestrator: recipe DAG, calls register per wave, evaluates cache, builds work-unit DAGs, feeds to luaotp, manages export/import state, error aggregation, cancellation | cook-contracts, cook-register, cook-dag, cook-luaotp, cook-cache |
| **cook-progress** | Terminal progress rendering primitives | nothing |
| **cook-cli** | Binary entry point, clap args, command dispatch, progress UI, env resolution, file watcher, test output formatting | cook-engine, cook-lang, cook-luagen, cook-progress, cook-contracts |

Dependency flow:

```
cook-cli → cook-engine → cook-register → cook-contracts
         → cook-lang      → cook-cache  → cook-contracts
         → cook-luagen     → cook-dag
         → cook-progress   → cook-luaotp → cook-contracts
         → cook-contracts
```

Key constraints:
- No crate depends on cook-cli or cook-engine except cook-cli. Everything below engine is a reusable library.
- cook-register has NO dependency on cook-cache. Registration captures all declared work. Cache evaluation is cook-engine's responsibility.
- cook-contracts contains ONLY behavior-free structs. No utility functions, no aggregation logic, no `Rc<RefCell>` types.
- All serialized collections use `BTreeMap`/`BTreeSet` for deterministic output (including `env_vars`).

## DDD Bounded Contexts

Each crate represents a bounded context with clear domain boundaries:

| Bounded Context | Domain | Owns |
|----------------|--------|------|
| **Language** (cook-lang) | Cookfile syntax and structure | Lexer, parser, AST types |
| **Code Generation** (cook-luagen) | AST-to-Lua transformation | Template expansion, Lua emission |
| **Registration** (cook-register) | Work unit discovery via Lua execution | CaptureState, Cook Lua API bindings, module loading |
| **Caching** (cook-cache) | Build artifact caching and invalidation | Cache storage, hash computation, staleness checks |
| **Scheduling** (cook-dag) | Dependency graph traversal | DAG structure, topological ordering, cycle detection |
| **Execution** (cook-luaotp) | Parallel Lua VM work dispatch | Worker pool, VM lifecycle, shell/chunk/test execution |
| **Orchestration** (cook-engine) | End-to-end build pipeline | Recipe DAG, wave scheduling, cache evaluation, export/import state, execution orchestration |
| **Presentation** (cook-cli) | User interface | CLI args, progress rendering, error formatting, test output display |
| **Shared Kernel** (cook-contracts) | Cross-context type agreements | Minimal behavior-free structs |

### Aggregate Ownership

Types live in the crate that owns their domain:

- `CaptureState`, `SharedCaptureState` → **cook-register** (uses `Rc<RefCell>`, registration-internal)
- `hash_str`, `resolve_glob` → **cook-cache** (utility functions for the caching domain)
- `TestResults`, `TestCaseResult`, `TestSuiteResult`, `TestStatus` → **cook-cli** (presentation/reporting logic)
- `ExportStore` → **cook-engine** (cross-recipe state managed by the orchestrator)
- `RecipeDag` → **cook-engine** (recipe-level orchestration)

### Error Handling Across Boundaries

Each crate defines its own error enum scoped to its domain:

- `cook-lang` → `ParseError`, `LexError`
- `cook-register` → `RegisterError`
- `cook-cache` → `CacheError`
- `cook-dag` → `CycleError`
- `cook-luaotp` → `ExecutionError`
- `cook-engine` → `EngineError` (wraps downstream errors as variants)
- `cook-cli` → formats `EngineError` into user-facing messages

Errors flow inward-to-outward: domain crates define specific errors, cook-engine wraps them, cook-cli renders them.

## Architecture: The Dual-DAG Execution Model

Cook has a two-layer DAG execution model:

### Layer 1: Recipe DAG (driven by cook-engine)

Operates at the recipe level. Recipes form a dependency graph (e.g., recipe "app" depends on recipe "lib"). Cook-engine processes recipes in waves — recipes whose dependencies are all satisfied can be processed in parallel.

### Layer 2: Work Unit DAG (structure from cook-dag, execution driven by cook-engine via cook-luaotp)

Within each wave of recipes, individual work units (compile, link, test) form a DAG. Step groups define parallelization boundaries — units within a step group run in parallel, sequential units form barriers.

### Orchestration Flow (cook-engine)

```
1. recipe_dag.pop_ready()             → wave of recipe names
2. For each ready recipe:
   registry.register_recipe()         → RecipeUnits (all units, uncached)
3. cache.evaluate(units)              → mark pre-satisfied units
4. build_dag(wave_units)              → Dag<WorkPayload>
5. Execute DAG: walk topology, feed ready nodes to luaotp
6. On completion: cache.record(unit)  → update cache entries
7. recipe_dag.mark_done(&wave)
8. Repeat until empty
```

Registration is deferred — a recipe's Lua is only executed once its dependencies are satisfied. This is critical because registration can depend on outputs from upstream recipes (e.g., `cook.import()` resolving transitive dependencies).

### Export/Import State Management

`cook.export(name, table)` and `cook.import(name)` enable cross-recipe data sharing (e.g., cpp.lua's transitive dependency resolution). The `ExportStore` (a `BTreeMap<String, LuaValue>`) is owned by **cook-engine**, which passes it to cook-register for each `register_recipe` call. This allows recipe "app" to import data exported by recipe "lib" in a previous wave.

## Key Design: Lua API Unification

The current codebase has two parallel APIs for registering work:

1. **Codegen API** (transpiled Cookfiles): `cook.begin_step()` / `cook.end_step()` / `cook.layer()` / `cook.exec()`
2. **Module API** (hand-written Lua like cpp.lua): `cook.step_group(fn)` / `cook.add_unit({...})` / `cook.export()` / `cook.import()`

The module API is strictly better — closure-based grouping, explicit inputs/outputs, no separate exec call. In the new design, **cook-luagen targets the module API exclusively**. The codegen API is retired.

### Codegen Output Examples

What cook-luagen emits for each Cookfile construct:

**A `cook` step (one-to-one file transformation):**

```
cook "build/obj/{stem}.o" using "{CC} -c {in} -o {out}"
```

Becomes:

```lua
cook.step_group(function()
    for _, _in in ipairs(recipe.ingredients[1]) do
        local _stem = path.stem(_in)
        local _out = "build/obj/" .. _stem .. ".o"
        cook.add_unit({
            inputs = { _in },
            output = _out,
            command = cook.env["CC"] .. " -c " .. _in .. " -o " .. _out,
        })
    end
end)
```

**A `cook` step (many-to-one):**

```
cook "build/libmath.a" using "{AR} rcs {out} {all}"
```

Becomes:

```lua
cook.add_unit({
    inputs = _prev_outputs,
    output = "build/libmath.a",
    command = cook.env["AR"] .. " rcs build/libmath.a " .. table.concat(_prev_outputs, " "),
})
```

**A `plate` step:**

```
plate "{CC} -o build/app {all}"
```

Becomes:

```lua
cook.add_unit({
    inputs = _prev_outputs,
    output = nil,
    command = cook.env["CC"] .. " -o build/app " .. table.concat(_prev_outputs, " "),
})
```

**A `test` step:**

```
test "./{out}" timeout 30
```

Becomes:

```lua
cook.step_group(function()
    for _, _out in ipairs(_prev_outputs) do
        cook.add_test({
            command = "./" .. _out,
            timeout = 30,
            should_fail = false,
        })
    end
end)
```

**A raw shell line:**

```
rm -rf build .cook
```

Becomes:

```lua
cook.add_unit({
    command = "rm -rf build .cook",
    cache = false,
})
```

**An inline Lua block:**

```
>{
    cpp.static_library("mathlib", { ... })
}
```

Becomes (passed through verbatim):

```lua
cpp.static_library("mathlib", { ... })
```

## cook-dag: Pure Data Structure

cook-dag provides the DAG data structure and topological traversal only. It does NOT include an executor — execution orchestration is cook-engine's responsibility.

```rust
pub struct Node<T> {
    pub id: usize,
    pub payload: T,
    pub dependents: Vec<usize>,
    pub remaining_deps: usize,
}

pub struct Dag<T> {
    nodes: Vec<Node<T>>,
}

impl<T> Dag<T> {
    pub fn new() -> Self;
    pub fn add_node(&mut self, payload: T, depends_on: &[usize]) -> usize;
    pub fn validate(&self) -> Result<(), CycleError>;
    pub fn initial_ready(&self) -> Vec<usize>;
    pub fn complete(&self, id: usize) -> Vec<usize>;  // returns newly ready
    pub fn node(&self, id: usize) -> &Node<T>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

cook-engine uses `Dag<WorkPayload>` for work-unit scheduling and its own `RecipeDag` (a simpler structure) for recipe-level wave scheduling.

## cook-register API Surface

Pure capture runtime. Produces ALL declared work units without cache filtering.

```rust
pub struct Registry {
    working_dir: PathBuf,
    env_vars: BTreeMap<String, String>,
}

impl Registry {
    pub fn new(working_dir: PathBuf, env_vars: BTreeMap<String, String>) -> Self;

    /// Execute generated Lua and capture all registered work units.
    /// export_store is owned by cook-engine and passed in for cross-recipe data sharing.
    pub fn register_recipe(
        &self,
        lua_source: &str,
        recipe_name: &str,
        export_store: &mut ExportStore,
    ) -> Result<RecipeUnits, RegisterError>;
}
```

Lua APIs bound by cook-register:

| Lua Function | Purpose |
|---|---|
| `cook.recipe(name, opts, fn)` | Declare a recipe |
| `cook.step_group(fn)` | Group units for parallel execution |
| `cook.add_unit({inputs, output, command})` | Register a work unit |
| `cook.add_test({command, timeout, should_fail})` | Register a test unit |
| `cook.export(name, table)` / `cook.import(name)` | Cross-recipe data sharing (backed by engine-owned ExportStore) |
| `cook.exec(cmd)` / `cook.sh(cmd)` | Shell execution during registration |
| `cook.env` | Environment variable table |
| `cook.cache.get(key)` / `cook.cache.set(key, val)` | Persistent key-value cache (registration-time only, for module use like compiler detection) |
| `cook.platform` | Platform info (os, arch) |
| `fs.*` | File system operations (glob, exists, read, write, mkdir_p) |
| `path.*` | Path utilities (stem, name, ext, dir) |

Note: `cook.cache` here is the module-level key-value cache used by Lua modules (e.g., cpp.lua caching compiler detection). This is distinct from the build artifact cache in cook-cache. This key-value cache is simple enough to live in cook-register.

## cook-engine Interface

```rust
pub struct Engine {
    num_workers: usize,
}

impl Engine {
    /// Single Cookfile execution
    pub fn run(
        &self,
        lua_source: &str,
        recipe_order: &[String],
        working_dir: PathBuf,
        env_vars: BTreeMap<String, String>,
        on_event: impl Fn(EngineEvent),
    ) -> Result<(), EngineError>;

    /// Workspace execution (multiple Cookfiles, recipe-level DAG)
    pub fn run_workspace(
        &self,
        registries: BTreeMap<String, Registry>,
        recipe_dag: RecipeDag,
        on_event: impl Fn(EngineEvent),
    ) -> Result<(), EngineError>;
}
```

cook-engine owns:
- Recipe DAG construction and wave scheduling (RecipeDag)
- ExportStore — cross-recipe data sharing state
- Cache evaluation — asking cook-cache "is this unit stale?" after registration
- Wiring register → dag → luaotp
- Execution orchestration — walking the work-unit DAG, feeding ready nodes to luaotp, handling completion/failure/cancellation
- Cache recording — updating cook-cache after successful execution
- Graph analysis — recipe dependency resolution, topological sorting, namespace resolution (current `analyzer` module)
- Error aggregation

cook-engine does NOT own:
- CLI args, progress rendering (cook-cli + cook-progress)
- Cookfile parsing or codegen (cook-cli calls cook-lang + cook-luagen)
- Environment resolution (cook-cli)
- Workspace discovery and loading (cook-cli)
- Test output formatting (cook-cli)

## cook-cli Responsibilities

The thin user-facing shell:

- clap argument parsing (run, test, init, menu, serve)
- Cookfile discovery and workspace resolution
- Environment resolution (system env → Cookfile vars → config blocks → .env → CLI --set)
- Pipeline glue: calls cook-lang → cook-luagen → passes Lua to cook-engine
- Progress UI: spawns renderer thread, wires EngineEvent → cook-progress
- File watcher: `cook serve` mode (notify crate)
- Color config: terminal detection, NO_COLOR, --color flag
- Test output: TestResults type, JUnit XML writing, terminal summary formatting
- Error formatting: converting EngineError into user-facing messages

## Migration Strategy

Bottom-up extraction, one crate at a time. Each phase produces compiling, passing code.

1. **Phase 0: Scaffold** — Create `~/dev/cook/` monorepo, set up workspace in `cli/`, move `marketing/`, `examples/`, `docs/`
2. **Phase 1: Leaves first** — Extract `cook-contracts` (behavior-free structs only), `cook-lang`, `cook-progress` (no internal dependencies)
3. **Phase 2: Pure transforms** — Extract `cook-luagen` (depends on cook-lang), `cook-cache` (depends on cook-contracts, absorbs `hash_str` and `resolve_glob`), `cook-dag` (depends on nothing, data structure only)
4. **Phase 3: Lua runtimes** — Extract `cook-luaotp` (worker pool + Lua VMs), then `cook-register` (capture-mode VM + Cook API bindings, absorbs `CaptureState`). Unify Lua API: retire `begin_step`/`end_step`/`layer`/`exec`, codegen targets `step_group`/`add_unit` exclusively. Remove cook-register → cook-cache dependency: registration captures all units without cache filtering.
5. **Phase 4: Orchestration** — Extract `cook-engine` (absorbs recipe DAG, analyzer/graph logic, export store, cache evaluation, execution orchestration)
6. **Phase 5: CLI shell** — What remains becomes `cook-cli` (absorbs TestResults types and formatting, workspace loading, env resolution)

Git strategy: fresh repo at `~/dev/cook/`, not a fork. Clean start.

## Performance

No performance concerns with the crate split. At these boundaries (parsing → codegen → execution), we pass owned data structures between phases. Rust monomorphizes and inlines across crate boundaries within the same workspace — the cost is zero at runtime.
