//! Artifact status strip — compact one-line summary of recipe progress.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::node::{NodeState, NodeStatus};
use crate::model::recipe::RecipeState;

const JOIN: &str = " · ";
const SUFFIX_GUARD: usize = 5;
const MAX_COMPLETED: usize = 3;
const MAX_WAITING: usize = 2;
const MAX_DISPLAY_LEN: usize = 20;

pub fn artifact_strip(recipe: &RecipeState, cols: usize) -> String {
    let (pills, pre_dropped) = build_pills(recipe);
    let cached_prefix = if recipe.cached_count > 0 {
        format!("{} cached", recipe.cached_count)
    } else {
        String::new()
    };

    let budget = cols.saturating_sub(SUFFIX_GUARD);
    fit(&cached_prefix, &pills, pre_dropped, budget)
}

#[derive(Debug, Clone)]
struct Pill {
    symbol: &'static str,
    text: String,
    priority: u8, // lower = drop first
}

/// Returns (pills, pre_dropped) where pre_dropped is the count of nodes omitted
/// before they even entered the pill list (due to MAX_WAITING / MAX_COMPLETED caps).
fn build_pills(recipe: &RecipeState) -> (Vec<Pill>, usize) {
    let mut completed: Vec<&NodeState> = recipe
        .nodes
        .values()
        .filter(|n| n.status == NodeStatus::Completed)
        .collect();
    completed.sort_by_key(|n| n.completed_at);
    let completed_total = completed.len();
    let completed: Vec<&NodeState> = completed.iter().rev().take(MAX_COMPLETED).rev().copied().collect();

    let mut running: Vec<&NodeState> = recipe
        .nodes
        .values()
        .filter(|n| n.status == NodeStatus::Running)
        .collect();
    running.sort_by_key(|n| n.started_at);

    let mut failed: Vec<&NodeState> = recipe
        .nodes
        .values()
        .filter(|n| n.status == NodeStatus::Failed)
        .collect();
    failed.sort_by_key(|n| n.completed_at);

    let all_waiting: Vec<&NodeState> = recipe
        .nodes
        .values()
        .filter(|n| n.status == NodeStatus::Waiting)
        .collect();
    let waiting_total = all_waiting.len();
    let waiting: Vec<&NodeState> = all_waiting.into_iter().take(MAX_WAITING).collect();

    // Nodes capped by collection limits that never enter the pill list.
    let pre_dropped = completed_total.saturating_sub(MAX_COMPLETED)
        + waiting_total.saturating_sub(MAX_WAITING);

    let mut pills = Vec::new();
    for n in completed {
        pills.push(Pill { symbol: "✓", text: truncate(&n.display()), priority: 1 });
    }
    for n in failed {
        pills.push(Pill { symbol: "✗", text: truncate(&n.display()), priority: 4 });
    }
    for n in running {
        pills.push(Pill { symbol: "◆", text: truncate(&n.display()), priority: 3 });
    }
    for n in waiting {
        pills.push(Pill { symbol: "◇", text: truncate(&n.display()), priority: 0 });
    }
    (pills, pre_dropped)
}

fn truncate(s: &str) -> String {
    if s.width() <= MAX_DISPLAY_LEN {
        s.to_string()
    } else {
        let mut out = String::new();
        let mut w = 0;
        for c in s.chars() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(0);
            if w + cw + 1 > MAX_DISPLAY_LEN { break; }
            out.push(c);
            w += cw;
        }
        out.push('…');
        out
    }
}

