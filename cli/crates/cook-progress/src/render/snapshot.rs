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
    let secs = d.as_secs_f64();
    if secs < 60.0 { format!("{secs:.1}s") }
    else if secs < 3600.0 { format!("{}m{:02}s", (secs as u64) / 60, (secs as u64) % 60) }
    else { format!("{}h{:02}m{:02}s",
        (secs as u64) / 3600,
        ((secs as u64) % 3600) / 60,
        (secs as u64) % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn entry(name: &str, secs_ago: u64) -> RunningEntry {
        let started = Instant::now() - Duration::from_secs(secs_ago);
        RunningEntry { started_at: started, display: name.into() }
    }

    fn snapshot(total: usize, done: usize, running: &[(&str, u64)], elapsed: u64) -> StatusSnapshot {
        StatusSnapshot {
            total_nodes: total,
            done_nodes: done,
            running: running.iter().map(|(n, s)| entry(n, *s)).collect(),
            started_at: Instant::now() - Duration::from_secs(elapsed),
        }
    }

    #[test]
    fn empty_when_no_running_work() {
        let s = snapshot(47, 47, &[], 4);
        let line = render_status_line(&s, StatusLineOptions { colored: false, ..Default::default() }, 100);
        assert_eq!(line, "");
    }

    #[test]
    fn empty_when_total_below_threshold() {
        let s = snapshot(2, 0, &[("a.c", 0), ("b.c", 0)], 1);
        let line = render_status_line(&s, StatusLineOptions { colored: false, min_nodes: 5 }, 100);
        assert_eq!(line, "");
    }

    #[test]
    fn renders_verb_bar_counter_names_elapsed() {
        let s = snapshot(47, 14, &[("lvm.o", 1), ("ldebug.o", 1), ("lcode.o", 0)], 2);
        let line = render_status_line(&s, StatusLineOptions { colored: false, ..Default::default() }, 120);
        assert!(line.contains("Cooking"));
        assert!(line.contains("14/47"));
        assert!(line.contains("lvm.o"));
        assert!(line.contains("ldebug.o"));
        assert!(line.contains("lcode.o"));
        // 2 secs of elapsed.
        assert!(line.contains("2.0s"), "got: {line}");
    }

    #[test]
    fn names_overflow_emits_plus_n() {
        let names: Vec<(&str, u64)> = (0..8).map(|i|
            (Box::leak(format!("file_with_long_name_{i}.o").into_boxed_str()) as &str, 0)
        ).collect();
        let s = snapshot(47, 14, &names, 1);
        let line = render_status_line(&s, StatusLineOptions { colored: false, ..Default::default() }, 80);
        assert!(line.contains("+"), "expected overflow indicator: {line}");
    }
}
