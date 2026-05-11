//! Right-pane output: lines from the selected node, ANSI-aware,
//! with optional timestamp gutter and wrap toggle.

use ansi_to_tui::IntoText;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use cook_progress::event::Stream;
use cook_progress::log_reader::LogLine;

use crate::state::UiState;
use crate::theme::Theme;

pub fn draw(f: &mut Frame, area: Rect, state: &UiState, theme: &Theme) {
    let block = Block::default().borders(Borders::NONE);
    let lines = build_lines(state, theme);
    let mut para = Paragraph::new(Text::from(lines))
        .block(block)
        .scroll((state.scroll_y, 0));
    if state.soft_wrap {
        para = para.wrap(Wrap { trim: false });
    }
    f.render_widget(para, area);
}

fn build_lines<'a>(state: &'a UiState, theme: &Theme) -> Vec<Line<'a>> {
    let Some((rid, nid)) = state.selected_node() else { return vec![] };
    let Some(recipe) = state.view.recipes.get(&rid) else { return vec![] };
    let Some(node) = recipe.nodes.get(&nid) else { return vec![] };

    let mut lines: Vec<Line<'a>> = Vec::with_capacity(node.lines.len() + 2);
    let label = format!(
        "{}/{}{}",
        recipe.name,
        node.name,
        node.elapsed_ms.map(|ms| format!("  ·  {:.1}s", ms as f64 / 1000.0)).unwrap_or_default(),
    );
    lines.push(Line::styled(label, theme.dim_style()));
    lines.push(Line::raw(""));

    // Collect line indices that are search matches for the currently selected node.
    let matched_lines: std::collections::BTreeSet<usize> = state.search.as_ref()
        .filter(|s| !s.editing)
        .map(|s| {
            s.matches.iter()
                .filter(|(r, n, _)| *r == rid && *n == nid)
                .map(|(_, _, i)| *i)
                .collect()
        })
        .unwrap_or_default();

    for (i, log) in node.lines.iter().enumerate() {
        let line = render_line(log, state, theme);
        if matched_lines.contains(&i) {
            let style = ratatui::style::Style::default()
                .bg(ratatui::style::Color::Yellow)
                .fg(ratatui::style::Color::Black);
            let spans = line.spans.into_iter().map(|s| s.patch_style(style)).collect::<Vec<_>>();
            lines.push(Line::from(spans));
        } else {
            lines.push(line);
        }
    }
    lines
}

fn render_line<'a>(log: &'a LogLine, state: &UiState, theme: &Theme) -> Line<'a> {
    let parsed = log.text.as_bytes().into_text().unwrap_or_else(|_| Text::raw(log.text.clone()));
    let mut spans: Vec<Span<'a>> = parsed.lines.into_iter().flat_map(|l| l.spans).collect();
    if log.stream == Stream::Stderr {
        let style = theme.err_style();
        spans = spans.into_iter().map(|s| s.patch_style(style)).collect();
    }
    if state.show_timestamps {
        let ts_text = ts_to_short(&log.ts);
        let mut out = vec![Span::styled(format!("{ts_text} "), theme.dim_style())];
        out.extend(spans);
        Line::from(out)
    } else {
        Line::from(spans)
    }
}

fn ts_to_short(ts: &Option<String>) -> String {
    let Some(s) = ts else { return "--:--:--.---".into() };
    s.split('T').nth(1)
        .map(|t| t.trim_end_matches('Z').to_string())
        .unwrap_or_else(|| s.clone())
}
