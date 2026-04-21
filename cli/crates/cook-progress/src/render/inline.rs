//! Inline renderer — indicatif MultiProgress driving the live frame.

use std::collections::BTreeMap;
use std::io;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::event::{ProgressEvent, RecipeId};
use crate::model::build::BuildState;
use crate::model::recipe::Status;
use crate::render::Renderer;

pub struct InlineRenderer {
    multi: MultiProgress,
    recipe_bars: BTreeMap<RecipeId, ProgressBar>,
    footer: Option<ProgressBar>,
}

impl InlineRenderer {
    pub fn new(draw_target: ProgressDrawTarget) -> Self {
        let multi = MultiProgress::with_draw_target(draw_target);
        Self {
            multi,
            recipe_bars: BTreeMap::new(),
            footer: None,
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
                bar.set_message(format!("{}/{} · {secs:.1}s{cached}", r.progress.1, r.progress.1));
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
                bar.set_message(format!("{}/{} · {secs:.1}s{deps}", r.progress.0, r.progress.1));
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
}
