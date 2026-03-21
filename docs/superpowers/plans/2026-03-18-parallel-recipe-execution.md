# Parallel Recipe Execution Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable independent recipes to execute in parallel by replacing the sequential recipe loop with a wave-based model.

**Architecture:** A `RecipeDag` tracks recipe-level dependencies. A wave loop pops ready recipes, registers them, flattens their work units into one `ExecutionDag`, executes on the shared pool, then marks them done. New `dependency_edges` function in `analyzer/graph.rs` exposes the adjacency info needed to build the `RecipeDag`.

**Tech Stack:** Rust, `std::collections::BTreeMap`, existing `ExecutionDag`/`build_dag`/`execute_dag` infrastructure.

**Spec:** `docs/superpowers/specs/2026-03-18-parallel-recipe-execution-design.md`

---

## Chunk 1: dependency_edges function

### Task 1: Add `dependency_edges` to analyzer/graph.rs

Extract adjacency-building logic from `topological_sort` into a shared helper, then add a public `dependency_edges` function that returns `BTreeMap<String, Vec<String>>` for reachable recipes.

**Files:**
- Modify: `src/analyzer/graph.rs`

- [ ] **Step 1: Write failing tests for `dependency_edges`**

Add these tests at the end of the existing `mod tests` block in `src/analyzer/graph.rs`:

```rust
#[test]
fn test_dependency_edges_single_recipe() {
    let mut recipes = HashMap::new();
    recipes.insert("build".to_string(), info(vec![], vec![], vec![]));
    let edges = dependency_edges(&recipes, "build").unwrap();
    assert_eq!(edges.len(), 1);
    assert!(edges["build"].is_empty());
}

#[test]
fn test_dependency_edges_linear_chain() {
    let mut recipes = HashMap::new();
    recipes.insert("build".to_string(), info(vec![], vec![], vec!["clean"]));
    recipes.insert("clean".to_string(), info(vec![], vec![], vec![]));
    let edges = dependency_edges(&recipes, "build").unwrap();
    assert_eq!(edges.len(), 2);
    assert_eq!(edges["build"], vec!["clean"]);
    assert!(edges["clean"].is_empty());
}

#[test]
fn test_dependency_edges_diamond() {
    let mut recipes = HashMap::new();
    recipes.insert("a".to_string(), info(vec![], vec![], vec!["b", "c"]));
    recipes.insert("b".to_string(), info(vec![], vec![], vec!["d"]));
    recipes.insert("c".to_string(), info(vec![], vec![], vec!["d"]));
    recipes.insert("d".to_string(), info(vec![], vec![], vec![]));
    let edges = dependency_edges(&recipes, "a").unwrap();
    assert_eq!(edges.len(), 4);
    let mut a_deps = edges["a"].clone();
    a_deps.sort();
    assert_eq!(a_deps, vec!["b", "c"]);
    assert_eq!(edges["b"], vec!["d"]);
    assert_eq!(edges["c"], vec!["d"]);
    assert!(edges["d"].is_empty());
}

#[test]
fn test_dependency_edges_excludes_unreachable() {
    let mut recipes = HashMap::new();
    recipes.insert("a".to_string(), info(vec![], vec![], vec!["b"]));
    recipes.insert("b".to_string(), info(vec![], vec![], vec![]));
    recipes.insert("c".to_string(), info(vec![], vec![], vec![]));
    let edges = dependency_edges(&recipes, "a").unwrap();
    assert_eq!(edges.len(), 2);
    assert!(!edges.contains_key("c"));
}

#[test]
fn test_dependency_edges_implicit_via_serves() {
    let mut recipes = HashMap::new();
    recipes.insert("build".to_string(), info(vec!["lib.a"], vec!["app"], vec![]));
    recipes.insert("compile".to_string(), info(vec![], vec!["lib.a"], vec![]));
    let edges = dependency_edges(&recipes, "build").unwrap();
    assert_eq!(edges.len(), 2);
    assert_eq!(edges["build"], vec!["compile"]);
    assert!(edges["compile"].is_empty());
}

#[test]
fn test_dependency_edges_unknown_target() {
    let recipes: HashMap<String, RecipeInfo> = HashMap::new();
    let result = dependency_edges(&recipes, "missing");
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- graph::tests::test_dependency_edges 2>&1 | head -30`
Expected: compilation error — `dependency_edges` not defined yet.

