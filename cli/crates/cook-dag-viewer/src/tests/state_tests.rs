use super::*;
use crate::dag_data::{EdgeData, EdgeKind, NodeData, WaveData, WaveDagData};

fn unit(id: &str, recipe: &str, label: &str) -> NodeData {
    NodeData {
        id: id.into(),
        kind: "unit".into(),
        label: label.into(),
        recipe: Some(recipe.into()),
        command: Some("cmd".into()),
        output: None,
        cached: Some(true),
        dep_kind: Some("sequential".into()),
        group_index: None,
        modified: None,
        discovered: None,
    }
}

fn graph_2x2() -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![
            WaveData {
                recipes: vec!["a".into(), "b".into()],
                nodes: vec![
                    unit("unit:a:0", "a", "a0"),
                    unit("unit:a:1", "a", "a1"),
                    unit("unit:b:0", "b", "b0"),
                ],
                edges: vec![],
            },
            WaveData {
                recipes: vec!["c".into()],
                nodes: vec![unit("unit:c:0", "c", "c0")],
                edges: vec![],
            },
        ],
        inter_wave_edges: vec![],
    }
}

#[test]
fn tree_groups_units_by_recipe() {
    let t = IndexTree::from_graph(&graph_2x2());
    assert_eq!(t.waves.len(), 2);
    assert_eq!(t.waves[0].recipes.len(), 2);
    assert_eq!(t.waves[0].recipes[0].name, "a");
    assert_eq!(t.waves[0].recipes[0].units.len(), 2);
    assert_eq!(t.waves[0].recipes[0].units[0].label, "a0");
    assert_eq!(t.waves[0].recipes[1].name, "b");
    assert_eq!(t.waves[0].recipes[1].units.len(), 1);
    assert_eq!(t.waves[1].recipes[0].name, "c");
}

#[test]
fn wave_zero_is_expanded_by_default() {
    let t = IndexTree::from_graph(&graph_2x2());
    assert!(t.waves[0].expanded);
    assert!(!t.waves[1].expanded);
}

#[test]
fn recipes_default_collapsed() {
    let t = IndexTree::from_graph(&graph_2x2());
    for w in &t.waves {
        for r in &w.recipes {
            assert!(!r.expanded);
        }
    }
}

#[test]
fn selection_node_id_returns_unit_id_when_fully_qualified() {
    let t = IndexTree::from_graph(&graph_2x2());
    let sel = Selection::unit(0, 0, 1);
    assert_eq!(sel.node_id(&t), Some("unit:a:1"));
}

#[test]
fn selection_node_id_is_none_at_wave_or_recipe_level() {
    let t = IndexTree::from_graph(&graph_2x2());
    assert_eq!(Selection::wave_only(0).node_id(&t), None);
    assert_eq!(Selection::recipe(0, 0).node_id(&t), None);
}

#[test]
fn app_state_starts_with_first_selection_and_follow_on() {
    let app = AppState::new(&graph_2x2());
    assert_eq!(app.selection, Selection::first());
    assert!(app.follow);
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn move_cursor_down_steps_through_waves() {
    let g = graph_2x2();
    let mut app = AppState::new(&g);
    // Wave 0 expanded by default but recipes collapsed.
    // Visible: W0, recipe a, recipe b, W1.
    assert_eq!(app.selection, Selection::wave_only(0));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::recipe(0, 0));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::recipe(0, 1));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::wave_only(1));
}

#[test]
fn expand_then_step_in_descends_into_units() {
    let g = graph_2x2();
    let mut app = AppState::new(&g);
    app.move_cursor(false); // recipe a
    app.expand_or_step_in();
    assert!(app.tree.waves[0].recipes[0].expanded);
    app.move_cursor(false); // first unit a0
    assert_eq!(app.selection, Selection::unit(0, 0, 0));
}

