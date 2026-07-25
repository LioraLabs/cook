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
                EdgeData { from: "file:in".into(), to: "unit:r:0".into(), kind: EdgeKind::Data },
                EdgeData { from: "unit:r:0".into(), to: "unit:r:1".into(), kind: EdgeKind::Data },
                EdgeData { from: "file:in".into(), to: "unit:r:1".into(), kind: EdgeKind::Data },
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
                to: "unit:a:0".into(), kind: EdgeKind::Data }],
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
                EdgeData { from: "file:f2".into(), to: "unit:x:0".into(), kind: EdgeKind::Data },
                EdgeData { from: "file:f1".into(), to: "unit:y:0".into(), kind: EdgeKind::Data },
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
fn multi_tier_edge_bends_in_gaps_not_inside_a_layer_column() {
    // file:in spans 2 layers to unit:r:1, with unit:r:0 occupying
    // layer 1 row 0 in the same column the dummy passes through.
    // The polyline must not cross unit:r:0's bounding box: every
    // bend must sit in the gap between two columns, never inside
    // the [layer_x, layer_x + node_w) span of any layer.
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
                EdgeData { from: "file:in".into(), to: "unit:r:0".into(), kind: EdgeKind::Data },
                EdgeData { from: "unit:r:0".into(), to: "unit:r:1".into(), kind: EdgeKind::Data },
                EdgeData { from: "file:in".into(), to: "unit:r:1".into(), kind: EdgeKind::Data },
            ],
        }],
        inter_wave_edges: vec![],
    };
    let l = compute(&g, LayoutDims::FULL);
    let dims = LayoutDims::FULL;
    let long = l
        .edges
        .iter()
        .find(|e| e.from == "file:in" && e.to == "unit:r:1")
        .expect("file→unit:r:1 should be routed");

    // Every horizontal segment that doesn't sit on a column anchor
    // (the source's right edge or target's left edge) must run only
    // in the gap between layers. A bend point inside a column's
    // interior is the bug we're guarding against.
    let layer_w = dims.layer_width;
    let node_w = dims.node_w;
    for &(x, _y) in &long.points {
        let col = x / layer_w;
        let col_start = col * layer_w;
        let col_interior_end = col_start + node_w;
        // Allow points exactly at column anchors (col_start = left
        // edge, col_interior_end = right edge). Disallow strictly
        // interior points unless the point is on a dummy's row.
        // Dummies don't render, so an interior point at a dummy's
        // y is fine — but we use the simpler test that ALL points
        // must land at a column edge or in the gap.
        let in_gap = x >= col_interior_end || x == col_start;
        assert!(
            in_gap,
            "polyline point ({x}, _) sits inside layer-{col} column \
             ({col_start}..{col_interior_end}); bends must go in gaps. \
             Full route: {:?}",
            long.points,
        );
    }
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