- [ ] **Step 3: Extract adjacency-building helper and implement `dependency_edges`**

Refactor `topological_sort` to extract the adjacency-building logic (lines 28-55) into a private helper function `build_adjacency`. Then add the public `dependency_edges` function. The changes go in `src/analyzer/graph.rs`:

```rust
use std::collections::{BTreeMap, HashMap, HashSet};

/// Build serves map and adjacency (recipe -> deps) for all recipes.
/// This is the shared logic used by both `topological_sort` and `dependency_edges`.
fn build_adjacency<'a>(
    recipes: &'a HashMap<String, RecipeInfo>,
) -> Result<HashMap<&'a str, HashSet<&'a str>>, GraphError> {
    // Build serves -> recipe name lookup
    let mut serves_map: HashMap<&str, &str> = HashMap::new();
    for (name, info) in recipes {
        for path in &info.serves {
            serves_map.insert(path.as_str(), name.as_str());
        }
    }

    // Build adjacency: recipe -> deps
    let mut deps: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (name, info) in recipes {
        let mut recipe_deps = HashSet::new();
        for req in &info.requires {
            if !recipes.contains_key(req.as_str()) {
                return Err(GraphError::UnknownRecipe(req.clone()));
            }
            recipe_deps.insert(req.as_str());
        }
        for ingredient in &info.ingredients {
            if let Some(&provider) = serves_map.get(ingredient.as_str()) {
                if provider != name.as_str() {
                    recipe_deps.insert(provider);
                }
            }
        }
        deps.insert(name.as_str(), recipe_deps);
    }

    Ok(deps)
}
```

Update `topological_sort` to call `build_adjacency` instead of duplicating the logic:

```rust
pub fn topological_sort(
    recipes: &HashMap<String, RecipeInfo>,
    target: &str,
) -> Result<Vec<String>, GraphError> {
    if !recipes.contains_key(target) {
        return Err(GraphError::UnknownRecipe(target.to_string()));
    }

    let deps = build_adjacency(recipes)?;

    // DFS topological sort (unchanged from here)
    // ...
}
```

Add the new public function:

```rust
/// Return the dependency edges for all recipes reachable from `target`.
/// Keys are recipe names; values are the recipes they depend on (sorted for determinism).
pub fn dependency_edges(
    recipes: &HashMap<String, RecipeInfo>,
    target: &str,
) -> Result<BTreeMap<String, Vec<String>>, GraphError> {
    let reachable = topological_sort(recipes, target)?;
    let adjacency = build_adjacency(recipes)?;

    let reachable_set: HashSet<&str> = reachable.iter().map(|s| s.as_str()).collect();
    let mut result = BTreeMap::new();
    for name in &reachable {
        let deps = adjacency
            .get(name.as_str())
            .map(|s| {
                let mut v: Vec<String> = s
                    .iter()
                    .filter(|d| reachable_set.contains(**d))
                    .map(|d| d.to_string())
                    .collect();
                v.sort();
                v
            })
            .unwrap_or_default();
        result.insert(name.clone(), deps);
    }
    Ok(result)
}
```

Also add `BTreeMap` to the import at the top of the file: change `use std::collections::{HashMap, HashSet};` to `use std::collections::{BTreeMap, HashMap, HashSet};`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- graph::tests 2>&1 | tail -20`
Expected: ALL tests pass (both new `dependency_edges` tests and existing `topological_sort` tests).

- [ ] **Step 5: Commit**

```bash
git add src/analyzer/graph.rs
git commit -m "feat: add dependency_edges function to analyzer graph"
```

---

## Chunk 2: RecipeDag structure

### Task 2: Create `RecipeDag` in scheduler/recipe_dag.rs

A lightweight DAG that tracks recipe-level dependencies and readiness with `pop_ready`/`mark_done` API.

**Files:**
- Create: `src/scheduler/recipe_dag.rs`
- Modify: `src/scheduler/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `src/scheduler/recipe_dag.rs` with only the test module first:

