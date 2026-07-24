use super::*;
use cook_progress::event::{NodeId, NodeKind, RecipeId};
use cook_progress::log_reader::{BuildView, NodeView, RecipeView};
use cook_progress::model::{NodeStatus, Status};
use std::collections::BTreeMap;

fn mk(failed_first_node: bool) -> BuildView {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        NodeId::new(0),
        NodeView {
            name: "a".into(),
            status: if failed_first_node {
                NodeStatus::Failed
            } else {
                NodeStatus::Completed
            },
            kind: NodeKind::Cooked,
            started_at: None,
            ended_at: None,
            elapsed_ms: None,
            skip_reason: None,
            lines: vec![],
        },
    );
    nodes.insert(
        NodeId::new(1),
        NodeView {
            name: "b".into(),
            status: NodeStatus::Failed,
            kind: NodeKind::Cooked,
            started_at: None,
            ended_at: None,
            elapsed_ms: None,
            skip_reason: None,
            lines: vec![],
        },
    );
    let mut recipes = BTreeMap::new();
    recipes.insert(
        RecipeId::new(0),
        RecipeView {
            name: "lib".into(),
            status: Status::Failed,
            nodes,
        },
    );
    BuildView {
        build_id: "x".into(),
        started_at: "t".into(),
        ended_at: None,
        exit_code: Some(1),
        recipes,
    }
}

#[test]
fn flat_index_includes_recipe_then_nodes_when_expanded() {
    let s = UiState::new(mk(false), LoadDiagnostics::default());
    assert_eq!(s.flat.len(), 3); // 1 recipe + 2 nodes
    assert!(matches!(s.flat[0], FlatRow::Recipe(_)));
    assert!(matches!(s.flat[1], FlatRow::Node(_, _)));
}

#[test]
fn initial_selection_lands_on_first_failed_node() {
    let s = UiState::new(mk(true), LoadDiagnostics::default());
    assert!(matches!(s.flat[s.selected], FlatRow::Node(_, _)));
    if let FlatRow::Node(_, nid) = s.flat[s.selected] {
        assert_eq!(nid, NodeId::new(0)); // first failed
    }
}

#[test]
fn picker_starts_closed_and_can_be_opened() {
    let mut s = UiState::new(mk(false), LoadDiagnostics::default());
    assert!(s.picker.is_none());
    s.picker = Some(PickerState { builds: vec![], cursor: 0 });
    assert!(s.picker.is_some());
}

#[test]
fn cycle_filter_failed_only_hides_passing_nodes() {
    let mut s = UiState::new(mk(false), LoadDiagnostics::default());
    s.cycle_filter(); // -> FailedOnly
    // Recipe row + only the failing node (b)
    assert_eq!(s.flat.len(), 2);
}

#[test]
fn search_finds_substring_in_node_lines() {
    use cook_progress::log_reader::LogLine;
    use cook_progress::event::Stream;
    let mut view = mk(false);
    // Add a line to the first node containing "error: foo"
    let (_rid, recipe) = view.recipes.iter_mut().next().unwrap();
    let (_nid, node) = recipe.nodes.iter_mut().next().unwrap();
    node.lines.push(LogLine { stream: Stream::Stdout, ts: None, text: "error: foo".into() });

    let mut s = UiState::new(view, LoadDiagnostics::default());
    s.set_search_pattern("ERROR".into());
    assert_eq!(s.search.as_ref().unwrap().matches.len(), 1);
}

#[test]
fn ensure_tree_visible_keeps_selection_in_viewport() {
    let mut s = UiState::new(mk(false), LoadDiagnostics::default());
    // mk() builds 1 recipe + 2 nodes = 3 flat rows. With a 2-row viewport
    // and selection at index 2, scroll must shift to 1 so the selection
    // is visible at the bottom.
    s.selected = 2;
    s.tree_scroll = 0;
    s.ensure_tree_visible(2);
    assert_eq!(s.tree_scroll, 1);

    // Moving selection back to index 0 must pull the viewport up to 0.
    s.selected = 0;
    s.ensure_tree_visible(2);
    assert_eq!(s.tree_scroll, 0);

    // Selection already in view → scroll unchanged (sticky).
    s.selected = 1;
    s.tree_scroll = 0;
    s.ensure_tree_visible(2);
    assert_eq!(s.tree_scroll, 0);
}

#[test]
fn ensure_tree_visible_clamps_when_viewport_larger_than_rows() {
    let mut s = UiState::new(mk(false), LoadDiagnostics::default());
    s.selected = 2;
    s.tree_scroll = 5; // stale offset past end
    s.ensure_tree_visible(10);
    assert_eq!(s.tree_scroll, 0); // clamped because flat.len() <= viewport
}
