//! Render `Layout` into an oversized `Buffer`. See design spec §Rendering.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Line};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget};

use crate::frame::{NodeStatus, ViewFrame};
use crate::render::layout::Layout;
use crate::state::AppState;

pub fn render<F: ViewFrame>(layout: &Layout, app: &AppState, frame: &F) -> Buffer {
    let area = Rect::new(0, 0, layout.canvas_w.max(1), layout.canvas_h.max(1));
    let mut buf = Buffer::empty(area);

    draw_edges(layout, area, &mut buf, &app.theme);
    draw_nodes(layout, area, &mut buf);
    overlay_badges(layout, frame, &mut buf, &app.theme);
    overlay_selection(layout, app, &mut buf);
    buf
}

fn draw_edges(layout: &Layout, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
    let lines: Vec<Line> = layout
        .edges
        .iter()
        .flat_map(|e| {
            e.points.windows(2).map(|w| Line {
                x1: w[0].0 as f64,
                y1: (area.height as i32 - w[0].1 as i32) as f64,
                x2: w[1].0 as f64,
                y2: (area.height as i32 - w[1].1 as i32) as f64,
                color: theme.edge,
            })
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

fn draw_nodes(layout: &Layout, _area: Rect, buf: &mut Buffer) {
    for node in &layout.nodes {
        let rect = Rect::new(node.x, node.y, node.w, node.h);
        let border_type = if node.kind == "file" && node.discovered == Some(true) {
            BorderType::Rounded
        } else {
            BorderType::Plain
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type);
        block.clone().render(rect, buf);
        let inner = block.inner(rect);
        Paragraph::new(node.label.clone()).render(inner, buf);
    }
}

fn overlay_badges<F: ViewFrame>(
    layout: &Layout,
    frame: &F,
    buf: &mut Buffer,
    theme: &crate::theme::Theme,
) {
    for node in &layout.nodes {
        let badge = if node.kind == "file" && node.discovered == Some(true) {
            Some(('~', theme.badge_discovered))
        } else {
            match frame.status_of(&node.id) {
                NodeStatus::Cached => Some(('✓', theme.badge_cached)),
                NodeStatus::Stale => Some(('✗', theme.badge_stale)),
                NodeStatus::Modified => Some(('⚠', theme.badge_modified)),
                _ => None,
            }
        };
        if let Some((ch, color)) = badge {
            let bx = node.x + node.w.saturating_sub(2);
            let by = node.y;
            if let Some(cell) = buf.cell_mut((bx, by)) {
                cell.set_char(ch).set_style(Style::default().fg(color));
            }
        }
    }
}

fn overlay_selection(layout: &Layout, app: &AppState, buf: &mut Buffer) {
    let Some(sel_id) = app.selection.node_id(&app.tree) else { return };
    let Some(node) = layout.nodes.iter().find(|n| n.id == sel_id) else { return };
    for dy in 0..node.h {
        for dx in 0..node.w {
            if let Some(cell) = buf.cell_mut((node.x + dx, node.y + dy)) {
                let sty = cell.style().add_modifier(Modifier::REVERSED);
                cell.set_style(sty);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{NodeData, WaveData, WaveDagData};
    use crate::frame::SnapshotFrame;
    use crate::render::layout;
    use crate::state::{AppState, Selection};

    fn dag() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![NodeData {
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
                }],
                edges: vec![],
            }],
            inter_wave_edges: vec![],
        }
    }

    #[test]
    fn renders_node_box_with_label_and_badge() {
        let g = dag();
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        app.selection = Selection { wave: 0, recipe: Some(0), unit: Some(0) };
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FULL);
        let buf = render(&layout, &app, &frame);

        let placed = layout.nodes.iter().find(|n| n.id == "unit:a:0").unwrap();
        // Top-left corner of the node box.
        let tl = buf.cell((placed.x, placed.y)).unwrap();
        assert_eq!(tl.symbol(), "┌");
        // Badge in the top-right area.
        let badge_x = placed.x + placed.w.saturating_sub(2);
        let badge_cell = buf.cell((badge_x, placed.y)).unwrap();
        assert_eq!(badge_cell.symbol().chars().next(), Some('✓'));
    }

    #[test]
    fn selection_overlay_applies_reverse_video() {
        let g = dag();
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        app.selection = Selection { wave: 0, recipe: Some(0), unit: Some(0) };
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FULL);
        let buf = render(&layout, &app, &frame);
        let placed = layout.nodes.iter().find(|n| n.id == "unit:a:0").unwrap();
        let cell = buf.cell((placed.x + 1, placed.y + 1)).unwrap();
        assert!(cell.style().add_modifier.contains(Modifier::REVERSED));
    }

    use crate::dag_data::EdgeData;

    fn dag_with_discovered_file() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![
                    NodeData {
                        id: "file:helpers.h".into(),
                        kind: "file".into(),
                        label: "helpers.h".into(),
                        recipe: None,
                        command: None,
                        output: None,
                        cached: None,
                        dep_kind: None,
                        group_index: None,
                        modified: None,
                        discovered: Some(true),
                    },
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
                ],
                edges: vec![EdgeData {
                    from: "file:helpers.h".into(),
                    to: "unit:a:0".into(),
                }],
            }],
            inter_wave_edges: vec![],
        }
    }

    #[test]
    fn discovered_file_node_uses_rounded_border() {
        let g = dag_with_discovered_file();
        let app = AppState::new(&g);
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FULL);
        let buf = render(&layout, &app, &frame);

        let helpers = layout.nodes.iter().find(|n| n.id == "file:helpers.h").unwrap();
        let tl = buf.cell((helpers.x, helpers.y)).unwrap();
        assert_eq!(tl.symbol(), "╭", "discovered file should use rounded top-left corner");

        let unit = layout.nodes.iter().find(|n| n.id == "unit:a:0").unwrap();
        let unit_tl = buf.cell((unit.x, unit.y)).unwrap();
        assert_eq!(unit_tl.symbol(), "┌", "unit should keep plain top-left corner");
    }

    #[test]
    fn discovered_file_node_renders_tilde_badge() {
        let g = dag_with_discovered_file();
        let app = AppState::new(&g);
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::FULL);
        let buf = render(&layout, &app, &frame);

        let helpers = layout.nodes.iter().find(|n| n.id == "file:helpers.h").unwrap();
        let badge_x = helpers.x + helpers.w.saturating_sub(2);
        let badge_cell = buf.cell((badge_x, helpers.y)).unwrap();
        assert_eq!(badge_cell.symbol(), "~");
    }
}