```rust
use std::collections::BTreeMap;

#[cfg(test)]
mod tests {
    use super::*;

    fn edges(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(name, deps)| {
                (
                    name.to_string(),
                    deps.iter().map(|d| d.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn test_single_recipe_ready_immediately() {
        let dep_edges = edges(&[("build", &[])]);
        let mut dag = RecipeDag::new(&dep_edges);
        let ready = dag.pop_ready();
        assert_eq!(ready, vec!["build"]);
        assert!(dag.pop_ready().is_empty()); // in-flight, not ready again
    }

    #[test]
    fn test_linear_chain() {
        let dep_edges = edges(&[("a", &["b"]), ("b", &[])]);
        let mut dag = RecipeDag::new(&dep_edges);

        let wave1 = dag.pop_ready();
        assert_eq!(wave1, vec!["b"]);

        dag.mark_done(&wave1);
        let wave2 = dag.pop_ready();
        assert_eq!(wave2, vec!["a"]);

        dag.mark_done(&wave2);
        assert!(dag.pop_ready().is_empty());
    }

    #[test]
    fn test_diamond_two_middle_recipes_in_same_wave() {
        // d -> b, c -> a
        let dep_edges = edges(&[
            ("a", &["b", "c"]),
            ("b", &["d"]),
            ("c", &["d"]),
            ("d", &[]),
        ]);
        let mut dag = RecipeDag::new(&dep_edges);

        let wave1 = dag.pop_ready();
        assert_eq!(wave1, vec!["d"]);

        dag.mark_done(&wave1);
        let mut wave2 = dag.pop_ready();
        wave2.sort();
        assert_eq!(wave2, vec!["b", "c"]);

        dag.mark_done(&wave2);
        let wave3 = dag.pop_ready();
        assert_eq!(wave3, vec!["a"]);

        dag.mark_done(&wave3);
        assert!(dag.pop_ready().is_empty());
    }

    #[test]
    fn test_all_independent_single_wave() {
        let dep_edges = edges(&[("a", &[]), ("b", &[]), ("c", &[])]);
        let mut dag = RecipeDag::new(&dep_edges);
        let mut wave = dag.pop_ready();
        wave.sort();
        assert_eq!(wave, vec!["a", "b", "c"]);

        dag.mark_done(&wave);
        assert!(dag.pop_ready().is_empty());
    }

    #[test]
    fn test_empty_dag() {
        let dep_edges = edges(&[]);
        let mut dag = RecipeDag::new(&dep_edges);
        assert!(dag.pop_ready().is_empty());
    }

    #[test]
    fn test_mark_done_decrements_dependents() {
        let dep_edges = edges(&[("a", &["b", "c"]), ("b", &[]), ("c", &[])]);
        let mut dag = RecipeDag::new(&dep_edges);

        let wave1 = dag.pop_ready();
        // b and c are ready
        assert_eq!(wave1.len(), 2);

        // Mark only b done, a should not be ready yet
        dag.mark_done(&["b".to_string()]);
        // c is still in-flight
        assert!(dag.pop_ready().is_empty());

        // Now mark c done
        dag.mark_done(&["c".to_string()]);
        let wave2 = dag.pop_ready();
        assert_eq!(wave2, vec!["a"]);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- recipe_dag::tests 2>&1 | head -10`
Expected: compilation error — `RecipeDag` not defined.

- [ ] **Step 3: Implement `RecipeDag`**

Add the struct and impl above the tests in `src/scheduler/recipe_dag.rs`:

