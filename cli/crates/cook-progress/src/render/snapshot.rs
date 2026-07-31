//! Sticky status line — pure data and pure rendering.
//!
//! `StatusSnapshot` is the small struct the tick thread reads atomically
//! via `arc_swap`. `render_status_line` is the pure function it uses to
//! compose the line string. Threading + I/O lives in `status_line.rs`.

use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthStr;

use crate::model::build::BuildState;
use crate::model::node::NodeStatus;
use crate::style::{format_verb, verb_for, LineKind, VERB_COL_WIDTH};
use crate::event::NodeKind;

#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    pub total_nodes: usize,
    pub done_nodes: usize,
    pub running: Vec<RunningEntry>,
    pub started_at: Instant,
}

#[derive(Debug, Clone)]
pub struct RunningEntry {
    pub started_at: Instant,
    pub display: String,
}

#[derive(Debug, Clone, Copy)]
pub struct StatusLineOptions {
    pub colored: bool,
    pub min_nodes: usize,
}

impl Default for StatusLineOptions {
    fn default() -> Self {
        Self { colored: true, min_nodes: 5 }
    }
}

impl StatusSnapshot {
    /// Empty snapshot — the tick thread won't paint until a real one is published.
    pub fn empty() -> Self {
        Self {
            total_nodes: 0,
            done_nodes: 0,
            running: Vec::new(),
            started_at: std::time::Instant::now(),
        }
    }

    pub fn from_state(state: &BuildState) -> Self {
        let total_nodes = state.totals.total_nodes;
        let done_nodes = state.totals.completed_nodes;
        let mut running: Vec<RunningEntry> = state.recipes.values()
            .flat_map(|r| r.nodes.values())
            .filter(|n| n.status == NodeStatus::Running)
            .filter_map(|n| n.started_at.map(|t| RunningEntry { started_at: t, display: n.display() }))
            .collect();
        running.sort_by_key(|e| e.started_at);
        Self {
            total_nodes,
            done_nodes,
            running,
            started_at: state.started_at.unwrap_or_else(Instant::now),
        }
    }
}

/// Safety margin reserved inside `names_budget` for the `+N` overflow suffix
/// and the comma-space separator before it. Empirically chosen.
const NAMES_BUDGET_MARGIN: usize = 2;

/// Pure function: render a status snapshot at a given terminal width.
/// Returns the line WITHOUT a trailing newline. Caller prepends `\r\x1b[2K`.
/// If the snapshot has fewer than `opts.min_nodes` total or `running` is empty,
/// returns an empty string (caller does not draw).
pub fn render_status_line(snap: &StatusSnapshot, opts: StatusLineOptions, cols: usize) -> String {
    if snap.total_nodes < opts.min_nodes { return String::new(); }
    if snap.running.is_empty() { return String::new(); }

    let verb = format_verb(verb_for(LineKind::StatusBar, NodeKind::Cooked), opts.colored);
    let counter = format!("{}/{}", snap.done_nodes, snap.total_nodes);
    let elapsed = fmt_elapsed(snap.started_at.elapsed());

    // Layout:  "<verb> [<bar>] <counter>: <names>    <elapsed>"
    let space = " ";
    let colon = ":";
    let trailing_pad = "    ";
    // Use VERB_COL_WIDTH for the verb's display width (not the actual char count
    // of the formatted string, which may include ANSI escapes when colored=true).
    let fixed = VERB_COL_WIDTH + space.len()
              + 2                            // brackets
              + space.len() + counter.width()
              + colon.len() + space.len()
              + trailing_pad.len() + elapsed.width();

    let inner = cols.saturating_sub(fixed);
    let bar_width = inner.saturating_div(4).clamp(10, 40);
    let names_budget = inner.saturating_sub(bar_width).saturating_sub(NAMES_BUDGET_MARGIN);

    let bar = render_bar(snap.done_nodes, snap.total_nodes, bar_width);
    let names = render_names(&snap.running, names_budget);

    format!("{verb} [{bar}] {counter}: {names}{trailing_pad}{elapsed}")
}

fn render_bar(done: usize, total: usize, width: usize) -> String {
    if width == 0 { return String::new(); }
    if total == 0 { return " ".repeat(width); }
    let filled = ((done as f64 / total as f64) * width as f64).floor() as usize;
    let filled = filled.min(width);

    let mut s = String::with_capacity(width);
    if filled == 0 {
        // Empty bar — all spaces.
    } else if filled == width {
        s.push_str(&"=".repeat(width));
    } else {
        // Partial bar: (filled-1) `=`s followed by `>`.
        s.push_str(&"=".repeat(filled - 1));
        s.push('>');
    }
    let chars_in_s = s.len();    // ASCII content: bytes == chars == width
    s.push_str(&" ".repeat(width.saturating_sub(chars_in_s)));
    s
}

fn render_names(running: &[RunningEntry], budget: usize) -> String {
    if budget == 0 || running.is_empty() { return String::new(); }
    let mut shown = Vec::new();
    let mut used = 0usize;
    for (i, entry) in running.iter().enumerate() {
        let candidate = if i == 0 {
            entry.display.clone()
        } else {
            format!(", {}", entry.display)
        };
        if used + candidate.width() > budget {
            break;
        }
        shown.push(candidate);
        used += shown.last().unwrap().width();
    }
    let remaining = running.len().saturating_sub(shown.len());
    let mut out = shown.join("");
    if remaining > 0 {
        let suffix = format!(", +{remaining}");
        if used + suffix.width() <= budget {
            out.push_str(&suffix);
        }
    }
    out
}

fn fmt_elapsed(d: Duration) -> String {
    // COOK-392: THE duration law.
    cook_contracts::render::duration_ms(d.as_millis() as u64)
}

#[cfg(test)]
#[path = "tests/snapshot_tests.rs"]
mod tests;
