//! Sugiyama-style layered layout.
//!
//! Pipeline (Sugiyama, Tagawa, Toda — 1981):
//!   1. **Layer assignment.** Longest-path layering from sources via a
//!      Kahn-style topological sweep. Sources sit at layer 0; every other
//!      node sits one layer past its deepest predecessor.
//!   2. **Dummy-node insertion.** Edges that span more than one layer are
//!      broken into unit-length segments so the crossing-reduction pass
//!      sees a proper layered graph.
//!   3. **Crossing reduction.** Alternating top-down / bottom-up
//!      barycenter sweeps, capped at `MAX_BARYCENTER_ITERS` or until the
//!      ordering stabilises.
//!   4. **Coordinate assignment.** Uniform grid: `x = layer * layer_width`,
//!      `y = row * (node_h + row_pad)`.
//!   5. **Edge routing.** Orthogonal polyline from right-anchor of source,
//!      through each dummy centre, to left-anchor of target, with mid-x
//!      bends inserted between control points whose `y` differs.
//!
//! The wave grouping that drives the underlying snapshot still shapes the
//! result — sources (file nodes) land in layer 0, units fed by them in
//! layer 1, and inter-wave dependencies push downstream waves further to
//! the right — but the layout no longer treats waves as opaque columns.

use std::collections::{BTreeMap, BTreeSet};

use crate::dag_data::{NodeData, WaveDagData};

/// Geometry preset for one density mode. The renderer picks one of
/// `LayoutDims::FULL`, `LayoutDims::COMPACT`, `LayoutDims::DOT`, or
/// `LayoutDims::FLOW`
/// based on `AppState.density`. See spec §4.2 / §5.2.
#[derive(Debug, Clone, Copy)]
pub struct LayoutDims {
    pub layer_width: u16,
    pub node_w: u16,
    pub node_h: u16,
    pub row_pad: u16,
}

impl LayoutDims {
    pub const FULL: Self    = Self { layer_width: 32, node_w: 22, node_h: 3, row_pad: 1 };
    pub const COMPACT: Self = Self { layer_width: 22, node_w: 18, node_h: 1, row_pad: 1 };
    pub const DOT: Self     = Self { layer_width:  3, node_w:  1, node_h: 1, row_pad: 0 };
    pub const FLOW: Self    = Self { layer_width:  6, node_w:  2, node_h: 2, row_pad: 1 };
}

const MAX_BARYCENTER_ITERS: usize = 24;

/// `(from, to, chain)` triples capturing the original edge endpoints
/// alongside the real-and-dummy node IDs that the polyline must traverse.
type EdgeChain = (String, String, Vec<String>);

