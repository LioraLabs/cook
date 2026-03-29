# Execution Flow: End-to-End Trace

This traces what happens when you run `cook build` from CLI to completion, following the data through every stage.

---

## 1. Overview

`cook build` runs a single top-level function call that fans out into parsing, environment setup, dependency analysis, and a per-recipe register-then-execute loop. The key insight is the **two-phase** approach for each recipe: the recipe body runs twice — once in _capture mode_ to discover what work needs to be done, and once more (in parallel via a DAG scheduler) to actually do it.

The full chain is:

```
main()
  └─ cli::run()
       └─ cmd_run()
            ├─ read_and_parse()         → Cookfile AST + Lua source
            ├─ resolve_env()            → merged HashMap<String, String>
            ├─ analyzer::resolve_execution_order()  → Vec<String> (recipe order)
            └─ for each recipe:
                 ├─ rt.register_recipe()    → RecipeUnits (Phase 1: capture)
                 ├─ scheduler::builder::build_dag()  → ExecutionDag
                 └─ scheduler::execute_dag()         (Phase 2: execute)
```

---

## 2. CLI Dispatch

**Entry point:** `src/main.rs:3`

```rust
fn main() {
    if let Err(e) = cli::run() { ... }
}
```

`main()` delegates immediately to `cli::run()` (`src/cli/mod.rs:94`). This is where clap parses `argv`:

```rust
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();   // line 95

    let result = match &cli.command {
        Some(Command::External(args)) => {
            let recipe = args.first().map(|s| s.as_str()).unwrap_or("build");
            let config = args.get(1).map(|s| s.as_str());
            cmd_run(&cli, recipe, config)            // e.g. `cook build`
        }
        None => cmd_run(&cli, "build", None),        // bare `cook` with no subcommand
        ...
    };
```

When you type `cook build`, clap sees `build` as an unknown subcommand and routes it to the `Command::External(args)` arm (line 104). When you type bare `cook`, the `None` arm fires (line 109), defaulting the recipe to `"build"`. Either way, `cmd_run(&cli, "build", None)` is called.

**Key CLI flags consumed here:**

| Flag | Field | Default |
|------|-------|---------|
| `-f / --file` | `cli.file` | `"Cookfile"` |
| `--emit-lua` | `cli.emit_lua` | `false` |
| `-q / --quiet` | `cli.quiet` | `false` |
| `--no-taste` | `cli.no_taste` | `false` |
| `-j / --jobs` | `cli.jobs` | CPU count |
| `--set KEY=VALUE` | `cli.set` | `[]` |

---

## 3. Read & Parse

**Function:** `read_and_parse()` — `src/cli/mod.rs:121`

```rust
fn read_and_parse(cli: &Cli) -> Result<(Cookfile, String), CookError> {
    let source = std::fs::read_to_string(&cli.file)...;   // line 122
    let cookfile = parser::parse(&source)...;              // line 126
    let lua_source = codegen::generate(&cookfile);         // line 128
    Ok((cookfile, lua_source))
}
```

Three things happen:

1. **Read from disk.** The `cli.file` path (default `"Cookfile"`) is read into a `String`. If the file is missing, the error surfaces here as `CookError::Other`.

2. **Parse to AST.** `parser::parse(&source)` runs the lexer then the parser, returning a `Cookfile` AST (`src/parser/ast.rs`). The AST records recipes, their ingredient glob patterns, explicit `requires` deps, cook steps, and bare variable assignments. Parse errors become `CookError::ParseError` and include the source location.

3. **Transpile to Lua.** `codegen::generate(&cookfile)` converts the AST to a Lua source string. Every recipe becomes a `cook.recipe("name", {ingredients=..., requires=...}, function() ... end)` call. Shell steps become `cook.exec(...)` calls; cook steps (with `begin_step`/`end_step` wrapping) become `cook.layer(...)` calls.

