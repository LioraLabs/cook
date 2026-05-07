//! Render orchestration. See design spec §Layout.

pub mod camera;
pub mod canvas;
pub mod detail;
pub mod focus;
pub mod index;
pub mod layout;
pub mod overlay;
pub mod search;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout as TuiLayout, Rect};
use ratatui::style::Style;

use crate::dag_data::WaveDagData;
use crate::frame::ViewFrame;
use crate::state::{AppState, Mode};

/// Build the focused subgraph for the current selection and lay it out
/// with the boxed-node geometry. There is only one render path —
/// every render filters to the selection's neighborhood.
pub fn pick_layout(app: &AppState, graph: &WaveDagData) -> layout::Layout {
    let focused = focus::focus_subgraph(graph, app);
    layout::compute(&focused, layout::LayoutDims::FULL)
}

pub struct RenderInputs<'a> {
    pub canvas: &'a Buffer,
    pub layout: &'a layout::Layout,
}

pub fn draw<F: ViewFrame>(
    area: Rect,
    buf: &mut Buffer,
    app: &mut AppState,
    frame: &F,
    r: RenderInputs<'_>,
) {
    fill(area, buf, ' ');

    let chunks = TuiLayout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    draw_top_bar(chunks[0], buf, app, frame);
    draw_body(chunks[1], buf, app, frame, r);

    // Overlays are rendered before draw_bottom_bar so that draw_bottom_bar
    // (which takes &mut app to consume last_pin_message) runs last, after
    // all immutable borrows of app fields are complete.
    match app.mode {
        Mode::EdgePicker => overlay::render_edge_picker(area, buf, &app.edge_picker),
        Mode::Help => overlay::render_help(area, buf),
        Mode::DetailOverlay => overlay::render_detail_overlay(area, buf, app, frame),
        Mode::Search => search::render(area, buf, &app.search),
        Mode::Normal => {}
    }

    draw_bottom_bar(chunks[2], buf, app);
}

fn draw_top_bar<F: ViewFrame>(area: Rect, buf: &mut Buffer, _app: &AppState, frame: &F) {
    let g = frame.graph();
    let recipe_count: usize = g.waves.iter().map(|w| w.recipes.len()).sum();
    let line = format!(
        " cook · {} · {} waves · {} recipes",
        g.target,
        g.waves.len(),
        recipe_count
    );
    write_line(area, buf, area.y, &line);
}

fn draw_body<F: ViewFrame>(
    area: Rect,
    buf: &mut Buffer,
    app: &mut AppState,
    frame: &F,
    r: RenderInputs<'_>,
) {
    let cols = TuiLayout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(1)])
        .split(area);
    app.ensure_index_visible(cols[0].height as usize);
    index::render(cols[0], buf, app, frame);
    let right = TuiLayout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(6)])
        .split(cols[1]);
    let cam = camera::Camera { x: app.camera_x, y: app.camera_y };
    camera::blit(r.canvas, cam, right[0], buf);
    detail::render(right[1], buf, app, frame);
}

fn draw_bottom_bar(area: Rect, buf: &mut Buffer, app: &mut AppState) {
    let mode = if app.follow { "follow" } else { "free" };
    let pin_message = app.last_pin_message.take();
    let hint = if let Some(msg) = pin_message {
        msg.render()
    } else {
        match app.mode {
            Mode::Search => " /search · esc cancel · enter jump".to_string(),
            Mode::EdgePicker => " 1-9 jump · esc cancel".to_string(),
            Mode::Help => " help · q close".to_string(),
            Mode::DetailOverlay => " esc close".to_string(),
            Mode::Normal => {
                " ? help · / · q · [/] · HJKL · p pin · 1-9 jump · X clear".to_string()
            }
        }
    };
    let line = format!("{} [{}]", hint, mode);
    write_line(area, buf, area.y, &line);
}

fn write_line(area: Rect, buf: &mut Buffer, y: u16, text: &str) {
    let max = area.x + area.width;
    let mut col = area.x;
    for ch in text.chars() {
        if col >= max {
            break;
        }
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(ch).set_style(Style::default());
        }
        col += 1;
    }
    while col < max {
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(' ').set_style(Style::default());
        }
        col += 1;
    }
}

fn fill(area: Rect, buf: &mut Buffer, ch: char) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(ch).set_style(Style::default());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};
    use crate::state::{AppState, Selection};

    fn unit_node(id: &str, recipe: &str, label: &str) -> NodeData {
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

    fn three_unit_chain() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![
                    unit_node("unit:a:0", "a", "a0"),
                    unit_node("unit:a:1", "a", "a1"),
                    unit_node("unit:a:2", "a", "a2"),
                ],
                edges: vec![
                    EdgeData { from: "unit:a:0".into(), to: "unit:a:1".into() },
                    EdgeData { from: "unit:a:1".into(), to: "unit:a:2".into() },
                ],
            }],
            inter_wave_edges: vec![],
        }
    }

    #[test]
    fn pick_layout_always_runs_focus_filter() {
        let g = three_unit_chain();
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        // Select the middle unit. Focus = a0, a1, a2 (a1 + 1-hop).
        app.selection = Selection::unit(0, 0, 1);
        let layout = pick_layout(&app, &g);
        let ids: std::collections::BTreeSet<&str> =
            layout.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            std::collections::BTreeSet::from(["unit:a:0", "unit:a:1", "unit:a:2"])
        );

        // Now select an end unit — focus drops the far one.
        app.selection = Selection::unit(0, 0, 0);
        let layout = pick_layout(&app, &g);
        let ids: std::collections::BTreeSet<&str> =
            layout.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            std::collections::BTreeSet::from(["unit:a:0", "unit:a:1"])
        );
    }
}