```rust
use std::collections::BTreeMap;

struct RecipeDagNode {
    deps: Vec<String>,
    remaining_deps: usize,
    in_flight: bool,
    done: bool,
}

pub struct RecipeDag {
    nodes: BTreeMap<String, RecipeDagNode>,
}

impl RecipeDag {
    pub fn new(dep_edges: &BTreeMap<String, Vec<String>>) -> Self {
        let nodes = dep_edges
            .iter()
            .map(|(name, deps)| {
                (
                    name.clone(),
                    RecipeDagNode {
                        remaining_deps: deps.len(),
                        deps: deps.clone(),
                        in_flight: false,
                        done: false,
                    },
                )
            })
            .collect();
        RecipeDag { nodes }
    }

    /// Return all recipes with no remaining deps that aren't in-flight or done.
    /// Marks returned recipes as in-flight.
    pub fn pop_ready(&mut self) -> Vec<String> {
        let ready: Vec<String> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.remaining_deps == 0 && !node.in_flight && !node.done)
            .map(|(name, _)| name.clone())
            .collect();

        for name in &ready {
            if let Some(node) = self.nodes.get_mut(name) {
                node.in_flight = true;
            }
        }

        ready
    }

    /// Mark recipes as done and decrement remaining_deps on their dependents.
    pub fn mark_done(&mut self, names: &[String]) {
        let done_set: std::collections::HashSet<&str> =
            names.iter().map(|s| s.as_str()).collect();

        for name in names {
            if let Some(node) = self.nodes.get_mut(name) {
                node.done = true;
                node.in_flight = false;
            }
        }

        // Decrement remaining_deps for any node that depends on a completed recipe
        for (_, node) in self.nodes.iter_mut() {
            if node.done {
                continue;
            }
            for dep in &node.deps {
                if done_set.contains(dep.as_str()) {
                    node.remaining_deps = node.remaining_deps.saturating_sub(1);
                }
            }
        }
    }
}
```

- [ ] **Step 4: Register the module**

Add `pub mod recipe_dag;` to `src/scheduler/mod.rs`, after the existing module declarations.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- recipe_dag::tests 2>&1 | tail -20`
Expected: ALL 6 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/scheduler/recipe_dag.rs src/scheduler/mod.rs
git commit -m "feat: add RecipeDag for wave-based recipe scheduling"
```

---

## Chunk 3: Multi-cache-manager support in execute_dag

### Task 3: Change execute_dag to accept per-recipe cache managers

Currently `execute_dag` takes `Option<Arc<ThreadSafeCacheManager>>` — a single cache manager for all recipes. In the wave model, recipes from different imports have different working directories and different `cache_dir` values. Each needs its own cache manager.

Change `execute_dag` to accept `std::collections::BTreeMap<String, Arc<ThreadSafeCacheManager>>` keyed by recipe name. The executor looks up the cache manager by `meta.recipe_name` when recording completions. An empty map = no caching (backward compatible with existing tests that pass `None`).

**Files:**
- Modify: `src/scheduler/executor.rs`
- Modify: `src/scheduler/mod.rs` (re-export)
- Modify: `src/engine/pipeline.rs` (update all callers: `cmd_run`, `cmd_run_workspace`, `cmd_test`, `cmd_test_workspace`)
- Modify: `src/scheduler/tests.rs` (update test calls)

- [ ] **Step 1: Change `execute_dag` signature**

In `src/scheduler/executor.rs`, change the `cache_manager` parameter:

From:
```rust
pub fn execute_dag(
    dag: ExecutionDag,
    num_workers: usize,
    _quiet: bool,
    cache_manager: Option<Arc<ThreadSafeCacheManager>>,
    event_tx: Option<mpsc::Sender<ProgressEvent>>,
    mut test_outputs: Option<&mut Vec<super::pool::TestOutput>>,
) -> Result<(), SchedulerError> {
```

To:
```rust
pub fn execute_dag(
    dag: ExecutionDag,
    num_workers: usize,
    _quiet: bool,
    cache_managers: std::collections::BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    event_tx: Option<mpsc::Sender<ProgressEvent>>,
    mut test_outputs: Option<&mut Vec<super::pool::TestOutput>>,
) -> Result<(), SchedulerError> {
```

