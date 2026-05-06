//! Snapshot tests for flow-mode rendering.

use cook_dag_viewer::frame::SnapshotFrame;
use cook_dag_viewer::render::{flow, layout};
use cook_dag_viewer::state::{AppState, DensityMode, GlyphStyle};

mod fixtures;

fn buf_to_string(buf: &ratatui::buffer::Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = buf.cell((x, y)).unwrap();
            out.push_str(cell.symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn flow_small_dag_circle() {
    let g = fixtures::small_dag();
    let mut app = AppState::new(&g);
    app.density = DensityMode::Flow;
    app.glyph = GlyphStyle::Circle;
    let frame = SnapshotFrame::new(g.clone());
    let layout = layout::compute(&g, layout::LayoutDims::FLOW);
    let buf = flow::render(&layout, &app, &frame);
    insta::assert_snapshot!(buf_to_string(&buf));
}

#[test]
fn flow_medium_dag_circle() {
    let g = fixtures::medium_dag();
    let mut app = AppState::new(&g);
    app.density = DensityMode::Flow;
    app.glyph = GlyphStyle::Circle;
    let frame = SnapshotFrame::new(g.clone());
    let layout = layout::compute(&g, layout::LayoutDims::FLOW);
    let buf = flow::render(&layout, &app, &frame);
    insta::assert_snapshot!(buf_to_string(&buf));
}

#[test]
fn flow_wide_dag_circle() {
    let g = fixtures::wide_dag();
    let mut app = AppState::new(&g);
    app.density = DensityMode::Flow;
    app.glyph = GlyphStyle::Circle;
    let frame = SnapshotFrame::new(g.clone());
    let layout = layout::compute(&g, layout::LayoutDims::FLOW);
    let buf = flow::render(&layout, &app, &frame);
    insta::assert_snapshot!(buf_to_string(&buf));
}

#[test]
fn flow_medium_dag_diamond() {
    let g = fixtures::medium_dag();
    let mut app = AppState::new(&g);
    app.density = DensityMode::Flow;
    app.glyph = GlyphStyle::Diamond;
    let frame = SnapshotFrame::new(g.clone());
    let layout = layout::compute(&g, layout::LayoutDims::FLOW);
    let buf = flow::render(&layout, &app, &frame);
    insta::assert_snapshot!(buf_to_string(&buf));
}