Back in `cmd_run()` (line 190), if `--emit-lua` was set, the Lua source is printed to stdout and the process exits immediately:

```rust
if cli.emit_lua {
    println!("{lua_source}");
    return Ok(());
}
```

**Data in:** path to Cookfile
**Data out:** `(Cookfile, String)` — the AST and transpiled Lua source

---

## 4. Environment Resolution

**Function:** `resolve_env()` — `src/cli/mod.rs:132`

Cook builds a single flat `HashMap<String, String>` by merging five layers in order. Later layers win.

```rust
fn resolve_env(
    cookfile: &Cookfile,
    selected_config: Option<&str>,
    dotenv_vars: HashMap<String, String>,
    cli_sets: &[String],
) -> Result<HashMap<String, String>, CookError> {
```

**Layer 1 — System environment** (line 139)
```rust
let mut env: HashMap<String, String> = std::env::vars().collect();
```
The starting point is the process's inherited environment. `PATH`, `HOME`, `CC`, anything the shell exported is already here.

**Layer 2 — Cookfile bare variables** (line 142)
```rust
for (k, v) in &cookfile.vars {
    env.insert(k.clone(), v.clone());
}
```
Variables declared at the top of the Cookfile (e.g., `CC "clang"`) are inserted, overriding system env.

**Layer 3 — Selected config block** (line 146)
```rust
if let Some(config_name) = selected_config {
    let config_vars = cookfile.configs.get(config_name).ok_or_else(|| ...)?;
    for (k, v) in config_vars { env.insert(k.clone(), v.clone()); }
}
```
If a config name was passed as a positional CLI argument (e.g., `cook build release`), the variables from that named config block are overlaid. Unknown config names produce a helpful error listing available configs.

**Layer 4 — `.env` file** (line 167)
```rust
for (k, v) in dotenv_vars { env.insert(k, v); }
```
`load_env(cookfile_dir)` is called at line 201 of `cmd_run()` before `resolve_env()` is called. It reads a `.env` file from the Cookfile's directory (if present) and returns a map. Those values are passed in as `dotenv_vars` and applied here.

**Layer 5 — `--set` CLI overrides** (line 172)
```rust
for set_arg in cli_sets {
    if let Some(eq_pos) = set_arg.find('=') {
        env.insert(set_arg[..eq_pos].to_string(), set_arg[eq_pos+1..].to_string());
    }
}
```
`--set KEY=VALUE` flags have the final word. The string is split on the first `=`, so values may contain `=` characters.

**Data in:** `Cookfile` AST, optional config name, dotenv map, `--set` strings
**Data out:** `HashMap<String, String>` — the fully-merged environment

This map is passed to `Runtime::new()` (line 215) and also forwarded to `execute_dag()` (line 264) so worker processes inherit it.

---

## 5. Dependency Analysis

**Function:** `analyzer::resolve_execution_order()` — `src/cli/mod.rs:204`, implemented in `src/analyzer/mod.rs:35`

```rust
let order = analyzer::resolve_execution_order(&cookfile, recipe_name)
    .map_err(|e| match e {
        GraphError::UnknownRecipe(name) => CookError::RecipeNotFound(name),
        GraphError::CycleDetected(name) => CookError::Other(format!("dependency cycle involving: {name}")),
    })?;
```

`resolve_execution_order` calls `build_recipe_info()` (`src/analyzer/mod.rs:7`) to extract a `HashMap<String, RecipeInfo>` from the AST, then calls `topological_sort()` (`src/analyzer/graph.rs:20`).

**Two kinds of dependency edges:**

1. **Explicit deps** (`requires` in the recipe header). The recipe declares `requires = {"clean"}` and the graph adds a direct edge. Validated at graph-build time: referencing a non-existent recipe is an immediate error.

2. **Implicit deps** (ingredient-serves matching). If recipe A lists `"lib.a"` in its `ingredients` list, and recipe B lists `"lib.a"` in its output (`serves`) list, the analyzer infers an edge A → B. This is an exact string match — glob patterns in `ingredients` do _not_ trigger implicit deps (`src/analyzer/graph.rs:47`).