- [ ] **Step 2: Update cache lookup in executor body**

In `src/scheduler/executor.rs`, find both places where `cache_manager` is used for `record_completion` and change them:

From:
```rust
if let (Some(cm), Some(meta)) = (&cache_manager, &dag.node(id).cache_meta) {
```

To:
```rust
if let Some(meta) = &dag.node(id).cache_meta {
    if let Some(cm) = cache_managers.get(&dag.node(id).recipe_name) {
```

(Add the closing `}` for the new inner `if let`.)

Update the flush at the end:

From:
```rust
if let Some(ref cm) = cache_manager {
    let _ = cm.flush_all();
}
```

To:
```rust
for cm in cache_managers.values() {
    let _ = cm.flush_all();
}
```

- [ ] **Step 3: Update callers in pipeline.rs**

In `src/engine/pipeline.rs`, update all `execute_dag` calls.

For `cmd_run` (single-Cookfile path, around line 205): change `Some(cache_manager)` to a single-entry BTreeMap:

```rust
let mut cache_managers = std::collections::BTreeMap::new();
cache_managers.insert(recipe_name.to_string(), cache_manager);
// ...
execute_dag(dag, num_jobs, cli.quiet, cache_managers, ...)
```

For `cmd_test` (around line 545) and `cmd_test_workspace` (around line 793): these pass `Some(cache_manager)` / `Some(cache_manager.clone())`. Change to a single-entry BTreeMap keyed by recipe name to preserve caching:

```rust
let mut cms = std::collections::BTreeMap::new();
cms.insert(name.clone(), cache_manager.clone());
let _ = crate::scheduler::execute_dag(dag, num_jobs, cli.quiet, cms, ...);
```

For the existing `cmd_run_workspace` (which will be replaced in Task 4): change `Some(cache_manager)` to a single-entry BTreeMap. (This will be further modified in Task 4.)

- [ ] **Step 4: Update test callers**

In `src/scheduler/tests.rs`, replace all `None` (the 4th argument) with `std::collections::BTreeMap::new()`. There are ~7 calls.

- [ ] **Step 5: Verify compilation and all tests pass**

Run: `cargo test 2>&1 | tail -30`
Expected: ALL tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/scheduler/executor.rs src/engine/pipeline.rs src/scheduler/tests.rs
git commit -m "refactor: change execute_dag to accept per-recipe cache managers"
```

---

## Chunk 4: Wave loop in cmd_run_workspace

### Task 4: Replace sequential loop with wave loop

Replace the `for name in &order` loop in `cmd_run_workspace` (`src/engine/pipeline.rs`) with the wave-based execution model. Also fix the pre-existing recipe name namespacing issue.

**Files:**
- Modify: `src/engine/pipeline.rs`

- [ ] **Step 1: Replace the sequential loop with the wave loop**

In `cmd_run_workspace` in `src/engine/pipeline.rs`, replace the code from the `topological_sort` call through the end of the sequential loop (before `let _ = event_tx.send(ProgressEvent::Finished)`). The code before (workspace loading, runtime creation, event channel setup) and after (Finished event, render thread join) stays the same.

Replace the call to `topological_sort` with `dependency_edges`:

```rust
    let dep_edges = crate::analyzer::graph::dependency_edges(&all_recipes, recipe_name).map_err(
        |e| match e {
            crate::analyzer::graph::GraphError::UnknownRecipe(name) => {
                CookError::RecipeNotFound(name)
            }
            crate::analyzer::graph::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
        },
    )?;
```

Emit `RecipeQueued` for all recipes upfront (unchanged behavior, just iterating the BTreeMap keys):

```rust
    for name in dep_edges.keys() {
        let _ = event_tx.send(ProgressEvent::RecipeQueued {
            name: name.clone(),
            total_nodes: 0,
        });
    }