#[test]
fn open_edge_picker_zero_candidates_no_op() {
    let g = graph_2x2();
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);
    app.open_edge_picker(&g, PickerDir::Downstream);
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn open_edge_picker_single_candidate_jumps_directly() {
    let mut g = graph_2x2();
    g.inter_wave_edges.push(EdgeData {
        from: "unit:a:0".into(),
        to: "unit:c:0".into(), kind: EdgeKind::Data });
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);
    app.open_edge_picker(&g, PickerDir::Downstream);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.selection.node_id(&app.tree), Some("unit:c:0"));
}

#[test]
fn open_edge_picker_multiple_candidates_opens_picker() {
    let mut g = graph_2x2();
    g.inter_wave_edges.push(EdgeData {
        from: "unit:a:0".into(),
        to: "unit:c:0".into(), kind: EdgeKind::Data });
    g.waves[0].edges.push(EdgeData {
        from: "unit:a:0".into(),
        to: "unit:b:0".into(), kind: EdgeKind::Data });
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);
    app.open_edge_picker(&g, PickerDir::Downstream);
    assert_eq!(app.mode, Mode::EdgePicker);
    assert_eq!(app.edge_picker.candidates.len(), 2);
}

#[test]
fn pan_camera_disables_follow() {
    let g = graph_2x2();
    let mut app = AppState::new(&g);
    let layout = crate::render::layout::compute(&g, crate::render::layout::LayoutDims::FULL);
    app.pan_camera(10, 10, &layout, ratatui::layout::Rect::new(0, 0, 80, 24));
    assert!(!app.follow);
}

#[test]
fn recenter_reengages_follow() {
    let g = graph_2x2();
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);
    let layout = crate::render::layout::compute(&g, crate::render::layout::LayoutDims::FULL);
    app.follow = false;
    app.recenter(&layout, ratatui::layout::Rect::new(0, 0, 80, 24));
    assert!(app.follow);
}

#[test]
fn pin_state_starts_empty() {
    let p = PinState::default();
    assert!(p.is_empty());
    assert!(!p.is_full());
    assert_eq!(p.iter().count(), 0);
}

#[test]
fn pin_returns_first_empty_slot() {
    let mut p = PinState::default();
    assert_eq!(p.pin("a"), Some(0));
    assert_eq!(p.pin("b"), Some(1));
    assert_eq!(p.pin("c"), Some(2));
}

#[test]
fn pin_is_idempotent_for_same_id() {
    let mut p = PinState::default();
    p.pin("a");
    p.pin("b");
    assert_eq!(p.pin("a"), Some(0), "re-pinning returns existing slot");
}

#[test]
fn pin_returns_none_when_full() {
    let mut p = PinState::default();
    for i in 0..PIN_SLOTS {
        p.pin(&format!("n{i}"));
    }
    assert!(p.is_full());
    assert_eq!(p.pin("overflow"), None);
}

#[test]
fn unpin_clears_slot_and_returns_true() {
    let mut p = PinState::default();
    p.pin("a");
    assert_eq!(p.unpin("a"), true);
    assert!(p.is_empty());
}

#[test]
fn unpin_returns_false_when_not_pinned() {
    let mut p = PinState::default();
    assert_eq!(p.unpin("nonesuch"), false);
}

#[test]
fn slot_of_finds_existing_pin() {
    let mut p = PinState::default();
    p.pin("a");
    p.pin("b");
    assert_eq!(p.slot_of("a"), Some(0));
    assert_eq!(p.slot_of("b"), Some(1));
    assert_eq!(p.slot_of("c"), None);
}

#[test]
fn id_at_returns_pinned_id() {
    let mut p = PinState::default();
    p.pin("a");
    assert_eq!(p.id_at(0), Some("a"));
    assert_eq!(p.id_at(1), None);
}

