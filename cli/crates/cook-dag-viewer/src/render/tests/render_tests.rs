use super::*;
use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};
use crate::state::{AppState, Selection};

fn unit_node(id: &str, recipe: &str, label: &str) -> NodeData {
    NodeData {
        id: id.into(),
        kind: "unit".into(),
        label: label.into(),
        recipe: Some(recipe.into()),
        command: Some("c".into()),
        output: None,
        cached: Some(true),
        dep_kind: Some("sequential".into()),
        group_index: None,
        modified: None,
        discovered: None,
    }
}

fn three_unit_chain() -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: vec![
                unit_node("unit:a:0", "a", "a0"),
                unit_node("unit:a:1", "a", "a1"),
                unit_node("unit:a:2", "a", "a2"),
            ],
            edges: vec![
                EdgeData { from: "unit:a:0".into(), to: "unit:a:1".into() },
                EdgeData { from: "unit:a:1".into(), to: "unit:a:2".into() },
            ],
        }],
        inter_wave_edges: vec![],
    }
}

#[test]
fn pick_layout_always_runs_focus_filter() {
    let g = three_unit_chain();
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    // Select the middle unit. Focus = a0, a1, a2 (a1 + 1-hop).
    app.selection = Selection::unit(0, 0, 1);
    let layout = pick_layout(&app, &g);
    let ids: std::collections::BTreeSet<&str> =
        layout.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        ids,
        std::collections::BTreeSet::from(["unit:a:0", "unit:a:1", "unit:a:2"])
    );

    // Now select an end unit — focus drops the far one.
    app.selection = Selection::unit(0, 0, 0);
    let layout = pick_layout(&app, &g);
    let ids: std::collections::BTreeSet<&str> =
        layout.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        ids,
        std::collections::BTreeSet::from(["unit:a:0", "unit:a:1"])
    );
}
