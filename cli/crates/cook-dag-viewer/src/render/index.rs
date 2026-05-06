//! Index tree renderer. See design spec §Index.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::frame::{NodeStatus, ViewFrame};
use crate::state::{AppState, Selection};

pub fn render<F: ViewFrame>(area: Rect, buf: &mut Buffer, app: &AppState, frame: &F) {
    let row_after_tree = render_tree(area, buf, app, frame);

    if app.density == crate::state::DensityMode::Flow && !app.pins.is_empty() {
        render_pinned_legend(area, buf, app, frame, row_after_tree);
    }
}

fn render_tree<F: ViewFrame>(
    area: Rect,
    buf: &mut Buffer,
    app: &AppState,
    frame: &F,
) -> u16 {
    let mut row = area.y;
    'outer: for (wi, wave) in app.tree.waves.iter().enumerate() {
        if row >= area.y + area.height {
            break;
        }
        let glyph = if wave.expanded { '▼' } else { '▶' };
        let line = format!("{} {}", glyph, wave.label);
        let style = sel_style(app.selection, Selection { wave: wi, recipe: None, unit: None });
        write_line(area, buf, row, 0, &line, style);
        row += 1;

        if !wave.expanded {
            continue;
        }
        for (ri, recipe) in wave.recipes.iter().enumerate() {
            if row >= area.y + area.height {
                break 'outer;
            }
            let glyph = if recipe.expanded { '▼' } else { '▶' };
            let line = format!("{} {}", glyph, recipe.name);
            let style =
                sel_style(app.selection, Selection { wave: wi, recipe: Some(ri), unit: None });
            write_line(area, buf, row, 2, &line, style);
            row += 1;

            if !recipe.expanded {
                continue;
            }
            for (ui, unit) in recipe.units.iter().enumerate() {
                if row >= area.y + area.height {
                    break 'outer;
                }
                let badge = badge(frame.status_of(&unit.node_id));
                let line = format!("● {}  {}", unit.label, badge);
                let style = sel_style(
                    app.selection,
                    Selection { wave: wi, recipe: Some(ri), unit: Some(ui) },
                );
                write_line(area, buf, row, 4, &line, style);
                row += 1;
            }
        }
    }
    row
}

fn render_pinned_legend<F: ViewFrame>(
    area: Rect,
    buf: &mut Buffer,
    app: &AppState,
    frame: &F,
    start_y: u16,
) {
    if start_y >= area.y + area.height {
        return;
    }

    let count = app.pins.iter().count();
    let header = format!(" pinned ({}) ", count);
    write_line(area, buf, start_y, 0, &header, Style::default());

    let mut y = start_y + 1;
    for (slot, node_id) in app.pins.iter() {
        if y + 1 >= area.y + area.height {
            break;
        }
        let Some(node) = find_legend_node(frame, node_id) else {
            continue;
        };
        let glyph = crate::state::pin_glyph(slot);
        let title = format!(" {} {}", glyph, truncate_label(&node.label, 22));
        write_line(
            area,
            buf,
            y,
            0,
            &title,
            Style::default().fg(app.theme.pin_slots[slot]),
        );
        if y + 1 < area.y + area.height {
            let context = format_legend_context(node);
            let context_line = format!("   {}", context);
            write_line(area, buf, y + 1, 0, &context_line, Style::default());
        }
        y += 2;
    }
}

fn find_legend_node<'a, F: ViewFrame>(
    frame: &'a F,
    id: &str,
) -> Option<&'a crate::dag_data::NodeData> {
    frame
        .graph()
        .waves
        .iter()
        .flat_map(|w| w.nodes.iter())
        .find(|n| n.id == id)
}

fn format_legend_context(node: &crate::dag_data::NodeData) -> String {
    match (node.kind.as_str(), node.recipe.as_deref()) {
        ("file", _) if node.discovered == Some(true) => "discovered".to_string(),
        ("file", _) => "declared".to_string(),
        ("unit", Some(r)) => r.to_string(),
        ("unit", None) => "unit".to_string(),
        _ => node.kind.clone(),
    }
}

fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max <= 1 {
        "…".to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

fn sel_style(current: Selection, this: Selection) -> Style {
    if current == this {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    }
}

fn badge(s: NodeStatus) -> char {
    match s {
        NodeStatus::Cached => '✓',
        NodeStatus::Stale => '✗',
        NodeStatus::Modified => '⚠',
        NodeStatus::Done => '·',
        NodeStatus::Pending | NodeStatus::Running | NodeStatus::Failed => ' ',
    }
}

fn write_line(area: Rect, buf: &mut Buffer, y: u16, indent: u16, text: &str, style: Style) {
    let x = area.x + indent;
    let max = area.x + area.width;
    let mut col = x;
    for ch in text.chars() {
        if col >= max {
            break;
        }
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(ch).set_style(style);
        }
        col += 1;
    }
    while col < max {
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(' ').set_style(style);
        }
        col += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{NodeData, WaveData, WaveDagData};
    use crate::frame::SnapshotFrame;
    use crate::state::AppState;

    fn unit(id: &str, recipe: &str, label: &str, cached: Option<bool>) -> NodeData {
        NodeData {
            id: id.into(),
            kind: "unit".into(),
            label: label.into(),
            recipe: Some(recipe.into()),
            command: Some("cmd".into()),
            output: None,
            cached,
            dep_kind: Some("sequential".into()),
            group_index: None,
            modified: None,
            discovered: None,
        }
    }

    fn graph() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![
                    unit("unit:a:0", "a", "a0", Some(true)),
                    unit("unit:a:1", "a", "a1", Some(false)),
                ],
                edges: vec![],
            }],
            inter_wave_edges: vec![],
        }
    }

    fn cell_at(buf: &Buffer, x: u16, y: u16) -> char {
        buf.cell((x, y)).unwrap().symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn renders_collapsed_wave_only() {
        let g = graph();
        let app = AppState::new(&g);
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 28, 5);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        // Wave 0 (1 recipes) is at row 0
        assert_eq!(cell_at(&buf, 0, 0), '▼');
    }

    #[test]
    fn renders_expanded_recipe_with_units_and_badges() {
        let g = graph();
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 28, 5);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        // Row 1 = recipe `a` at indent 2 (expanded).
        assert_eq!(cell_at(&buf, 2, 1), '▼');
        // Row 2 = unit a0 cached → ✓.
        assert_eq!(cell_at(&buf, 4, 2), '●');
    }

    #[test]
    fn pinned_legend_renders_in_flow_mode_with_pins() {
        let g = WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["compile".into()],
                nodes: vec![NodeData {
                    id: "unit:compile:0".into(),
                    kind: "unit".into(),
                    label: "bar.o".into(),
                    recipe: Some("compile".into()),
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
        };
        let mut app = AppState::new(&g);
        app.density = crate::state::DensityMode::Flow;
        app.pins.pin("unit:compile:0");
        let frame = SnapshotFrame::new(g.clone());
        let area = Rect::new(0, 0, 28, 24);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        let any_row_has_glyph = (0..area.height).any(|y| {
            (0..area.width).any(|x| {
                buf.cell((x, y)).unwrap().symbol().contains('❶')
            })
        });
        assert!(any_row_has_glyph, "legend should render ❶ for slot-0 pin");

        let any_row_has_label = (0..area.height).any(|y| {
            let line: String = (0..area.width)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect();
            line.contains("bar.o")
        });
        assert!(any_row_has_label, "legend should include the node label");
    }

    #[test]
    fn pinned_legend_hidden_when_density_not_flow() {
        let g = WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["compile".into()],
                nodes: vec![NodeData {
                    id: "unit:compile:0".into(),
                    kind: "unit".into(),
                    label: "bar.o".into(),
                    recipe: Some("compile".into()),
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
        };
        let mut app = AppState::new(&g);
        app.density = crate::state::DensityMode::Full;
        app.pins.pin("unit:compile:0");
        let frame = SnapshotFrame::new(g.clone());
        let area = Rect::new(0, 0, 28, 24);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        let any_row_has_glyph = (0..area.height).any(|y| {
            (0..area.width).any(|x| buf.cell((x, y)).unwrap().symbol().contains('❶'))
        });
        assert!(!any_row_has_glyph, "legend must not render outside Flow mode");
    }
}
