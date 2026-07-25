use super::*;
use crate::event::{NodeId, NodeKind};
use std::path::PathBuf;

fn topo(recipes: &[(u32, &str, &[u32], usize)]) -> Vec<RecipeTopo> {
    recipes.iter().map(|(id, name, deps, n)| RecipeTopo {
        id: RecipeId::new(*id),
        name: (*name).to_string(),
        deps: deps.iter().map(|d| RecipeId::new(*d)).collect(),
        expected_nodes: *n,
    }).collect()
}

#[test]
fn build_started_seeds_recipes_in_topo_order() {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "deps", &[], 12), (1, "lib", &[0], 6)]),
        total_nodes: 18,
    });
    assert_eq!(s.order, vec![RecipeId::new(0), RecipeId::new(1)]);
    assert_eq!(s.recipes.len(), 2);
    assert_eq!(s.totals.waiting, 2);
    assert_eq!(s.totals.total_nodes, 18);
}

#[test]
fn recipe_started_transitions_waiting_to_running() {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "deps", &[], 2)]), total_nodes: 2,
    });
    s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    assert_eq!(s.recipes[&RecipeId::new(0)].status, Status::Running);
    assert_eq!(s.totals.running, 1);
    assert_eq!(s.totals.waiting, 0);
}

#[test]
fn node_started_inserts_running_node() {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "lib", &[], 1)]), total_nodes: 1,
    });
    s.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0),
        node: NodeId::new(0),
        name: "lvm.c".into(),
        artifact: Some(PathBuf::from("build/obj/lvm.o")),
        fallback_label: "clang -c lvm.c".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        });
    let r = &s.recipes[&RecipeId::new(0)];
    assert_eq!(r.nodes.len(), 1);
    assert_eq!(r.nodes[&NodeId::new(0)].status, NodeStatus::Running);
}

#[test]
fn cache_hit_increments_counter_and_progress() {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "deps", &[], 3)]), total_nodes: 3,
    });
    s.apply(&ProgressEvent::NodeCacheHit {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "a".into(), artifact: None, kind: NodeKind::Cooked,
    });
    let r = &s.recipes[&RecipeId::new(0)];
    assert_eq!(r.cached_count, 1);
    assert_eq!(r.progress, (1, 3));
    assert_eq!(s.totals.completed_nodes, 1);
}

/// A cache hit must carry the node's kind through to the node it creates,
/// so a cached node is labelled exactly as the same node would be on a
/// miss. A test unit's name is a derived label (`<recipe>_test<N>`,
/// CS-0160), not command text: without the kind it fell through to the
/// command-token branch and rendered `$rust_test1`, and before the name
/// reached the node at all, a bare `?`.
#[test]
fn cache_hit_preserves_node_kind_so_test_labels_are_not_command_text() {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "rust", &[], 1)]), total_nodes: 1,
    });
    s.apply(&ProgressEvent::NodeCacheHit {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "rust_test1".into(), artifact: None, kind: NodeKind::Test,
    });
    let n = &s.recipes[&RecipeId::new(0)].nodes[&NodeId::new(0)];
    assert_eq!(n.kind, NodeKind::Test);
    assert_eq!(n.display(), "rust_test1");
}

#[test]
fn recipe_completed_marks_cached_when_all_cached() {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "deps", &[], 2)]), total_nodes: 2,
    });
    s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    s.apply(&ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(10),
        cached: 2, total: 2,
        kind: crate::event::RecipeKind::Recipe,
    });
    assert_eq!(s.recipes[&RecipeId::new(0)].status, Status::Cached);
    assert_eq!(s.totals.cached, 1);
}

#[test]
fn recipe_failed_records_first_error_summary() {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "lib", &[], 1)]), total_nodes: 1,
    });
    s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    s.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "x".into(), artifact: None, fallback_label: "x".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        });
    s.apply(&ProgressEvent::NodeFailed {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(10),
        error: "boom".into(),
    });
    s.apply(&ProgressEvent::RecipeFailed {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(20),
        completed: 1, total: 1,
    });
    let r = &s.recipes[&RecipeId::new(0)];
    assert_eq!(r.status, Status::Failed);
    assert_eq!(r.error_summary.as_deref(), Some("boom"));
}

#[test]
fn duplicate_recipe_completed_does_not_double_count_counters() {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "deps", &[], 1)]), total_nodes: 1,
    });
    s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    s.apply(&ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(10),
        cached: 0, total: 1,
        kind: crate::event::RecipeKind::Recipe,
    });
    let totals_after_first = s.totals;
    s.apply(&ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(10),
        cached: 0, total: 1,
        kind: crate::event::RecipeKind::Recipe,
    });
    assert_eq!(s.totals, totals_after_first, "duplicate RecipeCompleted must not mutate counters");
}

#[test]
fn cached_node_count_sums_per_recipe() {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "deps", &[], 2), (1, "lib", &[], 2)]),
        total_nodes: 4,
    });
    s.apply(&ProgressEvent::NodeCacheHit {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "a".into(), artifact: None, kind: NodeKind::Cooked,
    });
    s.apply(&ProgressEvent::NodeCacheHit {
        recipe: RecipeId::new(1), node: NodeId::new(0),
        name: "b".into(), artifact: None, kind: NodeKind::Cooked,
    });
    assert_eq!(s.totals.cached_node_count(&s), 2);
}

#[test]
fn duplicate_node_completed_does_not_double_count_progress() {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "lib", &[], 2)]), total_nodes: 2,
    });
    s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    s.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "a".into(), artifact: None, fallback_label: "a".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        });
    s.apply(&ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(1),
        kind: NodeKind::Cooked,
        cache_key: None,
    });
    assert_eq!(s.recipes[&RecipeId::new(0)].progress, (1, 2));
    assert_eq!(s.totals.completed_nodes, 1);

    // Duplicate — must not advance progress.
    s.apply(&ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(1),
        kind: NodeKind::Cooked,
        cache_key: None,
    });
    assert_eq!(s.recipes[&RecipeId::new(0)].progress, (1, 2));
    assert_eq!(s.totals.completed_nodes, 1);
}
