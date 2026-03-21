# Unified Engine Entry Point Design

**Date:** 2026-03-20
**Status:** Approved

## Problem

The current cook-cli has two separate execution paths for single-Cookfile and workspace builds. The single-Cookfile path executes recipes sequentially (one at a time), missing parallelization opportunities. It also reimplements topological sort locally instead of delegating to cook-engine, violating the DDD boundary.

## Solution

cook-engine exposes a single `run()` function. Both single-Cookfile and workspace builds call it. cook-cli converts its data and hands off. The engine owns the entire pipeline: dependency resolution, wave scheduling, registration, cache evaluation, DAG building, and execution.

## cook-engine Public API

```rust
pub fn run(
    recipe_infos: BTreeMap<String, RecipeInfo>,
    targets: &[String],
    registries: BTreeMap<String, (Registry, String)>,
    num_jobs: usize,
    on_event: impl Fn(EngineEvent),
) -> Result<RunResult, EngineError>
```

- `recipe_infos` — all recipes with fully-namespaced names and resolved deps
- `targets` — one or more recipes to execute (cmd_run: user-specified, cmd_test: discovered test recipes)
- `registries` — prefix → (Registry, lua_source), one per Cookfile
- `RunResult` — includes test outputs collected during execution

## Inside engine::run

1. `analyzer::dependency_edges(recipe_infos, targets)` → recipe dependency graph for all targets
2. `RecipeDag::new(edges)` → wave scheduler
3. Wave loop: `pop_ready` → register each recipe → cache eval → `build_dag` → execute → `mark_done`
4. Return `RunResult` with any test outputs

Both single-Cookfile and workspace builds follow the same wave-based execution. Independent recipes run in parallel.

## cook-cli Responsibilities

1. Parse Cookfile(s) via cook-lang, generate Lua via cook-luagen
2. Build `RecipeInfo` map: single Cookfile → direct extraction, workspace → `workspace_to_layout` + `analyzer::build_workspace_recipe_info`
3. Build registries: single Cookfile → one registry with empty prefix, workspace → one per import
4. Determine targets: `cmd_run` → user recipe, `cmd_test` → scan AST for test steps
5. Call `cook_engine::run()`
6. Format results: progress rendering, test output, error messages

## What Gets Deleted from cook-cli

- `resolve_execution_order` — engine does this
- `topological_sort_recipe_infos` — engine does this
- Local `RecipeInfo` struct — use engine's
- `cmd_run_workspace` as a separate function — merged into `cmd_run`
- `cmd_test_workspace` as a separate function — merged into `cmd_test`
- Sequential recipe loop in `cmd_run` — replaced by engine call

## What Stays in cook-cli

- `workspace_to_layout` — anti-corruption layer: Workspace → engine types
- Test recipe discovery (scanning Cookfile ASTs)
- Progress renderer wiring (EngineEvent → cook-progress)
- Environment resolution
- Result formatting (test output, JUnit XML, terminal summary)
- Workspace loading
- File watcher (cook serve)
