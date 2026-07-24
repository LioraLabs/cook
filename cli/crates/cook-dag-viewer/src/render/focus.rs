//! Selection-driven focus subgraph. See `2026-05-06-dag-tui-always-focused-design.md` §3-4.

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
        None | Some(SelectionLeaf::FilesFolder) => {
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
    // Wave-level and files-folder focus do not expand: they already
    // include every node in the wave.
    if matches!(
        app.selection.leaf,
        None | Some(SelectionLeaf::FilesFolder)
    ) {
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
#[path = "tests/focus_tests.rs"]
mod tests;
