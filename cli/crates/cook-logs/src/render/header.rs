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

fn duration_str(started: &str, ended: Option<&str>) -> String {
    let Some(end) = ended else { return "(running…)".into() };
    let parse = |s: &str| -> Option<i64> {
        let s = s.trim_end_matches('Z');
        let (date, time) = s.split_once('T')?;
        let (y, rest) = date.split_once('-')?;
        let (mo, d) = rest.split_once('-')?;
        let mut parts = time.splitn(3, ':');
        let h: i64 = parts.next()?.parse().ok()?;
        let mi: i64 = parts.next()?.parse().ok()?;
        let s_str = parts.next()?;
        let (secs, frac_ms) = match s_str.split_once('.') {
            Some((s, f)) => {
                let s: i64 = s.parse().ok()?;
                let mut f = f.to_string();
                f.truncate(3);
                while f.len() < 3 { f.push('0'); }
                let f: i64 = f.parse().ok()?;
                (s, f)
            }
            None => (s_str.parse().ok()?, 0i64),
        };
        let y: i64 = y.parse().ok()?;
        let mo: i64 = mo.parse().ok()?;
        let d: i64 = d.parse().ok()?;
        Some(((y*365 + mo*31 + d)*86400 + h*3600 + mi*60 + secs)*1000 + frac_ms)
    };
    let (Some(a), Some(b)) = (parse(started), parse(end)) else {
        return "(unknown duration)".into();
    };
    let ms = (b - a).max(0);
    if ms < 1000 { format!("{}ms", ms) }
    else if ms < 60_000 { format!("{:.1}s", ms as f64 / 1000.0) }
    else { format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1000) }
}
