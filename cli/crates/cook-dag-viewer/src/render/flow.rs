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
use ratatui::widgets::canvas::{Canvas, Circle, Line, Points};
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
    draw_nodes(layout, area, app, &mut buf);
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

fn draw_nodes(
    layout: &Layout,
    area: Rect,
    app: &AppState,
    buf: &mut Buffer,
) {
    let style = app.glyph;
    let color = app.theme.edge; // Task 8 swaps to per-node color.

    let w = area.width as f64;
    let h = area.height as f64;
    // Braille resolution: 2 dots wide × 4 dots tall per cell.
    let res_x = w * 2.0;
    let res_y = h * 4.0;

    Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, w])
        .y_bounds([0.0, h])
        .paint(|ctx| {
            for n in &layout.nodes {
                // Compute the target cell center (in cell coordinates).
                let cell_cx = n.x as f64 + n.w as f64 / 2.0;
                let cell_cy_top = (n.y + n.h / 2) as f64; // row of center cell
                // Invert ratatui's Braille mapping to find canvas coords that
                // land exactly in the center cell. ratatui uses:
                //   dot_x = round((cx - left) * (res_x - 1) / width)
                //   dot_y = round((top   - cy) * (res_y - 1) / height)
                // We target dot_x = cell_cx * 2 + 0 (left dot of center cell)
                // and dot_y = cell_cy_top * 4 + 1 (second Braille row).
                let target_dot_x = cell_cx * 2.0;
                let target_dot_y = cell_cy_top * 4.0 + 1.0;
                let cx = target_dot_x * w / (res_x - 1.0);
                let cy = h - target_dot_y * h / (res_y - 1.0);
                let radius = (n.w.min(n.h) as f64) / 2.0;
                draw_glyph(ctx, cx, cy, radius, style, color);
            }
        })
        .render(area, buf);
}

fn draw_glyph(
    ctx: &mut ratatui::widgets::canvas::Context<'_>,
    cx: f64,
    cy: f64,
    radius: f64,
    style: crate::state::GlyphStyle,
    color: Color,
) {
    use crate::state::GlyphStyle;
    match style {
        GlyphStyle::Dot => {
            let pts = Points { coords: &[(cx, cy)], color };
            ctx.draw(&pts);
        }
        GlyphStyle::Circle => {
            let c = Circle { x: cx, y: cy, radius, color };
            ctx.draw(&c);
        }
        GlyphStyle::Diamond => {
            let p_top = (cx, cy + radius);
            let p_right = (cx + radius, cy);
            let p_bot = (cx, cy - radius);
            let p_left = (cx - radius, cy);
            for (a, b) in [
                (p_top, p_right),
                (p_right, p_bot),
                (p_bot, p_left),
                (p_left, p_top),
            ] {
                ctx.draw(&Line { x1: a.0, y1: a.1, x2: b.0, y2: b.1, color });
            }
        }
        GlyphStyle::Square => {
            let s = radius;
            let tl = (cx - s, cy + s);
            let tr = (cx + s, cy + s);
            let br = (cx + s, cy - s);
            let bl = (cx - s, cy - s);
            for (a, b) in [(tl, tr), (tr, br), (br, bl), (bl, tl)] {
                ctx.draw(&Line { x1: a.0, y1: a.1, x2: b.0, y2: b.1, color });
            }
        }
    }
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
    fn flow_renders_node_glyphs_in_dot_style() {
        let g = two_node_dag();
        let mut app = AppState::new(&g);
        app.glyph = crate::state::GlyphStyle::Dot;
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FLOW);
        let buf = render(&layout, &app, &frame);

        // Each placed node center should have a Braille pixel.
        for n in &layout.nodes {
            let cx = n.x + n.w / 2;
            let cy = n.y + n.h / 2;
            let cell = buf.cell((cx, cy)).expect("cell in canvas");
            assert!(
                cell.symbol().chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
                "node {} should have a Braille pixel at ({}, {})", n.id, cx, cy,
            );
        }
    }

    #[test]
    fn flow_renders_circle_outline_for_circle_glyph() {
        let g = two_node_dag();
        let mut app = AppState::new(&g);
        app.glyph = crate::state::GlyphStyle::Circle;
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FLOW);
        let buf = render(&layout, &app, &frame);

        // A circle outline lights up Braille pixels on cells adjacent to
        // each node center (top / bottom / left / right of the center cell).
        for n in &layout.nodes {
            let cx = n.x + n.w / 2;
            let cy = n.y + n.h / 2;
            let neighbours = [
                (cx.saturating_sub(1), cy),
                (cx + 1, cy),
                (cx, cy.saturating_sub(1)),
                (cx, cy + 1),
            ];
            let any_outline = neighbours.iter().any(|&(x, y)| {
                buf.cell((x, y))
                    .map(|c| c.symbol().chars().any(|ch| ('\u{2800}'..='\u{28FF}').contains(&ch)))
                    .unwrap_or(false)
            });
            assert!(any_outline, "circle outline missing around node {}", n.id);
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
