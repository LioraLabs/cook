//! Inline renderer — indicatif MultiProgress driving the live frame.

use std::collections::BTreeMap;
use std::io;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use unicode_width::UnicodeWidthStr;

use crate::event::{ProgressEvent, RecipeId};
use crate::model::build::BuildState;
use crate::model::recipe::Status;
use crate::render::Renderer;

pub struct InlineRenderer {
    multi: MultiProgress,
    recipe_bars: BTreeMap<RecipeId, ProgressBar>,
    footer: Option<ProgressBar>,
    pending_resume: bool,
    original_target: Option<ProgressDrawTarget>,
}

impl InlineRenderer {
    pub fn new(draw_target: ProgressDrawTarget) -> Self {
        let multi = MultiProgress::with_draw_target(draw_target);
        Self {
            multi,
            recipe_bars: BTreeMap::new(),
            footer: None,
            pending_resume: false,
            original_target: None,
        }
    }

    fn create_waiting_bar(&self, name: &str, deps: &str) -> ProgressBar {
        let bar = self.multi.add(ProgressBar::new(0));
        bar.set_style(ProgressStyle::with_template("{prefix} {msg}").unwrap());
        bar.set_prefix(format!("◇ {:10}", name));
        bar.set_message(if deps.is_empty() {
            "waiting".to_string()
        } else {
            format!("waiting  ← {deps}")
        });
        bar
    }

    fn style_running() -> ProgressStyle {
        ProgressStyle::with_template(
            "{prefix} {bar:40.cyan/dim} {pos}/{len} · {elapsed}\n    {msg}",
        )
        .unwrap()
        .progress_chars("━━━")
    }

    fn style_oneline() -> ProgressStyle {
        ProgressStyle::with_template("{prefix} {msg}").unwrap()
    }

    fn deps_str(&self, state: &BuildState, deps: &[RecipeId]) -> String {
        let names: Vec<&str> = deps.iter()
            .filter_map(|d| state.recipes.get(d).map(|r| r.name.as_str()))
            .collect();
        if names.is_empty() { String::new() } else { format!("  ← {}", names.join(", ")) }
    }

    fn refresh_bar(&self, state: &BuildState, id: RecipeId) {
        let Some(bar) = self.recipe_bars.get(&id) else { return };
        let Some(r) = state.recipes.get(&id) else { return };
        let deps = self.deps_str(state, &r.deps);
        match r.status {
            Status::Waiting => {
                bar.set_style(Self::style_oneline());
                bar.set_prefix(format!("◇ {:10}", r.name));
                bar.set_message(if deps.is_empty() {
                    "waiting".into()
                } else {
                    format!("waiting{deps}")
                });
            }
            Status::Running => {
                bar.set_style(Self::style_running());
                bar.set_length(r.progress.1 as u64);
                bar.set_position(r.progress.0 as u64);
                bar.set_prefix(format!("◆ {}", r.name));
                let strip = crate::strip::artifact_strip(r, 100);
                let msg = if deps.is_empty() { strip } else { format!("{strip}{deps}") };
                bar.set_message(msg);
            }
            Status::Completed => {
                bar.set_style(Self::style_oneline());
                bar.set_prefix(format!("✓ {}", r.name));
                let secs = r.elapsed.unwrap_or_default().as_secs_f64();
                let cached = if r.cached_count > 0 {
                    format!(" · {} cached", r.cached_count)
                } else {
                    String::new()
                };
                let header = format!("{}/{} · {secs:.1}s{cached}", r.progress.1, r.progress.1);
                let strip = crate::strip::artifact_strip(r, 100);
                let msg = if strip.is_empty() {
                    header
                } else {
                    format!("{header}\n    {strip}")
                };
                bar.set_message(msg);
            }
            Status::Cached => {
                bar.set_style(Self::style_oneline());
                bar.set_prefix(format!("≋ {}", r.name));
                bar.set_message(format!("{}/{} cached", r.progress.1, r.progress.1));
            }
            Status::Failed => {
                bar.set_style(Self::style_oneline());
                bar.set_prefix(format!("✗ {}", r.name));
                let secs = r.elapsed.unwrap_or_default().as_secs_f64();
                let header = format!("{}/{} · {secs:.1}s{deps}", r.progress.0, r.progress.1);
                let strip = crate::strip::artifact_strip(r, 100);
                let msg = if strip.is_empty() {
                    header
                } else {
                    format!("{header}\n    {strip}")
                };
                bar.set_message(msg);
            }
        }
    }

