use super::*;
use crate::dag_data::{NodeData, WaveData, WaveDagData};
use crate::frame::SnapshotFrame;
use crate::render::layout;
use crate::state::{AppState, Selection};

fn dag() -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: vec![NodeData {
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
            }],
            edges: vec![],
        }],
        inter_wave_edges: vec![],
    }
}

#[test]
fn renders_node_box_with_label_and_badge() {
    let g = dag();
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);
    let frame = SnapshotFrame::new(g.clone());
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let buf = render(&layout, &app, &frame);

    let placed = layout.nodes.iter().find(|n| n.id == "unit:a:0").unwrap();
    // Top-left corner of the node box.
    let tl = buf.cell((placed.x, placed.y)).unwrap();
    assert_eq!(tl.symbol(), "┌");
    // Badge in the top-right area.
    let badge_x = placed.x + placed.w.saturating_sub(2);
    let badge_cell = buf.cell((badge_x, placed.y)).unwrap();
    assert_eq!(badge_cell.symbol().chars().next(), Some('✓'));
}

#[test]
fn selection_overlay_applies_reverse_video() {
    let g = dag();
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);
    let frame = SnapshotFrame::new(g.clone());
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let buf = render(&layout, &app, &frame);
    let placed = layout.nodes.iter().find(|n| n.id == "unit:a:0").unwrap();
    let cell = buf.cell((placed.x + 1, placed.y + 1)).unwrap();
    assert!(cell.style().add_modifier.contains(Modifier::REVERSED));
}

use crate::dag_data::{EdgeData, EdgeKind};

fn dag_with_discovered_file() -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: vec![
                NodeData {
                    id: "file:helpers.h".into(),
                    kind: "file".into(),
                    label: "helpers.h".into(),
                    recipe: None,
                    command: None,
                    output: None,
                    cached: None,
                    dep_kind: None,
                    group_index: None,
                    modified: None,
                    discovered: Some(true),
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
            edges: vec![EdgeData {
                from: "file:helpers.h".into(),
                to: "unit:a:0".into(), kind: EdgeKind::Data }],
        }],
        inter_wave_edges: vec![],
    }
}

#[test]
fn discovered_file_node_uses_rounded_border() {
    let g = dag_with_discovered_file();
    let app = AppState::new(&g);
    let frame = SnapshotFrame::new(g.clone());
    let layout = layout::compute(&g, layout::LayoutDims::FULL);
    let buf = render(&layout, &app, &frame);

    let helpers = layout.nodes.iter().find(|n| n.id == "file:helpers.h").unwrap();
    let tl = buf.cell((helpers.x, helpers.y)).unwrap();
    assert_eq!(tl.symbol(), "╭", "discovered file should use rounded top-left corner");

        let unit = layout.nodes.iter().find(|n| n.id == "unit:a:0").unwrap();
    let unit_tl = buf.cell((unit.x, unit.y)).unwrap();
    assert_eq!(unit_tl.symbol(), "┌", "unit should keep plain top-left corner");
    }

    #[test]
    fn discovered_file_node_renders_tilde_badge() {
        let g = dag_with_discovered_file();
        let app = AppState::new(&g);
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FULL);
        let buf = render(&layout, &app, &frame);

        let helpers = layout.nodes.iter().find(|n| n.id == "file:helpers.h").unwrap();
    let badge_x = helpers.x + helpers.w.saturating_sub(2);
    let badge_cell = buf.cell((badge_x, helpers.y)).unwrap();
    assert_eq!(badge_cell.symbol(), "~");
}