fn fit(cached_prefix: &str, pills: &[Pill], pre_dropped: usize, budget: usize) -> String {
    let mut ordered_indices: Vec<usize> = (0..pills.len()).collect();
    // Drop order: lowest priority first, within a priority drop by ascending index (oldest first).
    ordered_indices.sort_by(|&a, &b| pills[a].priority.cmp(&pills[b].priority).then(a.cmp(&b)));

    let mut included: Vec<bool> = vec![true; pills.len()];
    let rendered = |included: &[bool]| -> String {
        let parts: Vec<String> = pills
            .iter()
            .enumerate()
            .filter(|(i, _)| included[*i])
            .map(|(_, p)| format!("{} {}", p.symbol, p.text))
            .collect();

        let fit_dropped = included.iter().filter(|x| !**x).count();
        let total_dropped = fit_dropped + pre_dropped;
        let mut s = String::new();
        if !cached_prefix.is_empty() {
            s.push_str(cached_prefix);
            if !parts.is_empty() {
                s.push_str(JOIN);
            }
        }
        s.push_str(&parts.join(JOIN));
        if total_dropped > 0 {
            if !s.is_empty() { s.push(' '); }
            s.push_str(&format!("+{total_dropped}"));
        }
        s
    };

    let mut drop_cursor = 0;
    while rendered(&included).width() > budget && drop_cursor < ordered_indices.len() {
        let idx = ordered_indices[drop_cursor];
        included[idx] = false;
        drop_cursor += 1;
    }
    rendered(&included)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{NodeId, RecipeId};
    use std::path::PathBuf;
    use std::time::Instant;

    fn recipe_with(
        cached: usize,
        completed: &[&str],
        running: &[&str],
        waiting: &[&str],
    ) -> RecipeState {
        let mut r = RecipeState::new(RecipeId::new(0), "lib".into(), vec![], 1);
        r.cached_count = cached;
        let mut next = 0u32;
        let now = Instant::now();
        for (i, name) in completed.iter().enumerate() {
            let mut n = NodeState::new(
                NodeId::new(next),
                (*name).to_string(),
                Some(PathBuf::from(format!("build/{name}"))),
                String::new(),
            );
            n.status = NodeStatus::Completed;
            n.completed_at = Some(now + std::time::Duration::from_millis(i as u64));
            r.nodes.insert(NodeId::new(next), n);
            next += 1;
        }
        for (i, name) in running.iter().enumerate() {
            let mut n = NodeState::new(
                NodeId::new(next),
                (*name).to_string(),
                Some(PathBuf::from(format!("build/{name}"))),
                String::new(),
            );
            n.status = NodeStatus::Running;
            n.started_at = Some(now + std::time::Duration::from_millis(i as u64));
            r.nodes.insert(NodeId::new(next), n);
            next += 1;
        }
        for name in waiting.iter() {
            let n = NodeState::new(
                NodeId::new(next),
                (*name).to_string(),
                Some(PathBuf::from(format!("build/{name}"))),
                String::new(),
            );
            r.nodes.insert(NodeId::new(next), n);
            next += 1;
        }
        r
    }

    #[test]
    fn simple_no_overflow() {
        let r = recipe_with(0, &[], &["lua.o"], &["lua.bin"]);
        let s = artifact_strip(&r, 80);
        assert!(s.contains("◆ lua.o"));
        assert!(s.contains("◇ lua.bin"));
    }

    #[test]
    fn cached_prefix_shown_when_nonzero() {
        let r = recipe_with(27, &[], &["ldo.o"], &[]);
        let s = artifact_strip(&r, 80);
        assert!(s.starts_with("27 cached"));
    }

    #[test]
    fn overflow_drops_waiting_first() {
        let waiting: Vec<&str> = (0..50).map(|_| "waitx").collect();
        let r = recipe_with(0, &[], &["a"], &waiting);
        let s = artifact_strip(&r, 40);
        assert!(s.contains("◆ a"));
        assert!(s.contains("+"), "should have +N drop marker: {s}");
    }

    #[test]
    fn running_pills_are_never_dropped_before_waiting() {
        let running: Vec<&str> = (0..10).map(|_| "run").collect();
        let waiting: Vec<&str> = (0..5).map(|_| "wait").collect();
        let r = recipe_with(0, &[], &running, &waiting);
        let s = artifact_strip(&r, 40);
        // at least one running pill remains even at narrow width
        assert!(s.contains("◆ run"), "got: {s}");
    }

    #[test]
    fn long_artifact_name_is_truncated() {
        let r = recipe_with(0, &[], &["a_very_long_artifact_name_exceeding_twenty.o"], &[]);
        let s = artifact_strip(&r, 120);
        assert!(s.contains("…"));
    }

    #[test]
    fn cached_plus_all_dropped_emits_clean_marker() {
        // cached_count=5, all waiting nodes dropped by collection cap, no pills survive fit.
        let waiting: Vec<&str> = (0..5).map(|_| "wait").collect();
        let r = recipe_with(5, &[], &[], &waiting);
        let s = artifact_strip(&r, 15);
        // no spurious " · " or double spaces
        assert!(!s.contains(" ·  "), "unexpected double-separator: {s:?}");
        assert!(!s.contains("·  "), "unexpected separator+space: {s:?}");
        assert!(s.contains("+"), "expected +N marker: {s:?}");
    }

    #[test]
    fn failed_pill_outranks_running_under_pressure() {
        // priority: failed > running. At very narrow width only the highest-priority pills survive.
        let mut r = RecipeState::new(RecipeId::new(0), "lib".into(), vec![], 1);
        let now = Instant::now();
        // one running
        let mut n1 = NodeState::new(NodeId::new(0), "run".into(), Some(PathBuf::from("build/r")), String::new());
        n1.status = NodeStatus::Running;
        n1.started_at = Some(now);
        r.nodes.insert(NodeId::new(0), n1);
        // one failed
        let mut n2 = NodeState::new(NodeId::new(1), "fail".into(), Some(PathBuf::from("build/f")), String::new());
        n2.status = NodeStatus::Failed;
        n2.completed_at = Some(now);
        r.nodes.insert(NodeId::new(1), n2);

        // Budget that only fits one pill.
        let s = artifact_strip(&r, 15);
        assert!(s.contains("✗"), "failed pill must survive before running at narrow width: {s:?}");
    }
}
