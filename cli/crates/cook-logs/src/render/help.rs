//! `?` help overlay listing keybindings.

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use cook_progress::log_reader::LoadDiagnostics;

pub fn draw(f: &mut Frame, area: Rect, diag: &LoadDiagnostics) {
    let modal = centered(70, 70, area);
    f.render_widget(Clear, modal);
    let mut lines = vec![
        Line::raw("j/k       move selection"),
        Line::raw("h/l       collapse/expand"),
        Line::raw("Tab       switch tree/output pane"),
        Line::raw("g/G       top/bottom of output"),
        Line::raw("Ctrl-u/d  scroll output"),
        Line::raw("/         search (across nodes)"),
        Line::raw("n/N       next/prev match"),
        Line::raw("f         cycle filter: all → failed → has-stderr"),
        Line::raw("t         toggle timestamp gutter"),
        Line::raw("w         toggle soft wrap"),
        Line::raw("b         build picker"),
        Line::raw("r         reload build from disk"),
        Line::raw("y         yank log to clipboard (OSC 52)"),
        Line::raw("q/Esc     quit"),
        Line::raw(""),
    ];
    if diag.events_jsonl_missing {
        lines.push(Line::raw(
            "note: events.jsonl missing — using .log fallback (statuses unknown)",
        ));
    }
    if diag.skipped_jsonl_lines > 0 {
        lines.push(Line::raw(format!(
            "note: {} events skipped during load",
            diag.skipped_jsonl_lines
        )));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" help ")),
        modal,
    );
}

fn centered(px: u16, py: u16, r: Rect) -> Rect {
    use ratatui::layout::{Constraint, Direction, Layout};
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - py) / 2),
            Constraint::Percentage(py),
            Constraint::Percentage((100 - py) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - px) / 2),
            Constraint::Percentage(px),
            Constraint::Percentage((100 - px) / 2),
        ])
        .split(v[1])[1]
}
