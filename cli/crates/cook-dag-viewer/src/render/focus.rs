//! Compact mode focus subgraph. See spec §3.

use std::collections::BTreeSet;

use crate::dag_data::{EdgeData, NodeData, WaveDagData, WaveData};
use crate::state::{AppState, SelectionLeaf};

/// Build a single-wave subgraph containing the focus set (derived from
/// `app.selection`) plus its 1-hop expansion plus every edge connecting
/// two visible nodes. Cross-wave edges that touch the focus set are
/// merged into the synthetic wave.
pub fn focus_subgraph(graph: &WaveDagData, app: &AppState) -> WaveDagData {
    let focus = focus_set(graph, app);
    let visible = expand_one_hop(graph, &focus, app);
    build_subgraph(graph, &visible)
}

fn focus_set(graph: &WaveDagData, app: &AppState) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let wave_idx = app.selection.wave;
    let Some(wave) = graph.waves.get(wave_idx) else {
        return out;
    };
    match app.selection.leaf {
        None => {
            for n in &wave.nodes {
                out.insert(n.id.clone());
            }
        }
        Some(SelectionLeaf::Recipe { recipe, unit }) => {
            let recipe_name = wave.recipes.get(recipe).cloned();
            if let Some(name) = recipe_name {
                if let Some(u) = unit {
                    // Authoritative unit id from the IndexTree, not a format!()
                    // that mirrors dag_data's encoding. Two sources of truth
                    // would silently diverge if the encoding ever changed.
                    if let Some(node_id) = app
                        .tree
                        .waves
                        .get(wave_idx)
                        .and_then(|w| w.recipes.get(recipe))
                        .and_then(|r| r.units.get(u))
                        .map(|u_row| u_row.node_id.clone())
                    {
                        out.insert(node_id);
                    }
                } else {
                    for n in &wave.nodes {
                        if n.kind == "unit" && n.recipe.as_deref() == Some(&name) {
                            out.insert(n.id.clone());
                        }
                    }
                }
            }
        }
        Some(SelectionLeaf::File(fi)) => {
            if let Some(file) = app.tree.waves.get(wave_idx).and_then(|w| w.files.get(fi)) {
                out.insert(file.node_id.clone());
            }
        }
    }
    out
}

fn expand_one_hop(
    graph: &WaveDagData,
    focus: &BTreeSet<String>,
    app: &AppState,
) -> BTreeSet<String> {
    let mut visible = focus.clone();
    // Wave-level focus does not expand: spec §3.1.
    if app.selection.leaf.is_none() {
        return visible;
    }
    for wave in &graph.waves {
        for e in &wave.edges {
            if focus.contains(&e.from) {
                visible.insert(e.to.clone());
            }
            if focus.contains(&e.to) {
                visible.insert(e.from.clone());
            }
        }
    }
    for e in &graph.inter_wave_edges {
        if focus.contains(&e.from) {
            visible.insert(e.to.clone());
        }
        if focus.contains(&e.to) {
            visible.insert(e.from.clone());
        }
    }
    visible
}

fn build_subgraph(graph: &WaveDagData, visible: &BTreeSet<String>) -> WaveDagData {
    let mut nodes: Vec<NodeData> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut recipes: BTreeSet<String> = BTreeSet::new();
    for wave in &graph.waves {
        for n in &wave.nodes {
            if visible.contains(&n.id) && seen.insert(n.id.clone()) {
                if let Some(r) = &n.recipe {
                    recipes.insert(r.clone());
                }
                nodes.push(n.clone());
            }
        }
    }
    let mut edges: Vec<EdgeData> = Vec::new();
    let mut edge_seen: BTreeSet<(String, String)> = BTreeSet::new();
    let push = |from: &str, to: &str,
                    edges: &mut Vec<EdgeData>,
                    seen: &mut BTreeSet<(String, String)>| {
        if !visible.contains(from) || !visible.contains(to) {
            return;
        }
        let k = (from.to_string(), to.to_string());
        if seen.insert(k) {
            edges.push(EdgeData { from: from.to_string(), to: to.to_string() });
        }
    };
    for wave in &graph.waves {
        for e in &wave.edges {
            push(&e.from, &e.to, &mut edges, &mut edge_seen);
        }
    }
    for e in &graph.inter_wave_edges {
        push(&e.from, &e.to, &mut edges, &mut edge_seen);
    }

    WaveDagData {
        schema_version: graph.schema_version,
        target: graph.target.clone(),
        waves: vec![WaveData {
            recipes: recipes.into_iter().collect(),
            nodes,
            edges,
        }],
        inter_wave_edges: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};
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
                    EdgeData { from: "file:foo.cpp".into(), to: "unit:a:0".into() },
                    EdgeData { from: "file:noise.h".into(), to: "unit:b:0".into() },
                    EdgeData { from: "unit:a:0".into(), to: "unit:b:0".into() },
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
}
