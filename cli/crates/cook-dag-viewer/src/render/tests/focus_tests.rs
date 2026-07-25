use super::*;
use crate::dag_data::{EdgeData, EdgeKind, NodeData, WaveData, WaveDagData};
use crate::state::{AppState, Selection};

fn unit(id: &str, recipe: &str, label: &str) -> NodeData {
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

fn file(id: &str, label: &str) -> NodeData {
    NodeData {
        id: id.into(),
        kind: "file".into(),
        label: label.into(),
        recipe: None,
        command: None,
        output: None,
        cached: None,
        dep_kind: None,
        group_index: None,
        modified: Some(false),
        discovered: None,
    }
}

fn small_dag() -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into(), "b".into()],
            nodes: vec![
                file("file:foo.cpp", "foo.cpp"),
                file("file:noise.h", "noise.h"),
                unit("unit:a:0", "a", "a0"),
                unit("unit:b:0", "b", "b0"),
            ],
            edges: vec![
                EdgeData { from: "file:foo.cpp".into(), to: "unit:a:0".into(), kind: EdgeKind::Data },
                EdgeData { from: "file:noise.h".into(), to: "unit:b:0".into(), kind: EdgeKind::Data },
                EdgeData { from: "unit:a:0".into(), to: "unit:b:0".into(), kind: EdgeKind::Data },
            ],
        }],
        inter_wave_edges: vec![],
    }
}

#[test]
fn unit_focus_keeps_only_one_hop_neighborhood() {
    let g = small_dag();
    let mut app = AppState::new(&g);
    // Select unit:a:0 — its 1-hop neighbors are file:foo.cpp and unit:b:0.
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);

    let sub = focus_subgraph(&g, &app);
    let ids: BTreeSet<&str> = sub.waves[0].nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains("unit:a:0"));
    assert!(ids.contains("file:foo.cpp"));
    assert!(ids.contains("unit:b:0"));
    assert!(!ids.contains("file:noise.h"), "unrelated file must be filtered out");
    // The connecting edges land in the synthetic wave.
    let edges: BTreeSet<(&str, &str)> = sub.waves[0]
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    assert!(edges.contains(&("file:foo.cpp", "unit:a:0")));
    assert!(edges.contains(&("unit:a:0", "unit:b:0")));
    assert!(sub.inter_wave_edges.is_empty());
}

fn two_wave_dag() -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![
            WaveData {
                recipes: vec!["a".into()],
                nodes: vec![
                    file("file:foo.cpp", "foo.cpp"),
                    unit("unit:a:0", "a", "a0"),
                ],
                edges: vec![EdgeData {
                    from: "file:foo.cpp".into(),
                    to: "unit:a:0".into(), kind: EdgeKind::Data }],
            },
            WaveData {
                recipes: vec!["b".into()],
                nodes: vec![unit("unit:b:0", "b", "b0")],
                edges: vec![],
            },
        ],
        inter_wave_edges: vec![EdgeData {
            from: "unit:a:0".into(),
            to: "unit:b:0".into(), kind: EdgeKind::Data }],
    }
}

#[test]
fn wave_focus_returns_full_wave_no_inter_wave_edges() {
    let g = two_wave_dag();
    let app = AppState::new(&g);
    // Default: wave_only(0).
    assert_eq!(app.selection, Selection::wave_only(0));
    let sub = focus_subgraph(&g, &app);
    let ids: BTreeSet<&str> = sub.waves[0].nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains("file:foo.cpp"));
    assert!(ids.contains("unit:a:0"));
    assert!(!ids.contains("unit:b:0"), "wave 0 selection must not pull in wave 1 node");
}

#[test]
fn unit_focus_in_wave_0_pulls_inter_wave_neighbor_from_wave_1() {
    let g = two_wave_dag();
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);
    let sub = focus_subgraph(&g, &app);
    let ids: BTreeSet<&str> = sub.waves[0].nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(
        ids.contains("unit:b:0"),
        "1-hop expansion must follow inter-wave edges into wave 1",
    );
    let edges: BTreeSet<(&str, &str)> = sub.waves[0]
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    assert!(
        edges.contains(&("unit:a:0", "unit:b:0")),
        "inter-wave edge must be merged into the synthetic wave",
    );
}

#[test]
fn recipe_focus_pulls_units_plus_one_hop_neighbors() {
    let g = small_dag();
    let mut app = AppState::new(&g);
    // Recipe `a` selected (no unit). Focus = unit:a:0; 1-hop = file:foo.cpp + unit:b:0.
    app.selection = Selection::recipe(0, 0);
    let sub = focus_subgraph(&g, &app);
    let ids: BTreeSet<&str> = sub.waves[0].nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains("unit:a:0"));
    assert!(ids.contains("file:foo.cpp"));
    assert!(ids.contains("unit:b:0"));
    assert!(!ids.contains("file:noise.h"));
}

#[test]
fn file_focus_pulls_consumers() {
    let g = small_dag();
    let mut app = AppState::new(&g);
    // Need the IndexTree to surface foo.cpp in the wave's files.
    // foo.cpp is alphabetically first, so file_index 0.
    assert_eq!(
        app.tree.waves[0].files.get(0).map(|f| f.node_id.as_str()),
        Some("file:foo.cpp"),
    );
    app.selection = Selection::file(0, 0);
    let sub = focus_subgraph(&g, &app);
    let ids: BTreeSet<&str> = sub.waves[0].nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains("file:foo.cpp"));
    assert!(ids.contains("unit:a:0"), "file's consumer must be visible");
    assert!(!ids.contains("unit:b:0"), "non-consumer must be filtered");
    assert!(!ids.contains("file:noise.h"));
}

#[test]
fn focus_subgraph_for_files_folder_matches_wave_only() {
    let g = WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: vec![
                NodeData {
                    id: "file:foo.h".into(),
                    kind: "file".into(),
                    label: "foo.h".into(),
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
            edges: vec![EdgeData {
                from: "file:foo.h".into(),
                to: "unit:a:0".into(), kind: EdgeKind::Data }],
        }],
        inter_wave_edges: vec![],
    };

    let mut wave_app = AppState::new(&g);
    wave_app.selection = Selection::wave_only(0);
    let wave_sub = focus_subgraph(&g, &wave_app);

    let mut folder_app = AppState::new(&g);
    folder_app.selection = Selection::files_folder(0);
    let folder_sub = focus_subgraph(&g, &folder_app);

    assert_eq!(wave_sub.waves.len(), folder_sub.waves.len());

    let wave_node_ids: BTreeSet<&str> =
        wave_sub.waves[0].nodes.iter().map(|n| n.id.as_str()).collect();
    let folder_node_ids: BTreeSet<&str> =
        folder_sub.waves[0].nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(wave_node_ids, folder_node_ids);

    let wave_edges: BTreeSet<(&str, &str)> = wave_sub.waves[0]
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    let folder_edges: BTreeSet<(&str, &str)> = folder_sub.waves[0]
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    assert_eq!(wave_edges, folder_edges);

    assert_eq!(wave_sub.inter_wave_edges.len(), folder_sub.inter_wave_edges.len());
}
