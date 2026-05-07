//! Help, detail, and edge-picker overlays. See design spec §Selection & Edge
//! Jumps and §Help.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::state::{EdgePicker, PickerDir};

pub fn render_edge_picker(area: Rect, buf: &mut Buffer, picker: &EdgePicker) {
    let title = match picker.direction {
        PickerDir::Downstream => " Downstream consumers ",
        PickerDir::Upstream => " Upstream inputs ",
    };
    let popup = centered_rect(60, picker.candidates.len() as u16 + 2, area);
    Clear.render(popup, buf);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Plain);
    let inner = block.inner(popup);
    block.render(popup, buf);

    for (i, id) in picker.candidates.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let style = if i == picker.cursor {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let line = format!(" {}. {}", i + 1, id);
        let max = inner.x + inner.width;
        let mut col = inner.x;
        for ch in line.chars() {
            if col >= max {
                break;
            }
            if let Some(cell) = buf.cell_mut((col, y)) {
                cell.set_char(ch).set_style(style);
            }
            col += 1;
        }
    }
}

pub fn render_help(area: Rect, buf: &mut Buffer) {
    const HELP: &[&str] = &[
        " ?  show this help                              ",
        " q  quit                                        ",
        " j/k  move cursor up/down in index              ",
        " h/l  collapse / expand current node            ",
        " g/G  top / bottom of index                     ",
        " ]/[  jump downstream / upstream                ",
        " H J K L  pan camera (½-viewport)               ",
        " ctrl+arrows  pan camera (½-viewport)           ",
        " c  re-center camera, re-engage follow          ",
        " a  auto-fit camera to canvas                   ",
        " /  fuzzy search                                ",
        " n/N  next / prev search match                  ",
        " v  full-screen detail overlay                  ",
        " r  refresh (snapshot mode)                     ",
        " esc  close overlay                             ",
    ];
    let popup = centered_rect(56, HELP.len() as u16 + 2, area);
    Clear.render(popup, buf);
    let block = Block::default().title(" Help ").borders(Borders::ALL);
    let inner = block.inner(popup);
    block.render(popup, buf);
    for (i, line) in HELP.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let mut col = inner.x;
        for ch in line.chars() {
            if col >= inner.x + inner.width {
                break;
            }
            if let Some(cell) = buf.cell_mut((col, y)) {
                cell.set_char(ch).set_style(Style::default());
            }
            col += 1;
        }
    }
}

pub fn render_detail_overlay<F: crate::frame::ViewFrame>(
    area: Rect,
    buf: &mut Buffer,
    app: &crate::state::AppState,
    frame: &F,
) {
    let popup = centered_rect(area.width - 4, area.height - 4, area);
    Clear.render(popup, buf);
    let block = Block::default().title(" Detail ").borders(Borders::ALL);
    let inner = block.inner(popup);
    block.render(popup, buf);
    crate::render::detail::render(inner, buf, app, frame);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}
