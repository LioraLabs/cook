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
#[path = "tests/index_tests.rs"]
mod tests;
