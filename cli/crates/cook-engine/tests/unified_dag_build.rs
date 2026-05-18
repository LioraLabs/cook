//! Smoke test: `build_dag` handles cross-recipe edges in a single call
//! that spans every recipe in the workspace.
//!
//! The unified-DAG model (SHI-222) relies on this property: every reachable
//! recipe's units are passed in one `Vec<RecipeUnits>`, and cross-recipe
//! edges (both coarse `deps` and fine-grained `dep_edges`) are resolved
//! intra-call against the running `recipe_leaves` accumulator.
//!
//! The wave loop in the old call-site hid this property externally because
//! each call saw only one wave; this test pins it from outside the crate so
//! a future refactor cannot regress the contract.
//!
//! Phase 4 Task 4.2 of `standard/plans/2026-05-18-unified-register-and-dag-plan.md`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cook_contracts::{CapturedUnit, DepKind, RecipeUnits, WorkPayload};
use cook_engine::dag_builder::build_dag;

fn recipe(name: &str, cmd: &str) -> RecipeUnits {
    RecipeUnits {
        recipe_name: name.to_string(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: WorkPayload::Shell {
                cmd: cmd.to_string(),
                line: 1,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
        }],
        step_groups: vec![],
        working_dir: PathBuf::from("."),
        env_vars: BTreeMap::new(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    }
}

#[test]
fn dag_builder_assembles_cross_recipe_edges_across_full_set() {
    // Two recipes in a single call: `app` declares a coarse dep on `lib`.
    // The unified-DAG model passes both recipes to `build_dag` together,
    // and the cross-recipe edge MUST land in the resulting DAG.
    let lib = recipe("lib", "touch lib.a");
    let mut app = recipe("app", "touch app.b");
    app.deps = vec!["lib".to_string()];

    let dag = build_dag(vec![lib, app]).expect("build_dag should succeed");
    assert_eq!(
        dag.len(),
        2,
        "expected exactly two nodes (one per recipe), got {}",
        dag.len()
    );

    // node 0 = lib (no deps), node 1 = app (depends on lib's leaf).
    assert_eq!(
        dag.node(0).remaining_deps(),
        0,
        "lib unit has no deps"
    );
    assert_eq!(
        dag.node(1).remaining_deps(),
        1,
        "app unit must depend on lib's terminal node — cross-recipe edge \
         resolved within the single build_dag call"
    );
}

#[test]
fn dag_builder_handles_three_recipe_chain_in_one_call() {
    // a -> b -> c, all passed in a single call. Verifies the
    // recipe_leaves accumulator works correctly when more than two recipes
    // chain across the call.
    let a = recipe("a", "touch a.out");
    let mut b = recipe("b", "touch b.out");
    b.deps = vec!["a".to_string()];
    let mut c = recipe("c", "touch c.out");
    c.deps = vec!["b".to_string()];

    let dag = build_dag(vec![a, b, c]).expect("build_dag should succeed");
    assert_eq!(dag.len(), 3);

    assert_eq!(dag.node(0).remaining_deps(), 0, "a is a root");
    assert_eq!(dag.node(1).remaining_deps(), 1, "b depends on a");
    assert_eq!(
        dag.node(2).remaining_deps(),
        1,
        "c depends on b (terminal of b is b's only unit)"
    );
}

#[test]
fn dag_builder_handles_fine_grained_dep_edges_across_full_set() {
    // Fine-grained `dep_edges` (rather than coarse `deps`) must also be
    // resolved intra-call. Recipe `app` has `dep_edges = [(0, "lib")]`,
    // meaning unit #0 of `app` depends on `lib`'s terminal nodes.
    let lib = recipe("lib", "touch lib.a");
    let mut app = recipe("app", "touch app.b");
    app.dep_edges = vec![(0, "lib".to_string())];

    let dag = build_dag(vec![lib, app]).expect("build_dag should succeed");
    assert_eq!(dag.len(), 2);
    assert_eq!(
        dag.node(1).remaining_deps(),
        1,
        "app's unit 0 must depend on lib via fine-grained dep_edges"
    );
}
