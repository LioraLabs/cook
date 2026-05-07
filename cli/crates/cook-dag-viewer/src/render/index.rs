//! Index tree renderer. See design spec §Index.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::frame::{NodeStatus, ViewFrame};
use crate::state::{AppState, Selection};

pub fn render<F: ViewFrame>(area: Rect, buf: &mut Buffer, app: &AppState, frame: &F) {
    render_tree(area, buf, app, frame);
}

fn render_tree<F: ViewFrame>(
    area: Rect,
    buf: &mut Buffer,
    app: &AppState,
    frame: &F,
) {
    let scroll = app.index_scroll;
    let visible_end = scroll + area.height as usize;
    let mut logical: usize = 0;

    // Translate a logical row index to a physical y coordinate, or
    // None if the row is scrolled out (above the viewport) or below
    // the viewport.
    let phys_y = |logical: usize| -> Option<u16> {
        if logical < scroll || logical >= visible_end {
            return None;
        }
        Some(area.y + (logical - scroll) as u16)
    };

    'outer: for (wi, wave) in app.tree.waves.iter().enumerate() {
        if logical >= visible_end {
            break;
        }
        if let Some(y) = phys_y(logical) {
            let glyph = if wave.expanded { '▼' } else { '▶' };
            let line = format!("{} {}", glyph, wave.label);
            let style = sel_style(app.selection, Selection::wave_only(wi));
            write_line(area, buf, y, 0, &line, style);
        }
        logical += 1;

        if !wave.expanded {
            continue;
        }

        // Files folder (rendered only when the wave has any files).
        if !wave.files.is_empty() {
            if logical >= visible_end {
                break 'outer;
            }
            if let Some(y) = phys_y(logical) {
                let glyph = if wave.files_expanded { '▼' } else { '▶' };
                let line = format!("{} Files ({})", glyph, wave.files.len());
                let style = sel_style(app.selection, Selection::files_folder(wi));
                write_line(area, buf, y, 2, &line, style);
            }
            logical += 1;

            if wave.files_expanded {
                for (fi, file) in wave.files.iter().enumerate() {
                    if logical >= visible_end {
                        break 'outer;
                    }
                    if let Some(y) = phys_y(logical) {
                        let (kind_glyph, kind_color) = if file.discovered {
                            ('◇', Some(app.theme.badge_discovered))
                        } else {
                            ('▢', None)
                        };
                        let status = frame.status_of(&file.node_id);
                        let badge = file_badge(status);
                        let badge_color = match status {
                            NodeStatus::Modified => Some(app.theme.badge_modified),
                            _ => None,
                        };
                        let style = sel_style(app.selection, Selection::file(wi, fi));
                        // Render glyph (kind_color) + space + label, then badge at right.
                        // Reserve the rightmost 2 columns for the badge: 2 cells from the right edge
                        // is where the badge writes. Subtract indent (4) + glyph (1) + space (1) from
                        // that reserved span to bound the label.
                        let max_label = area
                            .width
                            .saturating_sub(4 + 1 + 1 + 2) as usize;
                        let label_line =
                            format!("{} {}", kind_glyph, truncate_label(&file.label, max_label));
                        let glyph_style = match kind_color {
                            Some(c) => style.fg(c),
                            None => style,
                        };
                        write_line(area, buf, y, 4, &label_line, glyph_style);
                        // Badge at the right edge — overwrite the last 1 cell.
                        let badge_x = area.x + area.width.saturating_sub(2);
                        if let Some(cell) = buf.cell_mut((badge_x, y)) {
                            let s = match badge_color {
                                Some(c) => style.fg(c),
                                None => style,
                            };
                            cell.set_char(badge).set_style(s);
                        }
                    }
                    logical += 1;
                }
            }
        }

        for (ri, recipe) in wave.recipes.iter().enumerate() {
            if logical >= visible_end {
                break 'outer;
            }
            if let Some(y) = phys_y(logical) {
                let glyph = if recipe.expanded { '▼' } else { '▶' };
                let line = format!("{} {}", glyph, recipe.name);
                let style = sel_style(app.selection, Selection::recipe(wi, ri));
                write_line(area, buf, y, 2, &line, style);
            }
            logical += 1;

            if !recipe.expanded {
                continue;
            }
            for (ui, unit) in recipe.units.iter().enumerate() {
                if logical >= visible_end {
                    break 'outer;
                }
                if let Some(y) = phys_y(logical) {
                    let badge = badge(frame.status_of(&unit.node_id));
                    let line = format!("● {}  {}", unit.label, badge);
                    let style = sel_style(app.selection, Selection::unit(wi, ri, ui));
                    write_line(area, buf, y, 4, &line, style);
                }
                logical += 1;
            }
        }
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

fn file_badge(s: NodeStatus) -> char {
    match s {
        NodeStatus::Modified => '⚠',
        NodeStatus::Done => '·',
        _ => ' ',
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

    fn graph_with_files() -> WaveDagData {
        use crate::dag_data::EdgeData;
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![
                    NodeData {
                        id: "file:bar.cpp".into(),
                        kind: "file".into(),
                        label: "bar.cpp".into(),
                        recipe: None,
                        command: None,
                        output: None,
                        cached: None,
                        dep_kind: None,
                        group_index: None,
                        modified: Some(true),
                        discovered: None,
                    },
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
                        modified: Some(false),
                        discovered: Some(true),
                    },
                    unit("unit:a:0", "a", "a0", Some(true)),
                ],
                edges: vec![
                    EdgeData { from: "file:bar.cpp".into(), to: "unit:a:0".into() },
                    EdgeData { from: "file:helpers.h".into(), to: "unit:a:0".into() },
                ],
            }],
            inter_wave_edges: vec![],
        }
    }

    fn row_text(buf: &Buffer, area: Rect, y: u16) -> String {
        (area.x..area.x + area.width)
            .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn files_folder_header_renders_with_count() {
        let g = graph_with_files();
        let app = AppState::new(&g); // wave 0 expanded by default; files folder collapsed
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 28, 6);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        // Row 0: wave header. Row 1: Files folder header (collapsed).
        let line = row_text(&buf, area, 1);
        assert!(line.contains("Files (2)"), "expected `Files (2)` header, got: `{line}`");
        assert!(line.contains('▶'), "collapsed folder uses the right-pointing triangle");
    }

    #[test]
    fn files_folder_expanded_lists_files_alphabetical() {
        let g = graph_with_files();
        let mut app = AppState::new(&g);
        app.tree.waves[0].files_expanded = true;
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 28, 8);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        // Row 1 = Files folder header (expanded with ▼).
        assert!(row_text(&buf, area, 1).contains('▼'));
        // Row 2 = first file alphabetically: bar.cpp (declared, modified).
        let bar_row = row_text(&buf, area, 2);
        assert!(bar_row.contains("bar.cpp"), "row 2 should contain bar.cpp, got: `{bar_row}`");
        assert!(bar_row.contains('▢'), "declared file uses ▢ glyph");
        assert!(bar_row.contains('⚠'), "modified file uses ⚠ badge");

        // Row 3 = helpers.h (discovered, clean).
        let helpers_row = row_text(&buf, area, 3);
        assert!(helpers_row.contains("helpers.h"));
        assert!(helpers_row.contains('◇'), "discovered file uses ◇ glyph");
        assert!(helpers_row.contains('·'), "clean file uses · badge");
    }

    #[test]
    fn files_folder_hidden_when_wave_has_no_files() {
        let g = graph(); // existing fixture in this mod with one unit, no files
        let app = AppState::new(&g);
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 28, 4);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        // Row 0 = wave header. Row 1 should be the recipe row (no Files header).
        let line = row_text(&buf, area, 1);
        assert!(!line.contains("Files"), "no Files header for empty-files wave");
    }

    #[test]
    fn files_folder_header_is_reversed_when_selected() {
        let g = graph_with_files();
        let mut app = AppState::new(&g);
        app.selection = Selection::files_folder(0);
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 28, 6);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        // Row 1 = Files folder header. The first non-blank cell (the ▶/▼ glyph
        // at indent 2) must carry REVERSED.
        let cell = buf.cell((2, 1)).unwrap();
        assert!(
            cell.style().add_modifier.contains(Modifier::REVERSED),
            "expected folder-header glyph to be REVERSED when selected"
        );
    }

    #[test]
    fn scrolled_view_skips_rows_above_the_offset() {
        // Wave 0 expanded with one recipe expanded → many unit rows.
        let mut nodes: Vec<NodeData> = Vec::new();
        for i in 0..10 {
            nodes.push(unit(&format!("unit:a:{i}"), "a", &format!("u{i}"), Some(true)));
        }
        let g = WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes,
                edges: vec![],
            }],
            inter_wave_edges: vec![],
        };
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        // Visible rows: wave_only=0, recipe(0)=1, unit(0..10)=2..11. Total 12.
        // Scroll past the wave + recipe rows → unit u0 should be the first row.
        app.index_scroll = 2;
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 28, 5);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        // Row 0 must NOT be the wave header.
        let r0 = row_text(&buf, area, 0);
        assert!(!r0.contains("Wave 0"), "row 0 should be scrolled past wave header, got: `{r0}`");
        assert!(r0.contains("u0"), "row 0 should be unit u0, got: `{r0}`");
        // Row 1 should be u1.
        let r1 = row_text(&buf, area, 1);
        assert!(r1.contains("u1"), "row 1 should be unit u1, got: `{r1}`");
    }

    #[test]
    fn files_folder_header_is_not_reversed_when_unselected() {
        let g = graph_with_files();
        let app = AppState::new(&g); // default selection = wave_only(0)
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 28, 6);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        let cell = buf.cell((2, 1)).unwrap();
        assert!(!cell.style().add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn long_filename_truncates_with_ellipsis_before_badge() {
        use crate::dag_data::EdgeData;
        let g = WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![
                    NodeData {
                        id: "file:include/platform/threading.h".into(),
                        kind: "file".into(),
                        label: "include/platform/threading.h".into(),
                        recipe: None,
                        command: None,
                        output: None,
                        cached: None,
                        dep_kind: None,
                        group_index: None,
                        modified: Some(true),
                        discovered: None,
                    },
                    unit("unit:a:0", "a", "a0", Some(true)),
                ],
                edges: vec![EdgeData {
                    from: "file:include/platform/threading.h".into(),
                    to: "unit:a:0".into(),
                }],
            }],
            inter_wave_edges: vec![],
        };
        let mut app = AppState::new(&g);
        app.tree.waves[0].files_expanded = true;
        let frame = SnapshotFrame::new(g);
        let area = Rect::new(0, 0, 28, 6);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, &frame);

        // Row 2 = the truncated file row. It must end with the badge `⚠` at
        // the right edge, with `…` somewhere in the label rather than the
        // tail of the filename being clobbered.
        let file_row = row_text(&buf, area, 2);
        assert!(
            file_row.contains('…'),
            "long filename must show the ellipsis truncation marker, got: `{file_row}`",
        );
        assert!(
            file_row.contains('⚠'),
            "badge must remain visible at right edge, got: `{file_row}`",
        );
        // The badge cell should not be clobbered by the label — confirm the
        // last non-space character isn't a slash or letter from the filename.
        let trimmed = file_row.trim_end();
        let last_char = trimmed.chars().last().unwrap_or(' ');
        assert_eq!(last_char, '⚠', "rightmost cell must be the badge, got: `{trimmed}`");
    }
}
