# Parallel Recipe Execution Design

## Summary

Cook currently executes recipes sequentially in topological order — register, build DAG, execute, next recipe. This design introduces a recipe-level DAG that groups independent recipes into "waves." All recipes in a wave are registered together, their work units are flattened into a single `ExecutionDag`, and executed on the shared worker pool. This enables independent recipes (e.g., `backend` and `frontend` after `proto` completes) to run in parallel.

## Motivation

In monorepo use cases, many independent packages depend on shared libraries. Without parallel recipe execution, `cook run build` in a monorepo with 10 independent services runs them all sequentially even though they could parallelize. This is the natural follow-up to the import/monorepo feature.

## Scope

This design covers `cmd_run_workspace` in `engine/pipeline.rs`. Single-Cookfile `cmd_run` (which doesn't use `topological_sort`) is unaffected. The `cmd_test` pipeline also iterates sequentially over recipes — applying the wave model there is a natural follow-up but out of scope for this design.

## Design Decisions

### Wave-Based Execution Model

Instead of a concurrent coordinator or thread-per-recipe model, we use a batched wave approach. Each iteration pops all recipes whose dependencies are satisfied, registers them, flattens their work units into one DAG, and executes.

```
loop {
    ready = recipe_dag.pop_ready()
    if ready.is_empty() { break }

    units = ready.map(|r| register(r))
    dag = build_dag(units)
    execute_dag(dag)

    recipe_dag.mark_done(ready)
}
```

**Example: `proto → backend, frontend → deploy`**
- Wave 1: `proto` — register and execute alone
- Wave 2: `backend` + `frontend` — registered together, one combined DAG, work units interleave on shared pool
- Wave 3: `deploy` — register and execute alone

**Why waves instead of a concurrent coordinator:** Simpler, no need for a second layer of async coordination. Registration must wait for dependency execution anyway, so the wave boundaries are natural synchronization points. The parallelism comes from multiple recipes' work units sharing the pool within a wave.

### RecipeDag Structure

A lightweight DAG that tracks recipe-level dependencies and readiness. Lives in `scheduler/`.

```rust
pub struct RecipeDag {
    nodes: BTreeMap<String, RecipeDagNode>,
}

struct RecipeDagNode {
    deps: Vec<String>,          // recipes this one depends on
    remaining_deps: usize,      // decremented as deps complete
    in_flight: bool,            // true after pop_ready, before mark_done
    done: bool,
}
```

**API:**
- `RecipeDag::new(dep_edges: &BTreeMap<String, Vec<String>>) -> Self` — build from dependency edges for reachable recipes
- `pop_ready(&mut self) -> Vec<String>` — return all recipes with `remaining_deps == 0`, `!in_flight`, and `!done`; set `in_flight = true` on returned recipes
- `mark_done(&mut self, names: &[String])` — set `done = true`, `in_flight = false`, decrement `remaining_deps` on dependents

No atomics needed — this runs single-threaded on the coordinator (main thread). The parallelism is within `execute_dag`, not at this level.

### Exposing Dependency Edges from the Analyzer

Currently `topological_sort` returns `Vec<String>`. We add a new public function:

```rust
/// Return the dependency edges for all recipes reachable from `target`.
/// Keys are recipe names; values are the recipes they depend on.
pub fn dependency_edges(
    recipes: &HashMap<String, RecipeInfo>,
    target: &str,
) -> Result<BTreeMap<String, Vec<String>>, GraphError>
```

Implementation: extract the adjacency-building logic (serves map + requires + ingredient matching) into a shared helper used by both `topological_sort` and `dependency_edges`. The new function calls `topological_sort` first to get the reachable set, then filters the adjacency map to only those recipes. `RecipeDag::new` consumes the result directly. The existing `topological_sort` function remains unchanged.

### Registration Constraint

A recipe is only registered after all its dependency recipes have fully executed. This is because ingredient globs may reference outputs produced by dependency recipes. The wave model enforces this naturally — `pop_ready` only returns recipes whose deps are in the `done` set.

Registration of recipes within a wave is sequential (register all, then build one DAG, then execute). No concurrent registration.

### Shared Worker Pool

All recipes in a wave share the same worker pool (controlled by `-j`/`--jobs`). No per-recipe pools. The existing `execute_dag` already handles multiple `RecipeUnits` via `build_dag(vec![units...])`, which wires cross-recipe edges only when `RecipeUnits.deps` declares them. Within a wave, independent recipes have no cross-recipe edges — their work units are free to interleave.

### Cache Manager Scoping

Currently each recipe creates its own `ThreadSafeCacheManager` scoped to `rt.working_dir().join(".cook/cache")`. In the wave model, recipes from different imports have different working directories and different cache directories. Since `execute_dag` takes a single `Option<Arc<ThreadSafeCacheManager>>`, and each `RecipeUnits` already carries its own `working_dir`, we pass `None` for the shared cache manager parameter and instead create per-recipe cache managers during registration. Each recipe's `RecipeUnits` already stores `working_dir`, and the cache completion logic in the executor uses `working_dir` from the DAG node. The wave loop creates a cache manager per recipe and stashes them; after execution, each recipe's cache is flushed using its own manager. (This mirrors how the sequential loop already works — one cache manager per iteration.)

### Error Handling

**Execution failures:** If any recipe in a wave fails during `execute_dag`, the wave loop breaks — no subsequent waves execute. Recipes in the same wave that don't depend on the failed recipe may have already completed or may be mid-execution. This matches existing behavior for within-recipe failures: the pool drains, in-flight work finishes, but no new work is submitted.

**Registration failures:** If registration fails for one recipe in a wave (e.g., Lua error), the entire wave is aborted. Any already-registered recipes in the wave do not execute. This matches the current sequential behavior where a registration error returns immediately. The wave loop breaks with the registration error.

**Empty DAGs:** If all recipes in a wave produce empty DAGs (fully cached), `mark_done` is still called for all recipes in the wave so dependents are unblocked. The `build_dag` call returns an empty DAG, `execute_dag` is skipped (existing `dag.is_empty()` check), and the loop continues to the next wave. This also applies when only some recipes produce empty units — `mark_done` is called for the entire wave regardless.

### Recipe Name Namespacing

`register_recipe` sets `RecipeUnits.recipe_name` to the local name (e.g., `"build"`), but the pipeline uses namespaced names (e.g., `"backend.build"`). After calling `register_recipe`, the wave loop must rewrite `units.recipe_name` to the namespaced name. This ensures:

- `RecipeStarted` events emitted by `execute_dag` match the `RecipeQueued` names
- `OutputLine` events carry the namespaced recipe name for correct renderer association
- `RecipeCompleted`/`RecipeFailed` events match their `RecipeQueued` counterparts

This is a pre-existing inconsistency in the sequential code that becomes a correctness issue with parallel recipes: two imports could both have a local `"build"` recipe, and `build_dag` uses `recipe_leaves.insert(ru.recipe_name, ...)` — duplicate local names would collide, silently corrupting cross-recipe dependency edges. The fix is a single line: `units.recipe_name = name.clone()` after registration. This fix is **required for correctness**, not just event consistency.

### Cross-Recipe Edge Behavior in build_dag

`build_dag` wires cross-recipe edges by looking up `RecipeUnits.deps` entries in its `recipe_leaves` map. Within a wave, recipes from prior waves are not present in the current `build_dag` call, so their dep entries silently resolve to no edges. This is correct and intentional: prior-wave recipes have already completed execution, so no DAG dependency is needed. Independent recipes within the same wave have no entries in each other's `deps`, so they get no cross-recipe edges and are free to interleave.

### No Changes Required

- **Worker pool** (`scheduler/pool.rs`) — unchanged, already handles concurrent work from multiple recipes
- **Executor** (`scheduler/executor.rs`) — unchanged, already processes a multi-recipe `ExecutionDag`
- **DAG builder** (`scheduler/builder.rs`) — unchanged, already accepts `Vec<RecipeUnits>` and handles missing dep entries in `recipe_leaves` gracefully
- **Renderers** (`TtyRenderer`, `PlainRenderer`) — unchanged, already track per-recipe state with `BTreeMap<String, RecipeRenderState>` and handle events from concurrent work units
- **Cache manager** — unchanged, already thread-safe
- **Registration** (`runtime/engine.rs`) — unchanged

### What Changes

1. **New: `RecipeDag`** in `scheduler/recipe_dag.rs` — the lightweight recipe-level DAG structure described above
2. **New: `dependency_edges`** in `analyzer/graph.rs` — exposes the adjacency info already computed inside `topological_sort`
3. **Modified: `cmd_run_workspace` in `engine/pipeline.rs`** — replace the sequential `for name in &order` loop with the wave loop, rewrite `recipe_name` to namespaced form after registration

## Architecture

```
analyzer::graph::dependency_edges → BTreeMap<String, Vec<String>>
                ↓
        scheduler::RecipeDag
                ↓
    ┌── wave loop (pipeline.rs) ──┐
    │                              │
    │  pop_ready() → [recipe_a,   │
    │                  recipe_b]   │
    │       ↓                      │
    │  register each → Vec<Units>  │
    │       ↓                      │
    │  build_dag(units) → DAG      │
    │       ↓                      │
    │  execute_dag(dag, pool)      │
    │       ↓                      │
    │  mark_done([a, b])           │
    │       ↓                      │
    │  loop back                   │
    └──────────────────────────────┘
```

## Testing

- **Unit tests for `RecipeDag`**: single recipe, linear chain, diamond dependency, independent recipes in same wave, empty wave (all cached)
- **Unit tests for `dependency_edges`**: reuses existing `topological_sort` test cases to verify edge output
- **Integration test**: monorepo with diamond pattern (`proto → backend, frontend → all`) — verify `backend` and `frontend` register and execute in the same wave
- **Existing tests**: all current tests pass unchanged — single-recipe execution is just a single wave with one recipe
