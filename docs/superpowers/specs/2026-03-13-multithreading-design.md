# Cook Multithreading Design

## Overview

Add fine-grained parallel execution to Cook using a DAG scheduler and a pool of Lua VM workers. Zero new dependencies — built entirely on `std::thread` and `std::sync`.

## Mental Model

Erlang-inspired "poor man's OTP": a pool of isolated Lua VMs on OS threads, pulling work units from a shared queue. No shared mutable state between workers. The scheduler owns the dependency graph and feeds ready work to the pool.

## Execution DAG

Today the analyzer produces a flat topological list of recipe names. The new model expands this into a fine-grained DAG of work units.

### DAG Construction (Two Phases)

**Phase 1 — Recipe registration (main thread):** Each recipe's generated Lua runs on a **single throwaway VM on the main thread** to register work, not execute it. This means the existing `Rc<RefCell<...>>` types (`SharedCacheState`, `RegisteredRecipe` list) remain unchanged during registration — no `Send`/`Sync` needed.

The recipe's full setup runs as it does today: load cache, resolve ingredient globs, record glob results, compute env hash, check recipe-level invalidation, build the `recipe` context table and set it as a Lua global. Then the recipe body function executes.

When Lua calls `cook.layer(inputs, output, hash, fn)`, instead of running `fn` immediately, the runtime:

- Runs the cache check (cheap, no I/O beyond stat/hash)
- If fresh → marks the node as "already satisfied" (no work needed, but its output is available as a dependency)
- If stale → intercepts the inner function by calling it in a **dry-run mode** where `cook.exec()` records the resolved command string instead of executing it. This captures the fully-expanded, concrete command (all Lua variables already resolved). A WorkUnit node is created in the DAG holding this captured payload.

For `cook.exec()` / `cook.sh()` calls **outside** of a layer (bare shell steps, plate steps), the same interception applies — the call records the command and creates a WorkUnit.

For `using >{ ... }` Lua blocks, the registration phase captures the block source code and the concrete values of all injected variables (`input`, `output`, `recipe.name`, `recipe.ingredients`, etc.) into a self-contained Lua string that can be executed on any VM without external dependencies.

`Step::Taste` steps are **skipped** during DAG construction (taste/REPL will be redesigned separately for the threaded model).

**Phase 2 — Edge wiring:** Dependencies between work units come from:

- **Explicit recipe deps** — all units in recipe B depend on all units in recipe A (if B depends on A)
- **Implicit within a recipe** — a cook step depends on the **immediately preceding** cook step's outputs (matching the codegen's `_cook_outputs_{n-1}` chaining). Bare shell/Lua steps maintain declaration order within their recipe.
- **Implicit cross-recipe** — if recipe B's ingredient matches recipe A's output (exact string match, same semantics as today's `serves_map`), B's units that consume that file depend on A's unit that produces it.

Cached (fresh) nodes are pre-satisfied — their dependents don't wait for them.

## Work Unit

```rust
enum WorkPayload {
    Shell { cmd: String, line: usize },
    LuaBlock { code: String },
}

struct WorkUnit {
    id: usize,
    payload: WorkPayload,
    recipe_name: String,         // For output prefixing, cache writes, and error reporting
    input_paths: Vec<String>,    // For cache entry update after completion
    output_path: Option<String>, // For cache entry update after completion
    command_hash: u64,           // For cache entry update after completion
    remaining_deps: AtomicUsize, // Count of unsatisfied dependencies
    dependents: Vec<usize>,      // WorkUnit IDs that depend on this one
}
```

The WorkUnit holds concrete, self-contained data — no Lua closures, no references to a specific VM's state. Any worker VM can execute any WorkUnit.

## Scheduler

The scheduler owns the DAG and coordinates execution. It runs on its own thread.

### Algorithm

1. Walk the DAG, find all nodes with `remaining_deps == 0` → push onto a ready queue
2. Workers pull from the ready queue, execute the payload on their VM
3. Worker sends back `(unit_id, Result)` on a completion channel
4. Scheduler receives completion:
   - **On success:** update cache entry for this unit. For each dependent, decrement `remaining_deps`. If it hits zero → push to ready queue.
   - **On failure:** mark the unit as failed. Propagate failure to all transitive dependents (mark them as cancelled). Continue running unrelated branches.
5. When all units are complete or failed → return results

### Failure Semantics

A failed unit cancels its downstream subgraph but does not kill unrelated work. This matches `make -k` behavior — do as much useful work as possible. The final exit reports all failures.

### Ready Queue

`Arc<Mutex<VecDeque<WorkUnit>>>` with a `Condvar` for workers to sleep on when empty. No lock-free structures needed — queue operations are nanoseconds vs millisecond+ shell commands.

### Cache Writes

One `Mutex<RecipeCache>` **per recipe** (matching the existing file-per-recipe storage in `.cook/cache/{recipe_name}.bin`). Multiple work units from the same recipe contend on their recipe's mutex but not on other recipes' caches. Cache writes happen on the scheduler thread after receiving completion (not on worker threads), keeping worker VMs free to pick up the next unit immediately.

## Worker Pool

### Startup

- Read `-j N` flag (default: `std::thread::available_parallelism()`)
- Spawn N OS threads, each creates its own `mlua::Lua` VM
- Each VM gets the standard Cook APIs registered (`cook.*`, `fs.*`, `path.*`) including working directory and environment variables
- Workers block on the ready queue condvar until work arrives

### Worker Loop

```
loop {
    unit = ready_queue.pop()        // blocks on condvar if empty
    if unit is poison pill → break  // shutdown signal

    match unit.payload {
        Shell { cmd, line } → run shell command via cook.exec()
        LuaBlock { code }  → load and execute code string in VM
    }
    send (unit.id, result) to scheduler channel
}
```

### VM Reuse

Workers reuse their Lua VM across work units. Each payload is self-contained so there is no state leakage concern. This avoids the cost of creating a new VM per unit.

### Shutdown

After the scheduler determines all work is done (or fatally failed), it pushes N poison pills onto the ready queue. Workers exit, threads join. The shutdown protocol is resilient to workers that have already terminated — extra poison pills are harmless.

### Output

Child process stdout/stderr is captured line-by-line and written atomically with a `[recipe_name]` prefix per line. This prevents interleaving at arbitrary byte boundaries across concurrent workers. Each line is a complete, prefixed unit.

### `-j 1` Mode

`-j 1` still uses the full scheduler and queue architecture (single worker thread). This ensures the parallel code path is always exercised, making `-j 1` a useful debugging mode.

## Changes to Existing Code

### Untouched

- **Parser, Lexer, AST** — Cookfile syntax does not change.

### Extended

- **Analyzer** — `resolve_execution_order()` still does the topo sort. New function expands recipe-level order into the fine-grained work unit DAG with dependency edges. Uses same exact-match semantics as today for implicit cross-recipe deps.

### Minor Changes

- **Codegen** — produces a list of Lua chunks per recipe (with dependency metadata) instead of one monolithic string.
- **CLI** — add `-j`/`--jobs` flag. Replace sequential recipe loop with "build DAG, launch pool, wait for completion." `cook serve` calls the new scheduler in its rebuild loop — no special integration needed.
- **Cache** — reads happen during registration on main thread (same as today, `Rc<RefCell<...>>` unchanged). Writes happen on scheduler thread after unit completion. Per-recipe `Mutex<RecipeCache>` for thread-safe write access.

### Significant Rework

- **Runtime** — `cook.layer()` changes from "execute now" to "register work unit + cache check + dry-run interception." `cook.exec()` / `cook.sh()` gain a dry-run mode for registration. New scheduler and worker pool modules. `--quiet` and `--no-taste` flags propagate to worker VMs via shared config.

## New File Structure

```
src/scheduler/
├── mod.rs          // DAG construction, scheduler loop
├── dag.rs          // WorkUnit, DependencyGraph structs, edge wiring
└── pool.rs         // Worker pool, VM creation, worker loop
```

## Modified Files

```
src/cli/mod.rs      // -j flag, replace sequential loop with scheduler
src/codegen/mod.rs  // Emit chunked Lua + dependency metadata
src/runtime/mod.rs  // Registration-phase cook.layer()/cook.exec()
src/runtime/api.rs  // API changes for registration vs execution mode
src/cache/mod.rs    // Thread-safe cache access
```

## Dependencies

None added. Built entirely on `std::thread`, `std::sync::{mpsc, Arc, Mutex, Condvar}`.

## Configuration

`-j N` / `--jobs N` flag. Defaults to `std::thread::available_parallelism()`.