The sort is a DFS post-order traversal starting at the target recipe. Only recipes reachable from the target are included — unrelated recipes in the Cookfile are ignored. Cycle detection is handled by tracking `Visiting` / `Visited` node states.

**Data in:** `Cookfile` AST, target recipe name
**Data out:** `Vec<String>` — recipe names in execution order (dependencies first)

Example: for `cook build` where `build` requires `compile` and `compile` requires nothing, the result is `["compile", "build"]`.

---

## 6. Per-Recipe Execution Loop

`cmd_run()` iterates over the ordered recipe names (`src/cli/mod.rs:232`):

```rust
for name in &order {
    // Phase 1: Register
    let units = rt.register_recipe(&lua_source, name)?;   // line 238

    // Build DAG for this recipe
    let dag = crate::scheduler::builder::build_dag(vec![units]);  // line 252

    if dag.is_empty() { continue; }

    // Phase 2: Execute
    crate::scheduler::execute_dag(
        dag, num_jobs, cookfile_dir.to_path_buf(),
        env_vars.clone(), cli.quiet, Some(cache_manager.clone()),
    )?;  // line 259
}
```

Recipes run _sequentially_ (one at a time). Within each recipe, work units run in parallel via the DAG. This ordering ensures cross-recipe file dependencies are respected before the next recipe scans its ingredients.

### Phase 1 — Register

**Function:** `Runtime::register_recipe()` — `src/runtime/mod.rs:239`

A fresh Lua VM is created. The Cook API is registered in **capture mode**: `cook.exec()` is a no-op that records a `CapturedUnit` instead of executing anything; `cook.layer()` similarly records the work unit and checks the cache to decide whether the step is already satisfied.

```rust
let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));
let recipes = register_cook_api_capture(&lua, &self.env_vars, &self.working_dir, capture_state.clone())?;
```

The generated Lua source is loaded into the VM (`lua.load(lua_source).exec()` at line 267), registering all recipe functions. Then `setup_recipe_context()` (line 276) runs cache invalidation and resolves ingredient globs into the `recipe.ingredients` table. Finally the recipe function is called (line 280).

As the recipe body runs, each `cook.exec()` / `cook.layer()` call appends to `capture_state.units`. `cook.begin_step()` / `cook.end_step()` bracket groups of parallelisable units into a `step_groups` entry.

**What comes back** is a `RecipeUnits` struct (line 291):

```rust
RecipeUnits {
    recipe_name: String,
    deps: Vec<String>,           // explicit requires
    units: Vec<CapturedUnit>,    // one per work item
    step_groups: Vec<Vec<usize>>,// indices of parallelisable groups
}
```

Each `CapturedUnit` carries:
- `payload: WorkPayload` — either `Shell { cmd, line }`, `Interactive { cmd, line }`, or `LuaChunk { ... }`
- `dep_kind: DepKind` — `Sequential` (depends on previous barrier) or `StepGroup(idx)` (parallel with group peers)
- `cache_meta: Option<CacheMeta>` — present when the step needs to run; `None` when pre-satisfied by cache

### Phase 2 — Build and Execute the DAG

**Build:** `scheduler::builder::build_dag()` — `src/scheduler/builder.rs:18`

`build_dag()` converts `RecipeUnits` into an `ExecutionDag`. It maintains a _barrier_: the set of DAG node IDs that a new sequential unit must wait for. `Sequential` units extend the barrier one node at a time. `StepGroup` units all share the same barrier entry point and together become the new barrier when the last group member is processed.

Cache-satisfied units (`CapturedUnit` with empty command and no `cache_meta`) are added as **presatisfied nodes** (no payload). The scheduler resolves them immediately without dispatching work.

**Execute:** `scheduler::execute_dag()` — `src/scheduler/mod.rs:82`

