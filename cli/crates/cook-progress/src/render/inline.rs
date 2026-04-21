//! Inline renderer — indicatif MultiProgress driving the live frame.

use std::collections::BTreeMap;
use std::io;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::event::{ProgressEvent, RecipeId};
use crate::model::build::BuildState;
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
}

impl Renderer for InlineRenderer {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        if let ProgressEvent::BuildStarted { .. } = event {
            for id in &state.order {
                let r = &state.recipes[id];
                let deps: Vec<&str> = r
                    .deps
                    .iter()
                    .filter_map(|d| state.recipes.get(d).map(|x| x.name.as_str()))
                    .collect();
                let deps_str = deps.join(", ");
                let bar = self.create_waiting_bar(&r.name, &deps_str);
                self.recipe_bars.insert(*id, bar);
            }
            let footer = self.multi.add(ProgressBar::new(0));
            footer.set_style(ProgressStyle::with_template("{msg}").unwrap());
            footer.set_message("");
            self.footer = Some(footer);
        }
        Ok(())
    }

    fn finish(&mut self, _state: &BuildState) -> io::Result<()> {
        for bar in self.recipe_bars.values() {
            bar.finish();
        }
        if let Some(f) = &self.footer {
            f.finish_and_clear();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::RecipeTopo;
    use crate::render::test_term::TestTerm;

    #[test]
    fn build_started_creates_a_bar_per_recipe() {
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();
        let ev = ProgressEvent::BuildStarted {
            recipes: vec![
                RecipeTopo {
                    id: RecipeId::new(0),
                    name: "deps".into(),
                    deps: vec![],
                    expected_nodes: 3,
                },
                RecipeTopo {
                    id: RecipeId::new(1),
                    name: "lib".into(),
                    deps: vec![RecipeId::new(0)],
                    expected_nodes: 5,
                },
            ],
            total_nodes: 8,
        };
        state.apply(&ev);
        inline.handle(&state, &ev).unwrap();
        inline.finish(&state).unwrap();
        assert_eq!(inline.recipe_bars.len(), 2);
    }
}