/// Output of [`insert_dummies`]: augmented layer table, unit-length edge
/// list, per-original-edge chains, and the set of dummy IDs.
type DummyInsertion = (
    BTreeMap<String, usize>,
    Vec<(String, String)>,
    Vec<EdgeChain>,
    BTreeSet<String>,
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedNode {
    pub id: String,
    pub kind: String, // "file" or "unit"
    pub label: String,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub discovered: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeRoute {
    pub from: String,
    pub to: String,
    pub points: Vec<(u16, u16)>, // orthogonal polyline
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub nodes: Vec<PlacedNode>,
    pub edges: Vec<EdgeRoute>,
    pub canvas_w: u16,
    pub canvas_h: u16,
}

pub fn compute(g: &WaveDagData, dims: LayoutDims) -> Layout {
    let (real_nodes, ordered_ids) = collect_nodes(g);
    if ordered_ids.is_empty() {
        return Layout {
            nodes: Vec::new(),
            edges: Vec::new(),
            canvas_w: dims.layer_width,
            canvas_h: 0,
        };
    }
    let real_edges = collect_edges(g, &real_nodes);
    let real_layers = assign_layers(&ordered_ids, &real_edges);
    let (layers, chain_edges, chains, dummies) = insert_dummies(&real_layers, &real_edges);

    let mut order = group_by_layer(&layers, &ordered_ids, &dummies);
    barycenter_sweeps(&mut order, &chain_edges);

    let positions = assign_coordinates(&order, dims);
    let canvas_w = canvas_width(&order, dims);
    let canvas_h = canvas_height(&order, dims);

    let placed_nodes: Vec<PlacedNode> = ordered_ids
        .iter()
        .map(|id| {
            let n = &real_nodes[id];
            let (x, y) = positions[id];
            PlacedNode {
                id: id.clone(),
                kind: n.kind.clone(),
                label: n.label.clone(),
                x,
                y,
                w: dims.node_w,
                h: dims.node_h,
                discovered: n.discovered,
            }
        })
        .collect();

    let edges: Vec<EdgeRoute> = chains
        .iter()
        .filter_map(|(from, to, chain)| {
            route_chain(chain, &positions, dims).map(|points| EdgeRoute {
                from: from.clone(),
                to: to.clone(),
                points,
            })
        })
        .collect();

    Layout { nodes: placed_nodes, edges, canvas_w, canvas_h }
}

// ---------------------------------------------------------------------------
// Pipeline stages
// ---------------------------------------------------------------------------

fn collect_nodes(g: &WaveDagData) -> (BTreeMap<String, NodeData>, Vec<String>) {
    let mut nodes = BTreeMap::new();
    let mut order = Vec::new();
    for wave in &g.waves {
        for n in &wave.nodes {
            if !nodes.contains_key(&n.id) {
                nodes.insert(n.id.clone(), n.clone());
                order.push(n.id.clone());
            }
        }
    }
    (nodes, order)
}

fn collect_edges(
    g: &WaveDagData,
    nodes: &BTreeMap<String, NodeData>,
) -> Vec<(String, String)> {
    let mut edges = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut push = |from: &String, to: &String, edges: &mut Vec<(String, String)>| {
        if !nodes.contains_key(from) || !nodes.contains_key(to) {
            return;
        }
        if from == to {
            return;
        }
        if seen.insert((from.clone(), to.clone())) {
            edges.push((from.clone(), to.clone()));
        }
    };
    for wave in &g.waves {
        for e in &wave.edges {
            push(&e.from, &e.to, &mut edges);
        }
    }
    for e in &g.inter_wave_edges {
        push(&e.from, &e.to, &mut edges);
    }
    edges
}

/// Longest-path layering. Cycle-tolerant — any node not reached by the
/// topological sweep falls back to layer 0.
fn assign_layers(
    ids: &[String],
    edges: &[(String, String)],
) -> BTreeMap<String, usize> {
    let mut indeg: BTreeMap<String, usize> =
        ids.iter().map(|s| (s.clone(), 0_usize)).collect();
    let mut succs: BTreeMap<String, Vec<String>> =
        ids.iter().map(|s| (s.clone(), Vec::new())).collect();
    for (from, to) in edges {
        if let Some(d) = indeg.get_mut(to) {
            *d += 1;
        }
        if let Some(v) = succs.get_mut(from) {
            v.push(to.clone());
        }
    }

    let mut layer: BTreeMap<String, usize> = BTreeMap::new();
    let mut remaining = indeg.clone();
    let mut work: Vec<String> = ids
        .iter()
        .filter(|s| indeg[*s] == 0)
        .cloned()
        .collect();
    for v in &work {
        layer.insert(v.clone(), 0);
    }

    while let Some(v) = work.pop() {
        let lv = *layer.get(&v).unwrap_or(&0);
        let next_layer = lv + 1;
        if let Some(children) = succs.get(&v) {
            for s in children {
                let entry = layer.entry(s.clone()).or_insert(0);
                if next_layer > *entry {
                    *entry = next_layer;
                }
                if let Some(rd) = remaining.get_mut(s) {
                    *rd -= 1;
                    if *rd == 0 {
                        work.push(s.clone());
                    }
                }
            }
        }
    }

    for id in ids {
        layer.entry(id.clone()).or_insert(0);
    }
    layer
}

/// Break edges spanning more than one layer into chains of unit-length
/// edges joined by virtual "dummy" nodes. Returns the augmented layer
/// table, the unit-length edge list (used by crossing reduction), the
/// per-original-edge chain (used by edge routing), and the set of dummy
/// IDs (which participate in ordering but are not rendered as boxes).
fn insert_dummies(
    real_layers: &BTreeMap<String, usize>,
    real_edges: &[(String, String)],
) -> DummyInsertion {
    let mut layers = real_layers.clone();
    let mut chain_edges: Vec<(String, String)> = Vec::new();
    let mut chains: Vec<EdgeChain> = Vec::new();
    let mut dummies: BTreeSet<String> = BTreeSet::new();
    let mut next_id = 0_usize;

    for (from, to) in real_edges {
        let lf = layers[from];
        let lt = layers[to];
        if lt <= lf + 1 {
            chain_edges.push((from.clone(), to.clone()));
            chains.push((from.clone(), to.clone(), vec![from.clone(), to.clone()]));
            continue;
        }
        let mut chain: Vec<String> = Vec::with_capacity(lt - lf + 1);
        chain.push(from.clone());
        for k in (lf + 1)..lt {
            let id = format!("__dummy_{}", next_id);
            next_id += 1;
            layers.insert(id.clone(), k);
            dummies.insert(id.clone());
            chain.push(id);
        }
        chain.push(to.clone());
        for w in chain.windows(2) {
            chain_edges.push((w[0].clone(), w[1].clone()));
        }
        chains.push((from.clone(), to.clone(), chain));
    }
    (layers, chain_edges, chains, dummies)
}

/// Group node IDs by layer, preserving real-node insertion order and
/// appending dummies at the end of their layers.
fn group_by_layer(
    layers: &BTreeMap<String, usize>,
    ordered_real: &[String],
    dummies: &BTreeSet<String>,
) -> BTreeMap<usize, Vec<String>> {
    let mut groups: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let max_layer = layers.values().max().copied().unwrap_or(0);
    for k in 0..=max_layer {
        groups.entry(k).or_default();
    }
    for id in ordered_real {
        groups.entry(layers[id]).or_default().push(id.clone());
    }
    for id in dummies {
        groups.entry(layers[id]).or_default().push(id.clone());
    }
    groups
}

/// Alternating top-down / bottom-up barycenter sweeps. Stops early when a
/// full round-trip leaves every layer unchanged.
fn barycenter_sweeps(
    order: &mut BTreeMap<usize, Vec<String>>,
    edges: &[(String, String)],
) {
    if order.len() < 2 {
        return;
    }

    let mut succs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut preds: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (from, to) in edges {
        succs.entry(from.clone()).or_default().push(to.clone());
        preds.entry(to.clone()).or_default().push(from.clone());
    }

    let layer_keys: Vec<usize> = order.keys().copied().collect();

    for _ in 0..MAX_BARYCENTER_ITERS {
        let mut changed = false;
        // Top-down: order each layer by mean-index of its predecessors.
        for win in layer_keys.windows(2) {
            let (prev, cur) = (win[0], win[1]);
            if reorder_by_barycenter(order, prev, cur, &preds) {
                changed = true;
            }
        }
        // Bottom-up: order each layer by mean-index of its successors.
        for win in layer_keys.windows(2).rev() {
            let (cur, nxt) = (win[0], win[1]);
            if reorder_by_barycenter(order, nxt, cur, &succs) {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

fn reorder_by_barycenter(
    order: &mut BTreeMap<usize, Vec<String>>,
    reference_layer: usize,
    target_layer: usize,
    neighbours: &BTreeMap<String, Vec<String>>,
) -> bool {
    let ref_index: BTreeMap<String, usize> = order[&reference_layer]
        .iter()
        .enumerate()
        .map(|(i, s)| (s.clone(), i))
        .collect();
    let target = order[&target_layer].clone();
    let mut keyed: Vec<(f64, usize, String)> = target
        .iter()
        .enumerate()
        .map(|(orig_i, id)| {
            let bary = neighbours
                .get(id)
                .map(|ns| {
                    let (sum, count) = ns.iter().fold((0.0_f64, 0_usize), |(s, c), n| {
                        match ref_index.get(n) {
                            Some(&i) => (s + i as f64, c + 1),
                            None => (s, c),
                        }
                    });
                    if count == 0 {
                        orig_i as f64
                    } else {
                        sum / count as f64
                    }
                })
                .unwrap_or(orig_i as f64);
            (bary, orig_i, id.clone())
        })
        .collect();
    keyed.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    let new_order: Vec<String> = keyed.into_iter().map(|(_, _, id)| id).collect();
    let changed = new_order != target;
    order.insert(target_layer, new_order);
    changed
}

fn assign_coordinates(
    order: &BTreeMap<usize, Vec<String>>,
    dims: LayoutDims,
) -> BTreeMap<String, (u16, u16)> {
    let mut out = BTreeMap::new();
    for (layer_idx, ids) in order {
        let x = (*layer_idx as u16) * dims.layer_width;
        for (i, id) in ids.iter().enumerate() {
            let y = (i as u16) * (dims.node_h + dims.row_pad);
            out.insert(id.clone(), (x, y));
        }
    }
    out
}

fn canvas_width(order: &BTreeMap<usize, Vec<String>>, dims: LayoutDims) -> u16 {
    let max_layer = order.keys().max().copied().unwrap_or(0) as u16;
    (max_layer + 1).saturating_mul(dims.layer_width)
}

fn canvas_height(order: &BTreeMap<usize, Vec<String>>, dims: LayoutDims) -> u16 {
    order
        .values()
        .map(|ids| ids.len() as u16)
        .max()
        .unwrap_or(0)
        .saturating_mul(dims.node_h + dims.row_pad)
}

/// Stitch the polyline for an original edge by walking the chain of
/// real-and-dummy nodes between its endpoints. Source and target anchor
/// on the right and left edges of their boxes; dummies are passed through
/// at their centres. Mid-x bends are inserted between control points
/// whose y-coordinate differs, keeping every segment orthogonal.
fn route_chain(
    chain: &[String],
    positions: &BTreeMap<String, (u16, u16)>,
    dims: LayoutDims,
) -> Option<Vec<(u16, u16)>> {
    if chain.len() < 2 {
        return None;
    }

    let mut controls: Vec<(u16, u16)> = Vec::with_capacity(chain.len());
    let from_pos = positions.get(&chain[0]).copied()?;
    controls.push((from_pos.0 + dims.node_w, from_pos.1 + dims.node_h / 2));
    for id in &chain[1..chain.len() - 1] {
        let (x, y) = positions.get(id).copied()?;
        controls.push((x + dims.node_w / 2, y + dims.node_h / 2));
    }
    let to_pos = positions.get(&chain[chain.len() - 1]).copied()?;
    controls.push((to_pos.0, to_pos.1 + dims.node_h / 2));

    let mut points: Vec<(u16, u16)> = Vec::with_capacity(controls.len() * 2);
    points.push(controls[0]);
    for w in controls.windows(2) {
        let (x1, y1) = w[0];
        let (x2, y2) = w[1];
        if x1 == x2 || y1 == y2 {
            points.push((x2, y2));
        } else {
            let mid_x = midpoint(x1, x2);
            points.push((mid_x, y1));
            points.push((mid_x, y2));
            points.push((x2, y2));
        }
    }
    Some(points)
}

fn midpoint(a: u16, b: u16) -> u16 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    lo + (hi - lo) / 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};

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

    fn layer_of(l: &Layout, id: &str) -> u16 {
        let n = l.nodes.iter().find(|n| n.id == id).unwrap();
        n.x / LayoutDims::FULL.layer_width
    }

    #[test]
    fn longest_path_layers_a_chain_left_to_right() {
        let g = WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![
                WaveData {
                    recipes: vec!["a".into()],
                    nodes: vec![file("file:foo", "foo"), unit("unit:a:0", "a", "a0")],
                    edges: vec![EdgeData {
                        from: "file:foo".into(),
                        to: "unit:a:0".into(),
                    }],
                },
                WaveData {
                    recipes: vec!["b".into()],
                    nodes: vec![unit("unit:b:0", "b", "b0")],
                    edges: vec![],
                },
            ],
            inter_wave_edges: vec![EdgeData {
                from: "unit:a:0".into(),
                to: "unit:b:0".into(),
            }],
        };
        let l = compute(&g, LayoutDims::FULL);
        assert_eq!(layer_of(&l, "file:foo"), 0);
        assert_eq!(layer_of(&l, "unit:a:0"), 1);
        assert_eq!(layer_of(&l, "unit:b:0"), 2);
        assert!(l.canvas_w >= 3 * LayoutDims::FULL.layer_width);
    }

    #[test]
    fn long_edges_get_dummy_nodes_for_routing() {
        // file → unit:a → unit:c, plus unit:b at the same layer as unit:a
        // forces unit:c to layer 2 and the (file → unit:c) edge to span
        // two layers (gets one dummy).
        let g = WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["r".into()],
                nodes: vec![
                    file("file:in", "in"),
                    unit("unit:r:0", "r", "a"),
                    unit("unit:r:1", "r", "c"),
                ],
                edges: vec![
                    EdgeData { from: "file:in".into(), to: "unit:r:0".into() },
                    EdgeData { from: "unit:r:0".into(), to: "unit:r:1".into() },
                    EdgeData { from: "file:in".into(), to: "unit:r:1".into() },
                ],
            }],
            inter_wave_edges: vec![],
        };
        let l = compute(&g, LayoutDims::FULL);
        assert_eq!(layer_of(&l, "file:in"), 0);
        assert_eq!(layer_of(&l, "unit:r:0"), 1);
        assert_eq!(layer_of(&l, "unit:r:1"), 2);
        let long = l
            .edges
            .iter()
            .find(|e| e.from == "file:in" && e.to == "unit:r:1")
            .expect("file→unit:r:1 should be routed");
        // A chain through one dummy plus a mid-x bend gives ≥ 4 control
        // points; pure horizontal straight-shot would give 2.
        assert!(
            long.points.len() >= 3,
            "long edge should bend through dummy positions, got {:?}",
            long.points,
        );
    }

    #[test]
    fn routes_short_edge_with_orthogonal_polyline() {
        let g = WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![file("file:foo", "foo"), unit("unit:a:0", "a", "a0")],
                edges: vec![EdgeData {
                    from: "file:foo".into(),
                    to: "unit:a:0".into(),
                }],
            }],
            inter_wave_edges: vec![],
        };
        let l = compute(&g, LayoutDims::FULL);
        assert_eq!(l.edges.len(), 1);
        let route = &l.edges[0].points;
        // Either a straight horizontal shot (2 points) or one mid-x bend
        // (4 points) — never diagonal.
        assert!(route.len() == 2 || route.len() == 4);
        for w in route.windows(2) {
            assert!(
                w[0].0 == w[1].0 || w[0].1 == w[1].1,
                "segment {:?}→{:?} not orthogonal",
                w[0],
                w[1],
            );
        }
    }

    #[test]
    fn barycenter_reduces_crossings_between_two_layers() {
        // Two recipes in one wave whose units are wired to two shared
        // file inputs in a crossing pattern. After barycenter sweep the
        // file order in layer 0 should track the unit order in layer 1.
        let g = WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["x".into(), "y".into()],
                nodes: vec![
                    file("file:f1", "f1"),
                    file("file:f2", "f2"),
                    unit("unit:x:0", "x", "x0"),
                    unit("unit:y:0", "y", "y0"),
                ],
                edges: vec![
                    EdgeData { from: "file:f2".into(), to: "unit:x:0".into() },
                    EdgeData { from: "file:f1".into(), to: "unit:y:0".into() },
                ],
            }],
            inter_wave_edges: vec![],
        };
        let l = compute(&g, LayoutDims::FULL);
        let f1 = l.nodes.iter().find(|n| n.id == "file:f1").unwrap();
        let f2 = l.nodes.iter().find(|n| n.id == "file:f2").unwrap();
        let x0 = l.nodes.iter().find(|n| n.id == "unit:x:0").unwrap();
        let y0 = l.nodes.iter().find(|n| n.id == "unit:y:0").unwrap();
        // Files are in the same layer, units in the next layer. After
        // crossing reduction, f2 should be aligned with x0 and f1 with
        // y0 — i.e. the (f1,f2) order matches the (y0,x0) order on y.
        assert_eq!(f1.x, f2.x, "files share a layer");
        assert_eq!(x0.x, y0.x, "units share a layer");
        let f1_first = f1.y < f2.y;
        let y0_first = y0.y < x0.y;
        assert_eq!(
            f1_first, y0_first,
            "barycenter should align file order with unit order to remove crossings",
        );
    }

    #[test]
    fn empty_dag_returns_empty_layout() {
        let g = WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![],
            inter_wave_edges: vec![],
        };
        let l = compute(&g, LayoutDims::FULL);
        assert!(l.nodes.is_empty());
        assert!(l.edges.is_empty());
    }

    #[test]
    fn layout_flow_preset_uses_2x2_node_with_stride_6() {
        let dims = LayoutDims::FLOW;
        assert_eq!(dims.layer_width, 6);
        assert_eq!(dims.node_w, 2);
        assert_eq!(dims.node_h, 2);
        assert_eq!(dims.row_pad, 1);
    }
}
