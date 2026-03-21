# Unified Engine Entry Point Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the two separate execution paths (single-Cookfile sequential loop + workspace wave loop) with a single `cook_engine::run()` that handles both. Delete duplicated analysis logic from cook-cli.

**Architecture:** cook-engine gets a top-level `run()` function that takes recipe infos, targets, and registries. cook-cli becomes a thin adapter that converts Workspace/Cookfile data into engine types and calls `run()`.

**Tech Stack:** Rust, existing cook-engine and cook-cli crates

**Spec:** `docs/superpowers/specs/2026-03-20-unified-engine-entry-point-design.md`

---

### Task 1: Add `RunResult` and `run()` to cook-engine

**Files:**
- Modify: `cli/crates/cook-engine/src/lib.rs` — add `RunResult` struct
- Create: `cli/crates/cook-engine/src/run.rs` — the unified entry point

- [ ] **Step 1: Add RunResult to lib.rs**

Add to `cli/crates/cook-engine/src/lib.rs`:

```rust
pub mod run;

/// Result of a complete engine run.
pub struct RunResult {
    /// Test outputs collected during execution (for cook-cli to format).
    pub test_outputs: Vec<cook_luaotp::TestOutput>,
}
```

Add `pub mod run;` to the module declarations.

- [ ] **Step 2: Write run.rs**

Create `cli/crates/cook-engine/src/run.rs`. This function:

1. Calls `analyzer::dependency_edges()` to build the recipe dep graph across all targets
2. Creates a `RecipeDag` from those edges
3. Wave loop: `pop_ready` → for each ready recipe, look up its registry by prefix, call `registry.register_recipe()` → build DAG → execute → mark done
4. Returns `RunResult` with collected test outputs

The key logic is the wave loop from the current `cmd_run_workspace` in pipeline.rs, but generalized:

```rust
pub fn run(
    recipe_infos: &BTreeMap<String, analyzer::RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, (cook_register::Registry, String)>,
    num_jobs: usize,
    on_event: impl Fn(EngineEvent) + Send + Sync,
) -> Result<RunResult, EngineError>
```

Use `analyzer::dependency_edges` to resolve deps for ALL targets combined. Then `RecipeDag::new(edges)`. Then the wave loop.

For recipe name → registry lookup: split on last `.` to get `(prefix, local_name)`. Empty prefix = root registry.

- [ ] **Step 3: Verify cook-engine compiles**

```bash
cd ~/dev/cook/cli && cargo build -p cook-engine
```

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-engine/ && git commit -m "feat(cook-engine): add unified run() entry point with wave execution"
```

---

### Task 2: Simplify cook-cli to use engine::run()

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs` — replace both execution paths with engine::run() calls

- [ ] **Step 1: Rewrite cmd_run**

Replace the current `cmd_run` (sequential loop + workspace branch) with:

1. Parse and generate Lua
2. Build recipe infos: if workspace → `workspace_to_layout` + `engine::analyzer::build_workspace_recipe_info`, if single → extract from cookfile directly into `engine::analyzer::RecipeInfo`
3. Build registries: single → one registry with `""` prefix, workspace → one per import
4. Call `engine::run(recipe_infos, &[target], registries, num_jobs, on_event)`
5. Handle result

No more `cmd_run_workspace` as a separate function.

- [ ] **Step 2: Rewrite cmd_test**

Same pattern but:
1. Discover test recipes by scanning AST (existing logic)
2. Pass discovered recipe names as `targets` to `engine::run()`
3. Format `RunResult.test_outputs` into test results

No more `cmd_test_workspace` as a separate function.

- [ ] **Step 3: Delete dead code**

Remove from pipeline.rs:
- `cmd_run_workspace`
- `cmd_test_workspace`
- `resolve_execution_order`
- `try_resolve_execution_order`
- `topological_sort_recipe_infos`
- Local `RecipeInfo` struct
- `topological_sort_recipes` (now just `engine::analyzer::topological_sort`)

Keep:
- `workspace_to_layout` — anti-corruption layer
- `build_workspace_recipe_info` — thin wrapper calling engine
- `find_full_prefix` — thin wrapper calling engine
- Test recipe discovery logic
- Progress renderer wiring
- `split_recipe_name` helper

- [ ] **Step 4: Build and test**

```bash
cd ~/dev/cook/cli && cargo build -p cook-cli && cargo test
```

- [ ] **Step 5: Test against examples**

```bash
cd ~/dev/cook/examples && ../cli/target/debug/cook build
cd ~/dev/cook/examples/cpp-project && ../../cli/target/debug/cook app
cd ~/dev/cook/examples/cpp-project && ../../cli/target/debug/cook run-tests
cd ~/dev/cook/examples/monorepo && ../../cli/target/debug/cook build
```

All must pass.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-cli/ && git commit -m "refactor(cook-cli): use unified engine::run() for all execution paths

Deletes duplicated topo sort, merges single-Cookfile and workspace
paths, enables wave-based parallel recipe execution everywhere."
```

---

## Completion Criteria

- [ ] `cook_engine::run()` exists and handles both single and workspace builds
- [ ] No topological sort logic in cook-cli
- [ ] No local `RecipeInfo` struct in cook-cli
- [ ] `cmd_run_workspace` and `cmd_test_workspace` deleted
- [ ] All 3 example projects pass (basic, cpp-project, monorepo)
- [ ] `cargo test` passes for full workspace
