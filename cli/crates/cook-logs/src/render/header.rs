//! Top status bar: `build-id · status · duration · N recipes · failed:M`.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use cook_progress::model::NodeStatus;

use crate::state::UiState;
use crate::theme::Theme;

pub fn draw(f: &mut Frame, area: Rect, state: &UiState, theme: &Theme) {
    let view = &state.view;
    let total_recipes = view.recipes.len();
    let failed_nodes: usize = view.recipes.values()
        .flat_map(|r| r.nodes.values())
        .filter(|n| n.status == NodeStatus::Failed)
        .count();

    let status_text = match view.exit_code {
        Some(0) => "✓ passed",
        Some(_) => "✗ failed",
        None    => "… unknown",
    };
    let status_style = match view.exit_code {
        Some(0) => theme.ok_style(),
        Some(_) => theme.err_style(),
        None    => Style::default(),
    };

    let duration_text = duration_str(&view.started_at, view.ended_at.as_deref());

    let line = Line::from(vec![
        Span::styled(view.build_id.clone(), theme.header_style()),
        Span::raw("  ·  "),
        Span::styled(status_text, status_style),
        Span::raw("  ·  "),
        Span::raw(duration_text),
        Span::raw("  ·  "),
        Span::raw(format!("{} recipes", total_recipes)),
        Span::raw("  ·  "),
        Span::styled(format!("failed:{}", failed_nodes),
            if failed_nodes > 0 { theme.err_style() } else { theme.dim_style() }),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// `ended - started` over the RFC-3339 timestamps `cook-progress` wrote.
///
/// Both the parse and the rendering are law with more than one end
/// (`cook_contracts::timestamp`, `cook_contracts::render`): this crate reads
/// what cook-progress wrote, so a private parse here is half a wire format.
/// It used to be exactly that, and it faked the calendar as
/// `(y*365 + m*31 + d)`, which made a one-second build spanning 28 February
/// read as 72 hours (COOK-421).
fn duration_str(started: &str, ended: Option<&str>) -> String {
    let Some(end) = ended else { return "(running…)".into() };
    let (Some(a), Some(b)) = (
        cook_contracts::timestamp::parse_rfc3339_ms(started),
        cook_contracts::timestamp::parse_rfc3339_ms(end),
    ) else {
        return "(unknown duration)".into();
    };
    // COOK-392: THE duration law.
    cook_contracts::render::duration_ms((b - a).max(0) as u64)
}

#[cfg(test)]
#[path = "../tests/header_tests.rs"]
mod tests;