```rust
pub fn execute_dag(
    dag: ExecutionDag,
    num_workers: usize,
    working_dir: PathBuf,
    env_vars: HashMap<String, String>,
    _quiet: bool,
    cache_manager: Option<Arc<ThreadSafeCacheManager>>,
) -> Result<(), SchedulerError>
```

Execution proceeds as follows:

1. **Seed.** `dag.initial_ready()` (line 161) returns all nodes with zero remaining deps. Each is passed to `process_ready()`.

2. **Dispatch.** `process_ready()` handles three cases (line 117):
   - `None` payload (presatisfied): mark done, immediately cascade to dependents.
   - `WorkPayload::Interactive`: push onto `interactive_queue` for main-thread execution.
   - Any other payload: submit to `WorkerPool` as a `WorkItem`.

3. **Worker pool.** `WorkerPool::spawn()` starts `num_workers` threads (default: CPU count). Each worker receives `WorkItem`s over a channel, runs the command as a subprocess, and sends a `WorkResult` back.

4. **Main loop.** The main thread waits on the result channel. When a result arrives, if it succeeded, `dag.complete(id)` decrements `remaining_deps` on all dependents and returns the newly-unblocked ones for immediate dispatch. If it failed, `cancel_subtree()` marks all transitive dependents as cancelled.

5. **Interactive queue.** Whenever `pending == 0` (pool drained) and `interactive_queue` is non-empty, the main thread runs the queued interactive command directly with stdin attached (line 169). This is how `@`-prefixed steps work — they require a TTY and cannot run in worker threads.

6. **Cache update.** After each successful node, if the node carries `CacheMeta`, `cache_manager.update_step()` is called (line 243). At the end of the DAG, `cm.flush_all()` writes the cache to disk (line 311).

7. **Termination.** The loop exits when `pending == 0` and `interactive_queue` is empty. `pool.shutdown()` is called to join worker threads.

**Data in:** `ExecutionDag`, worker count, working dir, env vars, optional cache manager
**Data out:** `Ok(())` or `Err(SchedulerError)` listing each failed node as `(node_id, recipe_name, message)`

---

## 7. Output and Error Handling

### What the user sees on success

Cook prints a short status line per recipe before registration:

```
cook: registering recipe 'build'   ← printed to stderr (line 234)
```

Workers stream their subprocess output via `SharedWriter` (`src/scheduler/output.rs`). On success, `cmd_run()` returns `Ok(())` and `run()` returns normally, exiting with code 0.

### What the user sees on failure

Errors are handled inside `cli::run()` (line 112) — it does **not** return `Err` to `main()`. Instead, it prints the error and calls `process::exit()` directly:

```rust
match result {
    Ok(()) => Ok(()),
    Err(e) => {
        eprintln!("cook: {e}");
        std::process::exit(e.exit_code());
    }
}
```

(`main.rs` has a fallback error handler, but it never fires during normal operation.)

Exit codes (`CookError::exit_code()`, line 73):

| Error | Exit code | Example message |
|-------|-----------|-----------------|
| `CommandFailed` | 1 | `Cookfile:42: command failed (exit 1): gcc main.c` |
| `ParseError` | 2 | `cook: parse error: unexpected token at line 5` |
| `RecipeNotFound` | 3 | `cook: recipe not found: myrecipe` |
| `Other` | 1 | `cook: dependency cycle involving: a` |

### Cookfile line numbers in error messages

When a shell command fails inside a recipe, the error carries the Cookfile source line. This is threaded through as a `COOK_CMD_FAILED:<line>:<code>:<cmd>` sentinel string in the Lua error message, then decoded in two places:
- `cmd_run()` (line 268) for errors from Phase 1 (capture mode)
- `cmd_run()` (line 269) for errors from Phase 2 (scheduler results)

The sentinel format means even errors that cross the Lua/Rust boundary or the thread-pool channel still carry the original source location.