    fn refresh_footer(&self, state: &BuildState) {
        let Some(footer) = &self.footer else { return };
        let t = &state.totals;
        let secs = state.elapsed().as_secs_f64();
        let mut parts = Vec::new();
        if t.running > 0 { parts.push(format!("{} running", t.running)); }
        if t.done > 0 { parts.push(format!("{} done", t.done)); }
        if t.waiting > 0 { parts.push(format!("{} waiting", t.waiting)); }
        if t.total_nodes > 0 { parts.push(format!("{}/{} cached", t.cached, t.total_nodes)); }
        parts.push(format!("{secs:.1}s"));
        footer.set_message(parts.join(" · "));
    }
}

impl Renderer for InlineRenderer {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        // If we are waiting to resume after InteractiveEnd, decide now.
        if self.pending_resume {
            match event {
                ProgressEvent::Finished { .. } => {
                    // Stay frozen; no resume.
                }
                _ => {
                    self.pending_resume = false;
                    let target = self.original_target.take()
                        .unwrap_or_else(ProgressDrawTarget::stderr);
                    self.multi = MultiProgress::with_draw_target(target);
                    self.recipe_bars.clear();
                    for id in &state.order {
                        let r = &state.recipes[id];
                        let deps = self.deps_str(state, &r.deps);
                        let bar = self.create_waiting_bar(&r.name, deps.trim_start_matches("  ← "));
                        self.recipe_bars.insert(*id, bar);
                    }
                    let footer = self.multi.add(ProgressBar::new(0));
                    footer.set_style(ProgressStyle::with_template("{msg}").unwrap());
                    self.footer = Some(footer);
                    for id in &state.order {
                        self.refresh_bar(state, *id);
                    }
                }
            }
        }

        // Handle interactive start: freeze bars by switching to hidden draw target.
        if let ProgressEvent::InteractiveStart { recipe, node } = event {
            let rname = state.recipes.get(recipe).map(|r| r.name.as_str()).unwrap_or("?");
            let nname = state.recipes.get(recipe)
                .and_then(|r| r.nodes.get(node))
                .map(|n| n.name.as_str()).unwrap_or("?");
            let _ = self.multi.println(format!("─── {rname}/{nname} (interactive) ───"));
            let _ = self.multi.println("");
            self.multi.set_draw_target(ProgressDrawTarget::hidden());
            return Ok(());
        }

        // Handle interactive end: flag pending resume; do not draw yet.
        if let ProgressEvent::InteractiveEnd { .. } = event {
            self.pending_resume = true;
            return Ok(());
        }

        // Build-started: create bars in waiting state, add footer.
        if let ProgressEvent::BuildStarted { .. } = event {
            for id in &state.order {
                let r = &state.recipes[id];
                let deps: Vec<&str> = r.deps.iter()
                    .filter_map(|d| state.recipes.get(d).map(|x| x.name.as_str()))
                    .collect();
                let deps_str = deps.join(", ");
                let bar = self.create_waiting_bar(&r.name, &deps_str);
                self.recipe_bars.insert(*id, bar);
            }
            let footer = self.multi.add(ProgressBar::new(0));
            footer.set_style(ProgressStyle::with_template("{msg}").unwrap());
            self.footer = Some(footer);
        }

