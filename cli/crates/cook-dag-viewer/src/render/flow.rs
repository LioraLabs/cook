//! Flow-mode renderer. See spec §4.4.
//!
//! Renders the layered DAG as Braille-canvas glyphs and straight
//! diagonal edges. Real edges (from `WaveDagData.edges`) are drawn as
//! single Lines between source-node and target-node centers; the
//! orthogonal polylines stored in `Layout.edges` are intentionally
//! ignored, so dummy bend points do not show up. The polylines are
//! preserved in `Layout.edges` so a future spline-edge upgrade can
//! consume them.

use std::collections::BTreeMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Line};
use ratatui::widgets::Widget;

use crate::dag_data::WaveDagData;
use crate::frame::ViewFrame;
use crate::render::layout::{Layout, PlacedNode};
use crate::state::AppState;

pub fn render<F: ViewFrame>(layout: &Layout, app: &AppState, frame: &F) -> Buffer {
    let area = Rect::new(0, 0, layout.canvas_w.max(1), layout.canvas_h.max(1));
    let mut buf = Buffer::empty(area);

    let nodes_by_id: BTreeMap<&str, &PlacedNode> =
        layout.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    draw_edges(&nodes_by_id, frame.graph(), area, app, &mut buf);
    buf
}

/// Build one Braille `Line` per real edge. Color is the theme's edge
/// color for now; Task 8 swaps this to the source node's cache-status
/// color.
fn draw_edges(
    nodes: &BTreeMap<&str, &PlacedNode>,
    graph: &WaveDagData,
    area: Rect,
    app: &AppState,
    buf: &mut Buffer,
) {
    let mut pairs: Vec<(&PlacedNode, &PlacedNode)> = Vec::new();
    for wave in &graph.waves {
        for e in &wave.edges {
            if let (Some(s), Some(t)) =
                (nodes.get(e.from.as_str()), nodes.get(e.to.as_str()))
            {
                pairs.push((*s, *t));
            }
        }
    }
    for e in &graph.inter_wave_edges {
        if let (Some(s), Some(t)) =
            (nodes.get(e.from.as_str()), nodes.get(e.to.as_str()))
        {
            pairs.push((*s, *t));
        }
    }

    let edge_color: Color = app.theme.edge;
    let lines: Vec<Line> = pairs
        .iter()
        .map(|(s, t)| Line {
            x1: s.x as f64 + s.w as f64 / 2.0,
            y1: flip_y(s.y, area.height) - s.h as f64 / 2.0,
            x2: t.x as f64 + t.w as f64 / 2.0,
            y2: flip_y(t.y, area.height) - t.h as f64 / 2.0,
            color: edge_color,
        })
        .collect();

    Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, area.width as f64])
        .y_bounds([0.0, area.height as f64])
        .paint(|ctx| {
            for line in &lines {
                ctx.draw(line);
            }
        })
        .render(area, buf);
}

/// Convert cell-y (top-down) to Canvas-y (bottom-up). Canvas's
/// y_bounds are mathematical (positive = up); cell coords run
/// top-down.
fn flip_y(y: u16, area_h: u16) -> f64 {
    (area_h as i32 - y as i32) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};
    use crate::frame::SnapshotFrame;
    use crate::render::layout;
    use crate::state::AppState;

    fn two_node_dag() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![
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
                    NodeData {
                        id: "unit:b:0".into(),
                        kind: "unit".into(),
                        label: "b0".into(),
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
                    from: "unit:a:0".into(),
                    to: "unit:b:0".into(),
                }],
            }],
            inter_wave_edges: vec![],
        }
    }

    #[test]
    fn flow_renders_braille_pixels_along_edge() {
        let g = two_node_dag();
        let app = AppState::new(&g);
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FLOW);
        let buf = render(&layout, &app, &frame);

        // At least one cell on the canvas contains a Unicode Braille
        // codepoint (U+2800..U+28FF).
        let mut any_braille = false;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    if cell.symbol().chars().any(|c: char| ('\u{2800}'..='\u{28FF}').contains(&c)) {
                        any_braille = true;
                    }
                }
            }
        }
        assert!(any_braille, "expected at least one Braille glyph on the Flow canvas");
    }
}