#[test]
fn iter_yields_pairs_in_slot_order_skipping_empty() {
    let mut p = PinState::default();
    p.pin("a");
    p.pin("b");
    p.pin("c");
    p.unpin("b");
    let pairs: Vec<(usize, &str)> = p.iter().collect();
    assert_eq!(pairs, vec![(0, "a"), (2, "c")]);
}

#[test]
fn pin_after_unpin_reuses_freed_slot() {
    let mut p = PinState::default();
    p.pin("a");
    p.pin("b");
    p.unpin("a");
    assert_eq!(p.pin("c"), Some(0), "should reuse the lowest empty slot");
}

#[test]
fn clear_empties_all_slots() {
    let mut p = PinState::default();
    p.pin("a");
    p.pin("b");
    p.clear();
    assert!(p.is_empty());
}

#[test]
fn pin_msg_full_renders_clear_hint() {
    assert_eq!(
        PinMsg::Full.render(),
        "pin slots full — clear with X"
    );
}

#[test]
fn pin_msg_cleared_all_handles_singular_and_plural() {
    assert_eq!(PinMsg::ClearedAll(1).render(), "cleared 1 pin");
    assert_eq!(PinMsg::ClearedAll(3).render(), "cleared 3 pins");
    assert_eq!(PinMsg::ClearedAll(0).render(), "cleared 0 pins");
}

#[test]
fn pin_msg_empty_slot_uses_one_indexed_label() {
    assert_eq!(PinMsg::EmptySlot(0).render(), "slot 1 empty");
    assert_eq!(PinMsg::EmptySlot(8).render(), "slot 9 empty");
}

fn graph_with_files() -> WaveDagData {
    use crate::dag_data::EdgeData;
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: vec![
                NodeData {
                    id: "file:foo.cpp".into(),
                    kind: "file".into(),
                    label: "foo.cpp".into(),
                    recipe: None,
                    command: None,
                    output: None,
                    cached: None,
                    dep_kind: None,
                    group_index: None,
                    modified: Some(false),
                    discovered: None,
                },
                NodeData {
                    id: "unit:a:0".into(),
                    kind: "unit".into(),
                    label: "a0".into(),
                    recipe: Some("a".into()),
                    command: Some("c".into()),
                    output: None,
                    cached: Some(true),
                    dep_kind: Some("sequential".into()),
                    group_index: None,
                    modified: None,
                    discovered: None,
                },
            ],
            edges: vec![EdgeData { from: "file:foo.cpp".into(), to: "unit:a:0".into(), kind: EdgeKind::Data }],
        }],
        inter_wave_edges: vec![],
    }
}

#[test]
fn move_cursor_walks_through_file_rows_when_folder_expanded() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    // Visible rows in order:
    //   wave_only(0)
    //   files_folder(0)
    //   file(0, 0)            ← foo.cpp
    //   recipe(0, 0)
    assert_eq!(app.selection, Selection::wave_only(0));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::files_folder(0));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::file(0, 0));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::recipe(0, 0));
}

#[test]
fn jump_to_node_on_file_lands_on_file_row_and_expands_folder() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = false;
    app.jump_to_node("file:foo.cpp");
    assert_eq!(app.selection, Selection::file(0, 0));
    assert!(app.tree.waves[0].expanded);
    assert!(app.tree.waves[0].files_expanded);
}

#[test]
fn selection_node_id_resolves_file_leaf() {
    let g = graph_with_files();
    let app = AppState::new(&g);
    let sel = Selection::file(0, 0);
    assert_eq!(sel.node_id(&app.tree), Some("file:foo.cpp"));
}

#[test]
fn bulk_pin_recipe_on_file_selection_emits_on_file() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    app.selection = Selection::file(0, 0);
    app.bulk_pin_recipe(&g);
    assert_eq!(app.last_pin_message, Some(PinMsg::OnFile));
    assert!(app.pins.is_empty());
}