        // Per-recipe events: refresh the corresponding bar.
        match event {
            ProgressEvent::RecipeStarted { recipe }
            | ProgressEvent::RecipeCompleted { recipe, .. }
            | ProgressEvent::RecipeFailed { recipe, .. }
            | ProgressEvent::NodeStarted { recipe, .. }
            | ProgressEvent::NodeCompleted { recipe, .. }
            | ProgressEvent::NodeFailed { recipe, .. }
            | ProgressEvent::NodeCacheHit { recipe, .. }
            | ProgressEvent::NodeSkipped { recipe, .. } => {
                self.refresh_bar(state, *recipe);
            }
            _ => {}
        }

        // Failure block: print fenced error block above live bars.
        if let ProgressEvent::RecipeFailed { recipe, .. } = event {
            if let Some(r) = state.recipes.get(recipe) {
                if let Some(err) = &r.error_summary {
                    let width: usize = 80;
                    let header = format!("─── {} · failed ─", r.name);
                    let dashes_needed = width.saturating_sub(header.width());
                    let dashes = "─".repeat(dashes_needed);
                    let _ = self.multi.println(format!("\n{header}{dashes}"));
                    for line in err.lines() {
                        let _ = self.multi.println(format!("│  {line}"));
                    }
                    let _ = self.multi.println("─".repeat(width));
                }
            }
        }

