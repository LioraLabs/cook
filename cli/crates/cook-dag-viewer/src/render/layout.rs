//! Pure layout pass. See design spec §Graph Layout.

use std::collections::BTreeMap;

use crate::dag_data::WaveDagData;

pub const WAVE_WIDTH: u16 = 32;
pub const NODE_W: u16 = 22;
pub const NODE_H: u16 = 3;
pub const RECIPE_PAD: u16 = 1;
pub const UNIT_PAD: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedNode {
    pub id: String,
    pub kind: String, // "file" or "unit"
    pub label: String,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
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

pub fn compute(g: &WaveDagData) -> Layout {
    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut node_pos: BTreeMap<String, (u16, u16)> = BTreeMap::new();
    let mut canvas_h: u16 = 0;

    for (wi, wave) in g.waves.iter().enumerate() {
        let band_x = wi as u16 * WAVE_WIDTH;
        let mut y_cursor: u16 = 0;
        for recipe in &wave.recipes {
            let recipe_unit_ids: Vec<&str> = wave
                .nodes
                .iter()
                .filter(|n| n.kind == "unit" && n.recipe.as_deref() == Some(recipe))
                .map(|n| n.id.as_str())
                .collect();

            // File nodes consumed first by units in this recipe (dedup).
            let mut file_ids: Vec<&str> = Vec::new();
            for u in &recipe_unit_ids {
                for e in &wave.edges {
                    if e.to == *u {
                        let n = wave.nodes.iter().find(|n| n.id == e.from);
                        if let Some(n) = n {
                            if n.kind == "file" && !file_ids.contains(&e.from.as_str()) {
                                file_ids.push(e.from.as_str());
                            }
                        }
                    }
                }
            }

            for fid in &file_ids {
                let Some(node) = wave.nodes.iter().find(|n| n.id == *fid) else { continue };
                let placed = PlacedNode {
                    id: node.id.clone(),
                    kind: node.kind.clone(),
                    label: node.label.clone(),
                    x: band_x,
                    y: y_cursor,
                    w: NODE_W,
                    h: NODE_H,
                };
                node_pos.insert(placed.id.clone(), (placed.x, placed.y));
                nodes.push(placed);
                y_cursor += NODE_H + UNIT_PAD;
            }

            for uid in &recipe_unit_ids {
                let Some(node) = wave.nodes.iter().find(|n| n.id == *uid) else { continue };
                let placed = PlacedNode {
                    id: node.id.clone(),
                    kind: node.kind.clone(),
                    label: node.label.clone(),
                    x: band_x,
                    y: y_cursor,
                    w: NODE_W,
                    h: NODE_H,
                };
                node_pos.insert(placed.id.clone(), (placed.x, placed.y));
                nodes.push(placed);
                y_cursor += NODE_H + UNIT_PAD;
            }
            y_cursor += RECIPE_PAD;
        }
        canvas_h = canvas_h.max(y_cursor);
    }

    let canvas_w = (g.waves.len() as u16).saturating_mul(WAVE_WIDTH).max(WAVE_WIDTH);

    let mut edges: Vec<EdgeRoute> = Vec::new();
    for wave in &g.waves {
        for e in &wave.edges {
            if let Some(route) = route_edge(&node_pos, &e.from, &e.to) {
                edges.push(EdgeRoute { from: e.from.clone(), to: e.to.clone(), points: route });
            }
        }
    }
    for e in &g.inter_wave_edges {
        if let Some(route) = route_edge(&node_pos, &e.from, &e.to) {
            edges.push(EdgeRoute { from: e.from.clone(), to: e.to.clone(), points: route });
        }
    }

    Layout { nodes, edges, canvas_w, canvas_h }
}

fn route_edge(
    pos: &BTreeMap<String, (u16, u16)>,
    from: &str,
    to: &str,
) -> Option<Vec<(u16, u16)>> {
    let (fx, fy) = pos.get(from).copied()?;
    let (tx, ty) = pos.get(to).copied()?;
    let from_anchor = (fx + NODE_W / 2, fy + NODE_H);
    let to_anchor = (tx + NODE_W / 2, ty);
    if from_anchor.0 == to_anchor.0 {
        Some(vec![from_anchor, to_anchor])
    } else {
        let mid_y = (from_anchor.1 + to_anchor.1) / 2;
        Some(vec![
            from_anchor,
            (from_anchor.0, mid_y),
            (to_anchor.0, mid_y),
            to_anchor,
        ])
    }
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
        }
    }

    #[test]
    fn places_two_waves_with_file_above_unit() {
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
        let l = compute(&g);

        let foo = l.nodes.iter().find(|n| n.id == "file:foo").unwrap();
        let a0 = l.nodes.iter().find(|n| n.id == "unit:a:0").unwrap();
        let b0 = l.nodes.iter().find(|n| n.id == "unit:b:0").unwrap();

        assert_eq!(foo.x, 0);
        assert!(foo.y < a0.y, "file should be above unit");
        assert_eq!(a0.x, 0);
        assert_eq!(b0.x, WAVE_WIDTH);
        assert!(l.canvas_w >= 2 * WAVE_WIDTH);
    }

    #[test]
    fn routes_edges_with_orthogonal_polyline() {
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
        let l = compute(&g);
        assert_eq!(l.edges.len(), 1);
        let route = &l.edges[0].points;
        assert!(route.len() == 2 || route.len() == 4);
    }
}