#[test]
fn bulk_pin_recipe_on_files_folder_selection_emits_on_file() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.selection = Selection::files_folder(0);
    app.bulk_pin_recipe(&g);
    assert_eq!(app.last_pin_message, Some(PinMsg::OnFile));
    assert!(app.pins.is_empty());
}

#[test]
fn files_folder_constructor_builds_expected_selection() {
    let sel = Selection::files_folder(2);
    assert_eq!(sel.wave, 2);
    assert!(matches!(sel.leaf, Some(SelectionLeaf::FilesFolder)));
}

#[test]
fn selection_node_id_returns_none_for_files_folder() {
    let g = graph_with_files();
    let app = AppState::new(&g);
    assert_eq!(Selection::files_folder(0).node_id(&app.tree), None);
}

#[test]
fn visible_rows_includes_files_folder_when_wave_expanded_and_has_files() {
    let g = graph_with_files();
    let app = AppState::new(&g);
    // graph_with_files() has wave 0 with one file (foo.cpp) and one unit.
    // Wave 0 is expanded by default. files_expanded is false by default.
    let rows = app.visible_rows();
    // Expected order:
    //   wave_only(0)
    //   files_folder(0)            ← new: present even when files collapsed
    //   recipe(0, 0)
    assert_eq!(rows[0], Selection::wave_only(0));
    assert_eq!(rows[1], Selection::files_folder(0));
    assert_eq!(rows[2], Selection::recipe(0, 0));
}

#[test]
fn visible_rows_omits_files_folder_when_wave_has_no_files() {
    let g = graph_2x2(); // no files in either wave
    let app = AppState::new(&g);
    let rows = app.visible_rows();
    // Wave 0 expanded, two recipes collapsed, then wave 1 collapsed.
    assert_eq!(rows[0], Selection::wave_only(0));
    assert_eq!(rows[1], Selection::recipe(0, 0));
    assert_eq!(rows[2], Selection::recipe(0, 1));
    assert_eq!(rows[3], Selection::wave_only(1));
    assert!(!rows.iter().any(|s| matches!(s.leaf, Some(SelectionLeaf::FilesFolder))));
}

#[test]
fn move_cursor_lands_on_files_folder_after_wave() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    assert_eq!(app.selection, Selection::wave_only(0));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::files_folder(0));
    app.move_cursor(false);
    // files_expanded is still false, so next row is the recipe.
    assert_eq!(app.selection, Selection::recipe(0, 0));
}

#[test]
fn expand_step_in_on_wave_with_files_steps_into_folder_row() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    // Wave 0 already expanded by default. Selection = wave_only(0).
    app.expand_or_step_in();
    // New behavior: stepping into an already-expanded wave with files
    // moves selection to the folder row (does NOT toggle files_expanded).
    assert_eq!(app.selection, Selection::files_folder(0));
    assert!(!app.tree.waves[0].files_expanded);
}

#[test]
fn expand_step_in_on_files_folder_collapsed_expands_it() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.selection = Selection::files_folder(0);
    app.expand_or_step_in();
    assert!(app.tree.waves[0].files_expanded);
    // Selection stays on the folder row after expansion.
    assert_eq!(app.selection, Selection::files_folder(0));
}

#[test]
fn expand_step_in_on_files_folder_expanded_steps_into_first_file() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    app.selection = Selection::files_folder(0);
    app.expand_or_step_in();
    assert_eq!(app.selection, Selection::file(0, 0));
}

#[test]
fn expand_step_in_on_wave_with_no_files_steps_into_first_recipe() {
    let g = graph_2x2();
    let mut app = AppState::new(&g);
    // Wave 0 expanded by default, no files.
    app.expand_or_step_in();
    assert_eq!(app.selection, Selection::recipe(0, 0));
}

#[test]
fn collapse_step_out_on_file_returns_to_folder_row() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    app.selection = Selection::file(0, 0);
    app.collapse_or_step_out();
    assert_eq!(app.selection, Selection::files_folder(0));
    // Folder stays expanded; we only step the cursor up one level.
    assert!(app.tree.waves[0].files_expanded);
}

