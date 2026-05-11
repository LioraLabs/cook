//! Left-pane tree: recipes with their nodes, with status glyphs.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use cook_progress::model::NodeStatus;

use crate::state::{FlatRow, UiState};
use crate::theme::Theme;

pub fn draw(f: &mut Frame, area: Rect, state: &UiState, theme: &Theme) {
    // Slice the visible window. `ensure_tree_visible` (called from the
    // draw loop) keeps `tree_scroll` in range; we still clamp here so the
    // renderer is robust if called with a stale offset.
    let total = state.flat.len();
    let viewport = area.height as usize;
    let start = state.tree_scroll.min(total.saturating_sub(viewport.max(1)));
    let end = (start + viewport).min(total);

    let mut lines: Vec<Line> = Vec::with_capacity(end - start);
    for i in start..end {
        let row = state.flat[i];
        let line = render_row(state, theme, row);
        let line = if i == state.selected && matches!(state.focus, crate::state::Focus::Tree) {
            apply_selection(line, theme)
        } else {
            line
        };
        lines.push(line);
    }
    let block = Block::default().borders(Borders::RIGHT);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_row<'a>(state: &'a UiState, theme: &Theme, row: FlatRow) -> Line<'a> {
    match row {
        FlatRow::Recipe(rid) => {
            let recipe = state.view.recipes.get(&rid).unwrap();
            let glyph = if state.expanded.contains(&rid) { "⏷" } else { "⏵" };
            let total = recipe.nodes.len();
            // Count successful nodes — use NodeStatus::Completed
            let ok = recipe.nodes.values().filter(|n| n.status == NodeStatus::Completed).count();
            Line::from(vec![
                Span::raw(format!("{glyph} ")),
                Span::raw(recipe.name.clone()),
                Span::styled(format!("   {}/{}", ok, total), theme.dim_style()),
            ])
        }
        FlatRow::Node(rid, nid) => {
            let node = &state.view.recipes[&rid].nodes[&nid];
            let (g, style) = status_glyph(theme, node.status);
            let dur = node.elapsed_ms
                .map(|ms| format!("  ·  {:.1}s", ms as f64 / 1000.0))
                .unwrap_or_default();
            Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{g} "), style),
                Span::raw(node.name.clone()),
                Span::styled(dur, theme.dim_style()),
            ])
        }
    }
}

fn status_glyph(theme: &Theme, status: NodeStatus) -> (&'static str, ratatui::style::Style) {
    // Use the real variants. Exhaustive match.
    match status {
        NodeStatus::Completed => ("✓", theme.ok_style()),
        NodeStatus::Failed => ("✗", theme.err_style()),
        NodeStatus::Skipped => ("⏭", theme.skip_style()),
        NodeStatus::Running => ("●", theme.header_style()),
        NodeStatus::Waiting => ("○", theme.dim_style()),
        NodeStatus::Unknown => ("?", theme.dim_style()),
    }
}

fn apply_selection<'a>(line: Line<'a>, theme: &Theme) -> Line<'a> {
    let sel = theme.selection_style();
    Line::from(line.spans.into_iter().map(|s| s.patch_style(sel)).collect::<Vec<_>>())
}