        self.refresh_footer(state);
        Ok(())
    }

    fn finish(&mut self, _state: &BuildState) -> io::Result<()> {
        for bar in self.recipe_bars.values() { bar.finish(); }
        if let Some(f) = &self.footer { f.finish_and_clear(); }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{NodeId, RecipeTopo};
    use crate::render::test_term::TestTerm;
    use std::time::Duration;

    fn drive(events: &[ProgressEvent]) -> (BuildState, TestTerm) {
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();
        for ev in events {
            state.apply(ev);
            inline.handle(&state, ev).unwrap();
        }
        inline.finish(&state).unwrap();
        (state, term)
    }

    #[test]
    fn build_started_creates_a_bar_per_recipe() {
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();
        let ev = ProgressEvent::BuildStarted {
            recipes: vec![
                RecipeTopo { id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes: 3 },
                RecipeTopo { id: RecipeId::new(1), name: "lib".into(), deps: vec![RecipeId::new(0)], expected_nodes: 5 },
            ],
            total_nodes: 8,
        };
        state.apply(&ev);
        inline.handle(&state, &ev).unwrap();
        inline.finish(&state).unwrap();
        assert_eq!(inline.recipe_bars.len(), 2);
    }

    #[test]
    fn running_recipe_transitions_from_waiting() {
        let events = vec![
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 2 }],
                total_nodes: 2,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "a.c".into(), artifact: Some("build/a.o".into()),
                fallback_label: "clang a.c".into(),
            },
        ];
        let (state, _term) = drive(&events);
        assert_eq!(state.recipes[&RecipeId::new(0)].status, Status::Running);
    }

    #[test]
    fn completed_recipe_shows_cached_suffix_when_any_cached() {
        let events = vec![
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes: 2 }],
                total_nodes: 2,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeCacheHit {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "pkg".into(), artifact: None,
            },
            ProgressEvent::RecipeCompleted {
                recipe: RecipeId::new(0),
                elapsed: Duration::from_millis(100),
                cached: 1, total: 2,
            },
        ];
        let (state, _term) = drive(&events);
        assert_eq!(state.recipes[&RecipeId::new(0)].status, Status::Completed);
        assert_eq!(state.recipes[&RecipeId::new(0)].cached_count, 1);
    }

    #[test]
    fn failure_emits_fenced_error_block() {
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();

        for ev in [
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 1 }],
                total_nodes: 1,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "x.c".into(), artifact: None, fallback_label: "clang x.c".into(),
            },
            ProgressEvent::NodeFailed {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                elapsed: Duration::from_millis(50),
                error: "boom: syntax error".into(),
            },
            ProgressEvent::RecipeFailed {
                recipe: RecipeId::new(0),
                elapsed: Duration::from_millis(100),
                completed: 1, total: 1,
            },
        ].iter() {
            state.apply(ev);
            inline.handle(&state, ev).unwrap();
        }
        inline.finish(&state).unwrap();

        let out = term.contents();
        assert!(out.contains("lib · failed"), "expected failure header; got:\n{out}");
        assert!(out.contains("boom: syntax error"), "expected error message; got:\n{out}");
    }

    #[test]
    fn interactive_start_freezes_and_end_flags_resume() {
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();

        let events = [
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 1 }],
                total_nodes: 1,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "repl".into(), artifact: None, fallback_label: "gdb".into(),
            },
            ProgressEvent::InteractiveStart { recipe: RecipeId::new(0), node: NodeId::new(0) },
            ProgressEvent::InteractiveEnd {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                elapsed: Duration::from_millis(10), success: true,
            },
        ];
        for ev in &events {
            state.apply(ev);
            inline.handle(&state, ev).unwrap();
        }
        assert!(inline.pending_resume, "InteractiveEnd should set pending_resume");

        // Next event is Finished — should stay frozen (pending_resume remains true).
        let fin = ProgressEvent::Finished { success: true };
        state.apply(&fin);
        inline.handle(&state, &fin).unwrap();
        assert!(inline.pending_resume, "Finished after InteractiveEnd should not resume");
    }

    #[test]
    fn completed_recipe_keeps_artifact_strip_visible() {
        use crate::render::test_term::TestTerm;
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();

        for ev in [
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 2 }],
                total_nodes: 2,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "a.c".into(), artifact: Some("build/a.o".into()),
                fallback_label: "x".into(),
            },
            ProgressEvent::NodeCompleted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                elapsed: Duration::from_millis(100),
            },
            ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(1),
                name: "b.c".into(), artifact: Some("build/b.o".into()),
                fallback_label: "x".into(),
            },
            ProgressEvent::NodeCompleted {
                recipe: RecipeId::new(0), node: NodeId::new(1),
                elapsed: Duration::from_millis(100),
            },
            ProgressEvent::RecipeCompleted {
                recipe: RecipeId::new(0),
                elapsed: Duration::from_millis(200),
                cached: 0, total: 2,
            },
        ].iter() {
            state.apply(ev);
            inline.handle(&state, ev).unwrap();
        }
        inline.finish(&state).unwrap();

        let out = term.contents();
        assert!(out.contains("a.o"), "completed recipe should still show artifact pills; got:\n{out}");
        assert!(out.contains("b.o"), "completed recipe should still show artifact pills; got:\n{out}");
    }

    #[test]
    fn next_non_finished_event_clears_pending_resume() {
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();

        let setup = [
            ProgressEvent::BuildStarted {
                recipes: vec![
                    RecipeTopo { id: RecipeId::new(0), name: "a".into(), deps: vec![], expected_nodes: 1 },
                    RecipeTopo { id: RecipeId::new(1), name: "b".into(), deps: vec![], expected_nodes: 1 },
                ],
                total_nodes: 2,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "x".into(), artifact: None, fallback_label: "x".into(),
            },
            ProgressEvent::InteractiveStart { recipe: RecipeId::new(0), node: NodeId::new(0) },
            ProgressEvent::InteractiveEnd {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                elapsed: Duration::from_millis(1), success: true,
            },
        ];
        for ev in &setup {
            state.apply(ev);
            inline.handle(&state, ev).unwrap();
        }
        assert!(inline.pending_resume);

        let next = ProgressEvent::RecipeStarted { recipe: RecipeId::new(1) };
        state.apply(&next);
        inline.handle(&state, &next).unwrap();
        assert!(!inline.pending_resume, "non-Finished event should resume");
    }
}