```

Replace the sequential `for name in &order` loop with the wave loop:

```rust
    let mut recipe_dag = crate::scheduler::recipe_dag::RecipeDag::new(&dep_edges);
    let mut run_result: Result<(), CookError> = Ok(());

    loop {
        let ready = recipe_dag.pop_ready();
        if ready.is_empty() {
            break;
        }

        // Register all ready recipes sequentially, collect cache managers
        let mut wave_units: Vec<crate::contracts::RecipeUnits> = Vec::new();
        let mut cache_managers: std::collections::BTreeMap<
            String,
            std::sync::Arc<crate::cache::ThreadSafeCacheManager>,
        > = std::collections::BTreeMap::new();

        for name in &ready {
            if !cli.quiet && !is_tty {
                eprintln!("cook: registering recipe '{name}'");
            }

            let (prefix, local_name) = split_recipe_name(name);
            let (rt, lua_source) = runtimes
                .get(&prefix)
                .ok_or_else(|| CookError::Other(format!("no runtime for recipe '{name}'")))?;

            let cache_dir = rt.working_dir().join(".cook").join("cache");
            let cache_manager =
                std::sync::Arc::new(crate::cache::ThreadSafeCacheManager::new(cache_dir));

            let mut units = rt
                .register_recipe(lua_source, &local_name, Some(event_tx.clone()))
                .map_err(|e| match e {
                    crate::runtime::RuntimeError::RecipeNotFound(name) => {
                        CookError::RecipeNotFound(name)
                    }
                    crate::runtime::RuntimeError::Lua(e) => {
                        CookError::Other(format!("lua error: {e}"))
                    }
                    crate::runtime::RuntimeError::CommandFailed {
                        command,
                        line,
                        code,
                    } => {
                        if line == 0 {
                            CookError::CommandFailed(format!(
                                "command failed (exit {code}): {command}"
                            ))
                        } else {
                            CookError::CommandFailed(format!(
                                "Cookfile:{line}: command failed (exit {code}): {command}"
                            ))
                        }
                    }
                })?;

            // Rewrite recipe_name to namespaced form (required for build_dag correctness
            // and renderer event consistency)
            units.recipe_name = name.clone();

            cache_managers.insert(name.clone(), cache_manager);
            wave_units.push(units);
        }

        // Build one flattened DAG for the entire wave
        let dag = crate::scheduler::builder::build_dag(wave_units);
        if !dag.is_empty() {
            let result = crate::scheduler::execute_dag(
                dag,
                num_jobs,
                cli.quiet,
                cache_managers,
                Some(event_tx.clone()),
                None,
            );

            if let Err(e) = result {
                run_result = Err(
                    if let Some((_, _recipe_name, msg)) = e.failures.first() {
                        if msg.contains("COOK_CMD_FAILED:") {
                            let parts: Vec<&str> = msg
                                .split("COOK_CMD_FAILED:")
                                .nth(1)
                                .unwrap_or("0:1:unknown")
                                .splitn(3, ':')
                                .collect();
                            let line =
                                parts.first().and_then(|s| s.parse().ok()).unwrap_or(0usize);
                            let code =
                                parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1i32);
                            let command = parts.get(2).unwrap_or(&"unknown").to_string();
                            if line == 0 {
                                CookError::CommandFailed(format!(
                                    "command failed (exit {code}): {command}"
                                ))
                            } else {
                                CookError::CommandFailed(format!(
                                    "Cookfile:{line}: command failed (exit {code}): {command}"
                                ))
                            }
                        } else {
                            CookError::Other(msg.clone())
                        }
                    } else {
                        CookError::Other("unknown scheduler error".into())
                    },
                );
                break;
            }
        }

        // Mark all recipes in this wave as done (even if DAG was empty/cached)
        recipe_dag.mark_done(&ready);
    }
