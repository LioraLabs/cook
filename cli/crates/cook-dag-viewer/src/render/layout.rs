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

/// Geometry parameters for the layered layout. The renderer always
/// uses `LayoutDims::FULL`; keeping the parameters in a struct lets
/// the layout engine stay decoupled from the specific values.
#[derive(Debug, Clone, Copy)]
pub struct LayoutDims {
    pub layer_width: u16,
    pub node_w: u16,
    pub node_h: u16,
    pub row_pad: u16,
}

impl LayoutDims {
    pub const FULL: Self = Self { layer_width: 32, node_w: 22, node_h: 3, row_pad: 1 };
}

const MAX_BARYCENTER_ITERS: usize = 24;

/// `(from, to, chain)` triples capturing the original edge endpoints
/// alongside the real-and-dummy node IDs that the polyline must traverse.
pub(crate) type EdgeChain = (String, String, Vec<String>);

/// Output of [`insert_dummies`]: augmented layer table, unit-length edge
/// list, per-original-edge chains, and the set of dummy IDs.
pub(crate) type DummyInsertion = (
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

pub(crate) fn collect_nodes(g: &WaveDagData) -> (BTreeMap<String, NodeData>, Vec<String>) {
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

pub(crate) fn collect_edges(
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
pub(crate) fn assign_layers(
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
pub(crate) fn insert_dummies(
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
pub(crate) fn group_by_layer(
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
pub(crate) fn barycenter_sweeps(
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
/// real-and-dummy nodes between its endpoints. Source anchors on its
/// right edge, target anchors on its left edge, and each dummy
/// contributes BOTH its left-edge and right-edge anchors so the
/// traversal across the dummy's column is a clean horizontal step.
/// Mid-x bends only appear in the gap *between* columns, never inside
/// one — that keeps multi-tier edges from cutting through real nodes.
fn route_chain(
    chain: &[String],
    positions: &BTreeMap<String, (u16, u16)>,
    dims: LayoutDims,
) -> Option<Vec<(u16, u16)>> {
    if chain.len() < 2 {
        return None;
    }

    let mut controls: Vec<(u16, u16)> = Vec::with_capacity(chain.len() * 2);
    let from_pos = positions.get(&chain[0]).copied()?;
    controls.push((from_pos.0 + dims.node_w, from_pos.1 + dims.node_h / 2));
    for id in &chain[1..chain.len() - 1] {
        let (x, y) = positions.get(id).copied()?;
        let row_y = y + dims.node_h / 2;
        controls.push((x, row_y));
        controls.push((x + dims.node_w, row_y));
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
#[path = "tests/layout_tests.rs"]
mod tests;