#[test]
fn collapse_step_out_on_files_folder_expanded_collapses_folder() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    app.selection = Selection::files_folder(0);
    app.collapse_or_step_out();
    assert!(!app.tree.waves[0].files_expanded);
    // Selection stays on the folder row after collapse.
    assert_eq!(app.selection, Selection::files_folder(0));
}

#[test]
fn collapse_step_out_on_files_folder_collapsed_returns_to_wave() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.selection = Selection::files_folder(0);
    assert!(!app.tree.waves[0].files_expanded);
    app.collapse_or_step_out();
    assert_eq!(app.selection, Selection::wave_only(0));
}

#[test]
fn collapse_step_out_on_collapsed_recipe_row_returns_to_wave() {
    let g = graph_2x2();
    let mut app = AppState::new(&g);
    app.selection = Selection::recipe(0, 0);
    // Recipe is collapsed by default.
    app.collapse_or_step_out();
    assert_eq!(app.selection, Selection::wave_only(0));
}

fn tall_graph(unit_count: usize) -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: (0..unit_count).map(|i| unit(&format!("unit:a:{i}"), "a", &format!("u{i}"))).collect(),
            edges: vec![],
        }],
        inter_wave_edges: vec![],
    }
}

#[test]
fn ensure_index_visible_keeps_in_view_selection_does_not_move_scroll() {
    let g = tall_graph(20);
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    // Visible rows: wave_only, recipe(0), unit(0..20). Total 22 rows.
    // Pretend the pane is 10 rows tall.
    app.index_scroll = 5;
    // Selection at row 8 — already in [5, 15).
    app.selection = Selection::unit(0, 0, 6); // logical idx = 2 (wave) + 1 (recipe) + 6 = wait
    // Visible rows order: wave_only(0)=0, recipe(0)=1, unit(0,0,0)=2 ... unit(0,0,19)=21.
    // Selection unit(0,0,6) is at logical idx 2 + 6 = 8. In [5, 15). Should not move scroll.
    app.ensure_index_visible(10);
    assert_eq!(app.index_scroll, 5);
}

#[test]
fn ensure_index_visible_scrolls_down_when_selection_below_viewport() {
    let g = tall_graph(20);
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.index_scroll = 0;
    // Selection at unit(0,0,15) → logical idx 17. With pane height 10, viewport [0..10). 17 >= 10 → scroll = 17 + 1 - 10 = 8.
    app.selection = Selection::unit(0, 0, 15);
    app.ensure_index_visible(10);
    assert_eq!(app.index_scroll, 8);
}

#[test]
fn ensure_index_visible_scrolls_up_when_selection_above_viewport() {
    let g = tall_graph(20);
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.index_scroll = 10;
    // Selection at unit(0,0,2) → logical idx 4. 4 < 10 → scroll = 4.
    app.selection = Selection::unit(0, 0, 2);
    app.ensure_index_visible(10);
    assert_eq!(app.index_scroll, 4);
}

#[test]
fn ensure_index_visible_clamps_scroll_when_visible_rows_shrink() {
    let g = tall_graph(20);
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    // 22 visible rows, pane 10 → max_scroll = 12.
    app.index_scroll = 50; // bogus large value
    app.selection = Selection::wave_only(0); // logical 0
    app.ensure_index_visible(10);
    // First the "selection above viewport" branch sets scroll = 0 (idx=0 < 50).
    // Then clamp to max_scroll = 12. Result is 0 because 0.min(12) = 0.
    assert_eq!(app.index_scroll, 0);
}

#[test]
fn ensure_index_visible_no_op_when_pane_height_is_zero() {
    let g = tall_graph(20);
    let mut app = AppState::new(&g);
    app.index_scroll = 7;
    app.ensure_index_visible(0);
    assert_eq!(app.index_scroll, 7);
}
