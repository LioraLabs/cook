//! Radial polar coordinate variant of the Sugiyama layout. See spec
//! §4.3. Same pipeline as render::layout for stages 1–3; stage 4
//! emits polar (radius, angle) → (x, y).

use std::collections::BTreeMap;
use std::f64::consts::TAU;

use crate::dag_data::WaveDagData;
use crate::render::layout::{
    self, EdgeRoute, Layout, LayoutDims, PlacedNode,
};

pub fn compute(g: &WaveDagData, dims: LayoutDims) -> Layout {
    let (real_nodes, ordered_ids) = layout::collect_nodes(g);
    if ordered_ids.is_empty() {
        return Layout {
            nodes: Vec::new(),
            edges: Vec::new(),
            canvas_w: dims.layer_width,
            canvas_h: dims.layer_width,
        };
    }
    let real_edges = layout::collect_edges(g, &real_nodes);
    let real_layers = layout::assign_layers(&ordered_ids, &real_edges);
    let (layers, _chain_edges, _chains, dummies) =
        layout::insert_dummies(&real_layers, &real_edges);

    // Group real nodes by layer, preserving insertion order.
    let mut by_layer: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for id in &ordered_ids {
        by_layer.entry(layers[id]).or_default().push(id.clone());
    }
    // Ensure dummy layers are also present (no nodes in them, but the
    // BTreeMap iteration covers every layer index).
    for id in &dummies {
        by_layer.entry(layers[id]).or_default();
    }
    let max_layer = layers.values().max().copied().unwrap_or(0);

    // Ring pitch ~= 2× the rectangular layer width, so each ring has
    // breathing room. Outer radius spans (max_layer + 1) rings.
    let ring_pitch = (dims.layer_width as f64) * 2.0;
    let outer_radius = ring_pitch * (max_layer as f64 + 1.0);
    let canvas_side = ((outer_radius * 2.0).ceil() as u16).max(dims.layer_width * 2);
    let center_x = canvas_side as f64 / 2.0;
    let center_y = canvas_side as f64 / 2.0;

    let mut placed: Vec<PlacedNode> = Vec::with_capacity(ordered_ids.len());
    for (&layer_idx, members) in &by_layer {
        // Target (max layer) at center, sources (layer 0) outermost.
        let radius = ring_pitch * ((max_layer - layer_idx) as f64);
        let count = members.len() as f64;
        for (i, id) in members.iter().enumerate() {
            let angle = if count > 1.0 {
                (i as f64 / count) * TAU
            } else {
                0.0
            };
            let x_f = center_x + radius * angle.cos();
            let y_f = center_y + radius * angle.sin();
            let n = &real_nodes[id];
            placed.push(PlacedNode {
                id: id.clone(),
                kind: n.kind.clone(),
                label: n.label.clone(),
                x: x_f.round().max(0.0) as u16,
                y: y_f.round().max(0.0) as u16,
                w: dims.node_w,
                h: dims.node_h,
                discovered: n.discovered,
            });
        }
    }

    // Edge polylines: not used by the Flow renderer, but keep
    // `Layout.edges` populated as straight 2-point routes between
    // real-node centers so other consumers (search, debug dumps) keep
    // working. A future spline-edge follow-up can adopt them.
    let placed_by_id: BTreeMap<&str, &PlacedNode> =
        placed.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut edges: Vec<EdgeRoute> = Vec::new();
    for (from, to) in &real_edges {
        if let (Some(s), Some(t)) =
            (placed_by_id.get(from.as_str()), placed_by_id.get(to.as_str()))
        {
            edges.push(EdgeRoute {
                from: from.clone(),
                to: to.clone(),
                points: vec![
                    (s.x + s.w / 2, s.y + s.h / 2),
                    (t.x + t.w / 2, t.y + t.h / 2),
                ],
            });
        }
    }

    Layout {
        nodes: placed,
        edges,
        canvas_w: canvas_side,
        canvas_h: canvas_side,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};
    use crate::render::layout::LayoutDims;

    fn linear_dag() -> WaveDagData {
        // 3 layers: a → b → c (target).
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "c".into(),
            waves: vec![WaveData {
                recipes: vec!["r".into()],
                nodes: vec![
                    node("a"), node("b"), node("c"),
                ],
                edges: vec![
                    EdgeData { from: "a".into(), to: "b".into() },
                    EdgeData { from: "b".into(), to: "c".into() },
                ],
            }],
            inter_wave_edges: vec![],
        }
    }

    fn node(id: &str) -> NodeData {
        NodeData {
            id: id.into(),
            kind: "unit".into(),
            label: id.into(),
            recipe: Some("r".into()),
            command: Some("c".into()),
            output: None,
            cached: Some(true),
            dep_kind: Some("sequential".into()),
            group_index: None,
            modified: None,
            discovered: None,
        }
    }

    #[test]
    fn target_node_sits_at_canvas_center() {
        let g = linear_dag();
        let l = compute(&g, LayoutDims::FLOW);
        let center_x = l.canvas_w / 2;
        let center_y = l.canvas_h / 2;
        let target = l.nodes.iter().find(|n| n.id == "c").unwrap();
        let dx = (target.x as i32 - center_x as i32).abs();
        let dy = (target.y as i32 - center_y as i32).abs();
        assert!(dx <= 1, "target.x within ±1 of canvas center");
        assert!(dy <= 1, "target.y within ±1 of canvas center");
    }

    #[test]
    fn outer_layer_radius_exceeds_inner_layer_radius() {
        let g = linear_dag();
        let l = compute(&g, LayoutDims::FLOW);
        let center_x = l.canvas_w as f64 / 2.0;
        let center_y = l.canvas_h as f64 / 2.0;
        let dist = |id: &str| {
            let n = l.nodes.iter().find(|n| n.id == id).unwrap();
            ((n.x as f64 - center_x).powi(2) + (n.y as f64 - center_y).powi(2)).sqrt()
        };
        let r_a = dist("a");
        let r_b = dist("b");
        let r_c = dist("c");
        // c is at center (smallest), b on next ring, a on outer ring.
        assert!(r_c < r_b, "target ring is innermost");
        assert!(r_b < r_a, "source ring is outermost");
    }
}
