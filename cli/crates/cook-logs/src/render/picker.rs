//! Centred modal listing recent builds (build picker overlay).

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use cook_progress::log_reader::BuildSummary;

pub fn draw(f: &mut Frame, area: Rect, builds: &[BuildSummary], cursor: usize) {
    let modal = centered_rect(60, 60, area);
    f.render_widget(Clear, modal);
    let lines: Vec<Line> = builds
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let prefix = if i == cursor { "▸ " } else { "  " };
            let status = match b.exit_code {
                Some(0) => "exit 0".to_string(),
                Some(c) => format!("exit {c}"),
                None => "exit ?".to_string(),
            };
            let failed = if b.failed_count > 0 {
                format!("  {} failed", b.failed_count)
            } else {
                String::new()
            };
            Line::raw(format!(
                "{prefix}{}  {status}  {} recipes{failed}",
                b.build_id, b.recipe_count
            ))
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" select build ")),
        modal,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    use ratatui::layout::{Constraint, Direction, Layout};
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
