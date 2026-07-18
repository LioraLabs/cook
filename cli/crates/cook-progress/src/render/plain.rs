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

    /// Clean, never-empty label for a node: the node's `display()` (its own
    /// full declared output path, or a cleaned command token — never raw
    /// `set -e`-prefixed multi-line command text) when the node is present
    /// in state; a placeholder on a lookup miss, so the label is never
    /// blank (the `report/` bug).
    fn node_display(&self, state: &BuildState, recipe: &RecipeId, node: &crate::event::NodeId) -> String {
        state.recipes.get(recipe)
            .and_then(|r| r.nodes.get(node))
            .map(|n| n.display())
            .unwrap_or_else(|| "?".to_string())
    }
}

/// Interactive-frame label: drops the internal `@N` source-line tag rather
/// than exposing it in user-facing output (mirrors event_writer.rs's inline
/// renderer, which already strips it).
fn interactive_label(rname: &str, name: &str) -> String {
    if name.starts_with('@') {
        rname.to_string()
    } else {
        format!("{rname}/{name}")
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
            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total, .. } => {
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
            ProgressEvent::RecipeSkipped { recipe, elapsed, completed, total, .. } => {
                let name = self.name(state, *recipe);
                writeln!(self.out, "  {:24} skipped  ({}/{} ran, upstream-failed) {}", name, completed, total, fmt_secs(*elapsed))?;
            }
            ProgressEvent::NodeStarted { .. } => {}
            ProgressEvent::NodeCompleted { recipe, node, elapsed, kind: _ } => {
                let rname = self.name(state, *recipe);
                let nname = self.node_display(state, recipe, node);
                writeln!(self.out, "  {}/{:40}{}", rname, nname, fmt_secs(*elapsed))?;
            }
            ProgressEvent::NodeFailed { recipe, node, elapsed, error } => {
                let rname = self.name(state, *recipe);
                let nname = self.node_display(state, recipe, node);
                writeln!(self.out, "  {}/{:40}FAILED {}", rname, nname, fmt_secs(*elapsed))?;
                for line in error.lines() {
                    writeln!(self.out, "  [{rname}/{nname}] {line}")?;
                }
            }
            ProgressEvent::NodeCacheHit { recipe, node, .. } => {
                let rname = self.name(state, *recipe);
                let nname = self.node_display(state, recipe, node);
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
                // Same label as the completion line (own full output path,
                // or a clean fallback) — not the raw node name/command text.
                let nname = self.node_display(state, recipe, node);
                let tag = match stream {
                    Stream::Stdout => "",
                    Stream::Stderr => "(stderr) ",
                };
                writeln!(self.out, "  [{rname}/{nname}] {tag}{line}")?;
            }
            ProgressEvent::InteractiveStart { recipe, name, .. } => {
                let rname = self.name(state, *recipe);
                let label = interactive_label(&rname, name);
                writeln!(self.out, "─── {label} ───")?;
            }
            ProgressEvent::InteractiveEnd { recipe, name, elapsed, success, .. } => {
                let rname = self.name(state, *recipe);
                let label = interactive_label(&rname, name);
                let ok = if *success { "ok" } else { "failed" };
                writeln!(self.out, "─── {label} resumed ({ok}, {}) ───", fmt_secs(*elapsed))?;
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
        // The summary line used to say "cook build done ..." regardless of
        // which recipe ran (`cook greet` would still print "cook build done");
        // confusing for non-build entrypoints. Drop the recipe label — the
        // per-row progress lines above already named what ran.
        if ok {
            writeln!(self.out, "cook done in {elapsed} ({total} nodes, {cached} cached recipes, {done} done)")?;
        } else {
            writeln!(self.out, "cook FAILED after {elapsed}")?;
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
            kind: crate::event::RecipeKind::Recipe,
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
    fn recipe_skipped_writes_skipped_not_done_line() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "report", 1)]), total_nodes: 1,
        });
        state.apply(&ProgressEvent::RecipeStarted {
            recipe: RecipeId::new(0),
        });
        let ev = ProgressEvent::RecipeSkipped {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(400),
            skipped: 1,
            completed: 0,
            total: 1,
        };
        state.apply(&ev);
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            r.handle(&state, &ev).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("report"), "got: {s}");
        assert!(s.contains("skipped"), "got: {s}");
        assert!(s.contains("0/1 ran"), "got: {s}");
        assert!(!s.contains("done"), "got: {s}");
    }

    #[test]
    fn node_output_prefix_includes_recipe_and_node() {
        // The live-stdout tag must use the node's own full output path
        // (its `display()` label) — not its raw node name/command text.
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "lib", 1)]), total_nodes: 1,
        });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "lvm.c".into(),
            artifact: Some(std::path::PathBuf::from("build/obj/lvm.o")),
            fallback_label: "x".into(),
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
        assert!(s.contains("[lib/build/obj/lvm.o]"), "got: {s}");
        assert!(s.contains("(stderr)"), "got: {s}");
        assert!(s.contains("warning: unused"), "got: {s}");
    }

    #[test]
    fn cache_hit_line_uses_full_output_path_not_raw_command() {
        // The dominant second-run all-cached case must show the unit's own
        // full declared output path, not the raw shell command that
        // produced it (item 1) and not just the output's basename — the
        // distinguishing directory segment must survive.
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "build", 1)]), total_nodes: 1,
        });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "wc -w < a.txt > build/counts/alpha.count".into(),
            artifact: Some(std::path::PathBuf::from("build/counts/alpha.count")),
            fallback_label: "wc -w < a.txt > build/counts/alpha.count".into(),
            kind: crate::event::NodeKind::Cooked,
        });
        let ev = ProgressEvent::NodeCacheHit {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "wc -w < a.txt > build/counts/alpha.count".into(),
            artifact: Some(std::path::PathBuf::from("build/counts/alpha.count")),
        };
        state.apply(&ev);
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            r.handle(&state, &ev).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("build/build/counts/alpha.count"), "got: {s}");
        assert!(s.contains("cached"), "got: {s}");
        assert!(!s.contains("wc -w"), "raw command leaked into label: {s}");
    }

    #[test]
    fn interactive_label_drops_internal_line_tag() {
        // `@N` is an internal source-line tag; never expose it in frames.
        assert_eq!(interactive_label("greet", "@23"), "greet");
        assert_eq!(interactive_label("greet", "shell"), "greet/shell");
    }
}
