//! Index tree renderer. See design spec §Index.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::frame::{NodeStatus, ViewFrame};
use crate::state::{AppState, Selection};

pub fn render<F: ViewFrame>(area: Rect, buf: &mut Buffer, app: &AppState, frame: &F) {
    let mut row = area.y;
    for (wi, wave) in app.tree.waves.iter().enumerate() {
        if row >= area.y + area.height {
            return;
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
                return;
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
                    return;
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
}
