//! Plain append-only renderer — non-TTY / CI-safe output.

use std::io::{self, Write};
use std::time::Duration;

use crate::event::{ProgressEvent, RecipeId, SkipReason, Stream};
use crate::model::build::BuildState;
use crate::render::Renderer;

pub struct PlainRenderer<W: Write + Send> {
    out: W,
}

impl<W: Write + Send> PlainRenderer<W> {
    pub fn new(out: W) -> Self { Self { out } }

    fn name(&self, state: &BuildState, recipe: RecipeId) -> String {
        state.recipes.get(&recipe).map(|r| r.name.clone()).unwrap_or_else(|| format!("recipe#{}", recipe.raw()))
    }
}

fn fmt_secs(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.2}s")
    } else {
        let m = (secs as u64) / 60;
        let s = (secs as u64) % 60;
        format!("{m}m{s}s")
    }
}

impl<W: Write + Send> Renderer for PlainRenderer<W> {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        match event {
            ProgressEvent::BuildStarted { recipes, .. } => {
                for r in recipes {
                    writeln!(self.out, "  {:24} queued  ({} nodes)", r.name, r.expected_nodes)?;
                }
            }
            ProgressEvent::RecipeStarted { .. } => {}
            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total } => {
                let name = self.name(state, *recipe);
                let detail = if *cached > 0 {
                    format!("({cached}/{total} cached)")
                } else {
                    format!("({total}/{total})")
                };
                writeln!(self.out, "  {:24} done     {:24} {}", name, detail, fmt_secs(*elapsed))?;
            }
            ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => {
                let name = self.name(state, *recipe);
                writeln!(self.out, "  {:24} FAILED   ({}/{} steps) {}", name, completed, total, fmt_secs(*elapsed))?;
            }
            ProgressEvent::NodeStarted { .. } => {}
            ProgressEvent::NodeCompleted { recipe, node, elapsed, kind: _ } => {
                let rname = self.name(state, *recipe);
                let nname = state.recipes.get(recipe)
                    .and_then(|r| r.nodes.get(node))
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                writeln!(self.out, "  {}/{:40}{}", rname, nname, fmt_secs(*elapsed))?;
            }
            ProgressEvent::NodeFailed { recipe, node, elapsed, error } => {
                let rname = self.name(state, *recipe);
                let nname = state.recipes.get(recipe)
                    .and_then(|r| r.nodes.get(node))
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                writeln!(self.out, "  {}/{:40}FAILED {}", rname, nname, fmt_secs(*elapsed))?;
                for line in error.lines() {
                    writeln!(self.out, "  [{rname}/{nname}] {line}")?;
                }
            }
            ProgressEvent::NodeCacheHit { recipe, name: nname, .. } => {
                let rname = self.name(state, *recipe);
                writeln!(self.out, "  {}/{:40}cached", rname, nname)?;
            }
            ProgressEvent::NodeSkipped { recipe, name: nname, reason, .. } => {
                let rname = self.name(state, *recipe);
                let reason_str = match reason {
                    SkipReason::UpstreamFailed => "upstream-failed",
                    SkipReason::ConditionFalse => "condition-false",
                    SkipReason::Disabled => "disabled",
                };
                writeln!(self.out, "  {}/{:40}skipped ({reason_str})", rname, nname)?;
            }
            ProgressEvent::NodeOutput { recipe, node, line, stream } => {
                let rname = self.name(state, *recipe);
                let nname = state.recipes.get(recipe)
                    .and_then(|r| r.nodes.get(node))
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                let tag = match stream {
                    Stream::Stdout => "",
                    Stream::Stderr => "(stderr) ",
                };
                writeln!(self.out, "  [{rname}/{nname}] {tag}{line}")?;
            }
            ProgressEvent::InteractiveStart { recipe, name, .. } => {
                let rname = self.name(state, *recipe);
                writeln!(self.out, "─── {rname}/{name} ───")?;
            }
            ProgressEvent::InteractiveEnd { recipe, name, elapsed, success, .. } => {
                let rname = self.name(state, *recipe);
                let ok = if *success { "ok" } else { "failed" };
                writeln!(self.out, "─── {rname}/{name} resumed ({ok}, {}) ───", fmt_secs(*elapsed))?;
            }
            ProgressEvent::Finished { .. } => {}
        }
        Ok(())
    }

    fn finish(&mut self, state: &BuildState) -> io::Result<()> {
        let ok = state.finished.unwrap_or(false);
        let done = state.totals.done;
        let cached = state.totals.cached;
        let total = state.totals.total_nodes;
        let elapsed = fmt_secs(state.elapsed());
        if ok {
            writeln!(self.out, "cook build done in {elapsed} ({total} nodes, {cached} cached recipes, {done} done)")?;
        } else {
            writeln!(self.out, "cook build FAILED after {elapsed}")?;
        }
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{NodeId, RecipeTopo};

    fn topo(recipes: &[(u32, &str, usize)]) -> Vec<RecipeTopo> {
        recipes.iter().map(|(id, name, n)| RecipeTopo {
            id: RecipeId::new(*id),
            name: (*name).to_string(),
            deps: vec![],
            expected_nodes: *n,
        }).collect()
    }

    #[test]
    fn build_started_writes_queued_lines() {
        let mut state = BuildState::new();
        let ev = ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", 2), (1, "lib", 3)]),
            total_nodes: 5,
        };
        state.apply(&ev);
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            r.handle(&state, &ev).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("deps"));
        assert!(s.contains("queued  (2 nodes)"));
        assert!(s.contains("lib"));
    }

    #[test]
    fn recipe_completed_writes_done_line() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", 2)]), total_nodes: 2,
        });
        let ev = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(400),
            cached: 0, total: 2,
        };
        state.apply(&ev);
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            r.handle(&state, &ev).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("deps"), "got: {s}");
        assert!(s.contains("done"), "got: {s}");
        assert!(s.contains("0.40s"), "got: {s}");
    }

    #[test]
    fn node_output_prefix_includes_recipe_and_node() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "lib", 1)]), total_nodes: 1,
        });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "lvm.c".into(), artifact: None, fallback_label: "x".into(),
            kind: crate::event::NodeKind::Cooked,
        });
        let ev = ProgressEvent::NodeOutput {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            line: "warning: unused".into(), stream: Stream::Stderr,
        };
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            r.handle(&state, &ev).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("[lib/lvm.c]"), "got: {s}");
        assert!(s.contains("(stderr)"), "got: {s}");
        assert!(s.contains("warning: unused"), "got: {s}");
    }
}