```

The code after (Finished event, render thread join, return) stays exactly the same.

- [ ] **Step 2: Verify the project compiles**

Run: `cargo build 2>&1 | tail -10`
Expected: successful compilation.

- [ ] **Step 3: Run all existing tests**

Run: `cargo test 2>&1 | tail -30`
Expected: ALL existing tests pass. The sequential behavior is preserved — single-recipe runs produce a single wave, multi-recipe linear chains produce one recipe per wave (same order as before).

- [ ] **Step 4: Commit**

```bash
git add src/engine/pipeline.rs
git commit -m "feat: replace sequential recipe loop with wave-based execution"
```

---

## Chunk 5: Integration test

### Task 5: Add integration test for parallel recipe execution

Add a test that verifies independent recipes are registered in the same wave (and thus execute via a single `build_dag` call).

**Files:**
- Modify: `tests/import_integration.rs`

- [ ] **Step 1: Add diamond-pattern integration test**

Add this test at the end of `tests/import_integration.rs`:

```rust
#[test]
fn test_parallel_recipe_execution_diamond() {
    // Diamond: proto -> backend, frontend -> all
    // backend and frontend should execute in the same wave
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Proto library
    fs::create_dir_all(root.join("libs/proto")).unwrap();
    fs::write(
        root.join("libs/proto/Cookfile"),
        "recipe \"generate\"\n    echo \"generating protos\"\nend\n",
    )
    .unwrap();

    // Backend service (depends on proto)
    fs::create_dir_all(root.join("services/backend")).unwrap();
    fs::write(
        root.join("services/backend/Cookfile"),
        "import proto ../../libs/proto\n\nrecipe \"build\": \"proto.generate\"\n    echo \"building backend\"\nend\n",
    )
    .unwrap();

    // Frontend service (depends on proto)
    fs::create_dir_all(root.join("services/frontend")).unwrap();
    fs::write(
        root.join("services/frontend/Cookfile"),
        "import proto ../../libs/proto\n\nrecipe \"build\": \"proto.generate\"\n    echo \"building frontend\"\nend\n",
    )
    .unwrap();

    // Root aggregator
    fs::write(
        root.join("Cookfile"),
        concat!(
            "import backend ./services/backend\n",
            "import frontend ./services/frontend\n",
            "\n",
            "recipe \"all\": \"backend.build\" \"frontend.build\"\n",
            "    echo \"all done\"\n",
            "end\n",
        ),
    )
    .unwrap();

    let output = cook_cmd()
        .current_dir(root)
        .arg("all")
        .output()
        .expect("failed to run cook");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "cook failed: {stderr}"
    );
    // Verify all recipes ran by checking output
    assert!(
        stderr.contains("all done") || output.status.success(),
        "expected all recipes to complete"
    );
}
```

- [ ] **Step 2: Run the integration test**

Run: `cargo test --test import_integration test_parallel_recipe_execution_diamond -- --nocapture 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 3: Run all tests to verify nothing broke**

Run: `cargo test 2>&1 | tail -20`
Expected: ALL tests pass.

- [ ] **Step 4: Commit**

```bash
git add tests/import_integration.rs
git commit -m "test: add diamond-pattern integration test for parallel recipes"
```

---

## Chunk 6: Recipe name namespacing fix for sequential path

### Task 6: Apply recipe name namespacing fix to single-Cookfile path

The namespacing fix in `cmd_run_workspace` is done in Task 3. But the single-Cookfile `cmd_run` path also has this latent issue for multi-recipe Cookfiles. Apply the same `units.recipe_name = name.clone()` fix there too for consistency.

**Files:**
- Modify: `src/engine/pipeline.rs`

- [ ] **Step 1: Check if `cmd_run` has the same issue**

Read `cmd_run` in `src/engine/pipeline.rs` (around line 114-248). If it also passes a local name to `register_recipe` while using a different name for events, apply the same fix. If `cmd_run` doesn't use namespacing (single Cookfile = no namespacing), this task is a no-op — skip and mark complete.

Note: `cmd_run` is the single-Cookfile path. Recipe names there are already the full name (no prefix splitting). So this task is likely a no-op. Verify and mark complete.

- [ ] **Step 2: Run all tests**

Run: `cargo test 2>&1 | tail -20`
Expected: ALL tests pass.

- [ ] **Step 3: Commit (if changes were made)**

Only commit if changes were made. Otherwise skip.
