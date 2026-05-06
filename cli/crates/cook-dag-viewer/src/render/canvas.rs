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
    if matches!(app.density, crate::state::DensityMode::Flow) {
        return crate::render::flow::render(layout, app, frame);
    }
    let area = Rect::new(0, 0, layout.canvas_w.max(1), layout.canvas_h.max(1));
    let mut buf = Buffer::empty(area);

    draw_edges(layout, area, &mut buf, &app.theme);
    match app.density {
        crate::state::DensityMode::Compact => draw_compact(layout, app, frame, &mut buf),
        crate::state::DensityMode::Full => draw_nodes(layout, area, &mut buf),
        crate::state::DensityMode::Flow => unreachable!(),
    }
    // Badge overlay (✓ ✗ ⚠) is a Full-mode-only affordance: in Flow the
    // glyph itself carries cache colour; in Compact the bracketed label
    // is coloured per status, so a separate badge would clobber the last
    // label cell.
    if matches!(app.density, crate::state::DensityMode::Full) {
        overlay_badges(layout, frame, &mut buf, &app.theme);
    }
    overlay_selection(layout, app, &mut buf);
    buf
}

/// Map a node's `NodeStatus` to its theme colour. Returns `Color::Reset`
/// for non-cache states (Done / Pending / Running / Failed) so the dot or
/// label renders in the terminal default.
fn status_color<F: ViewFrame>(node_id: &str, frame: &F, theme: &crate::theme::Theme) -> ratatui::style::Color {
    match frame.status_of(node_id) {
        NodeStatus::Cached => theme.badge_cached,
        NodeStatus::Stale => theme.badge_stale,
        NodeStatus::Modified => theme.badge_modified,
        _ => ratatui::style::Color::Reset,
    }
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


/// Render each node as a single-row bracketed label: `[label]`. The label
/// is left-padded into the interior width (node_w - 2 cells); too-long
/// labels truncate with an ellipsis. Brackets and label inherit the
/// node's cache-status colour (Green / Red / Yellow), so Compact carries
/// the same visual signal Full mode gets via the badge overlay.
fn draw_compact<F: ViewFrame>(layout: &Layout, app: &AppState, frame: &F, buf: &mut Buffer) {
    for node in &layout.nodes {
        let interior_w = node.w.saturating_sub(2) as usize;
        let label = truncate_to(&node.label, interior_w);
        let row_y = node.y;
        let style = Style::default().fg(status_color(&node.id, frame, &app.theme));

        // Left bracket
        if let Some(cell) = buf.cell_mut((node.x, row_y)) {
            cell.set_char('[').set_style(style);
        }
        // Label
        for (i, ch) in label.chars().enumerate() {
            let x = node.x + 1 + i as u16;
            if let Some(cell) = buf.cell_mut((x, row_y)) {
                cell.set_char(ch).set_style(style);
            }
        }
        // Pad
        for x in node.x + 1 + label.chars().count() as u16
            ..node.x + node.w.saturating_sub(1)
        {
            if let Some(cell) = buf.cell_mut((x, row_y)) {
                cell.set_char(' ').set_style(style);
            }
        }
        // Right bracket
        if let Some(cell) = buf.cell_mut((node.x + node.w.saturating_sub(1), row_y)) {
            cell.set_char(']').set_style(style);
        }
    }
}

fn truncate_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else if max == 1 {
        "\u{2026}".to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('\u{2026}');
        out
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

    #[test]
    fn compact_mode_renders_bracketed_label_in_one_row() {
        let g = dag();
        let mut app = AppState::new(&g);
        app.density = crate::state::DensityMode::Compact;
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::COMPACT);
        let buf = render(&layout, &app, &frame);

        let placed = layout.nodes.iter().find(|n| n.id == "unit:a:0").unwrap();
        // First cell of the row is `[`
        let first = buf.cell((placed.x, placed.y)).unwrap();
        assert_eq!(first.symbol(), "[", "compact mode opens with `[`");
        // Last cell of the row is `]`
        let last = buf
            .cell((placed.x + placed.w.saturating_sub(1), placed.y))
            .unwrap();
        assert_eq!(last.symbol(), "]", "compact mode closes with `]`");
    }

    #[test]
    fn compact_mode_label_inherits_cache_status_color() {
        let g = dag();
        let mut app = AppState::new(&g);
        app.density = crate::state::DensityMode::Compact;
        let frame = SnapshotFrame::new(g.clone());
        let layout = layout::compute(&g, layout::LayoutDims::COMPACT);
        let buf = render(&layout, &app, &frame);

        let placed = layout.nodes.iter().find(|n| n.id == "unit:a:0").unwrap();
        // Bracket cell carries the status colour.
        let bracket = buf.cell((placed.x, placed.y)).unwrap();
        assert_eq!(
            bracket.style().fg,
            Some(app.theme.badge_cached),
            "cached unit's [ bracket should pick up theme.badge_cached"
        );
    }
}
