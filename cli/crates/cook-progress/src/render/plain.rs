//! Plain append-only renderer — non-TTY / CI-safe output.
//!
//! Applies the same noise rules as the inline event writer: cached node
//! rows are held per recipe and dropped entirely when the recipe finishes
//! with no real work (its `done (N/N cached)` summary row says it all),
//! toolchain probes collapse to one row per recipe, and internal recipes
//! (`__cc_*` double-underscore convention) show their module tag and skip
//! queued/summary rows.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::Duration;

use crate::event::{ProgressEvent, RecipeId, SkipReason, Stream};
use crate::model::build::BuildState;
use crate::naming::{display_recipe_name, is_internal_recipe, probe_module};
use crate::render::Renderer;

/// Per-recipe hold buffer: cached rows waiting on evidence of real work,
/// plus the recipe's grouped toolchain probes.
#[derive(Debug, Default)]
struct RecipeBuffer {
    cached_rows: Vec<String>,
    probes_ran: usize,
    probes_cached: usize,
    probes_elapsed: Duration,
    probe_module: Option<String>,
}

pub struct PlainRenderer<W: Write + Send> {
    out: W,
    buffers: BTreeMap<RecipeId, RecipeBuffer>,
}

impl<W: Write + Send> PlainRenderer<W> {
    pub fn new(out: W) -> Self { Self { out, buffers: BTreeMap::new() } }

    fn name(&self, state: &BuildState, recipe: RecipeId) -> String {
        self.raw_name(state, recipe)
            .map(|n| display_recipe_name(&n))
            .unwrap_or_else(|| format!("recipe#{}", recipe.raw()))
    }

    fn raw_name(&self, state: &BuildState, recipe: RecipeId) -> Option<String> {
        state.recipes.get(&recipe).map(|r| r.name.clone())
    }

    fn is_internal(&self, state: &BuildState, recipe: RecipeId) -> bool {
        self.raw_name(state, recipe).is_some_and(|n| is_internal_recipe(&n))
    }

    /// Flush a recipe's grouped probe row, then its held cached rows.
    fn flush_recipe(&mut self, state: &BuildState, recipe: RecipeId) -> io::Result<usize> {
        let ran = self.flush_probes(state, recipe)?;
        self.flush_cached(recipe)?;
        Ok(ran)
    }

    /// One row for the recipe's probes, only if any actually ran; a
    /// fully-cached probe set stays silent. Returns how many probes ran.
    fn flush_probes(&mut self, state: &BuildState, recipe: RecipeId) -> io::Result<usize> {
        let Some(buf) = self.buffers.get_mut(&recipe) else { return Ok(0) };
        let (ran, cached) = (buf.probes_ran, buf.probes_cached);
        let elapsed = buf.probes_elapsed;
        let module = buf.probe_module.take().unwrap_or_default();
        buf.probes_ran = 0;
        buf.probes_cached = 0;
        buf.probes_elapsed = Duration::ZERO;
        if ran == 0 { return Ok(0); }
        let rname = self.name(state, recipe);
        let noun = if ran + cached == 1 { "probe" } else { "probes" };
        let label = if cached > 0 {
            format!("probe:{module} ({} {noun}, {cached} cached)", ran + cached)
        } else {
            format!("probe:{module} ({ran} {noun})")
        };
        writeln!(self.out, "  {}/{:40}{}", rname, label, fmt_secs(elapsed))?;
        Ok(ran)
    }

    fn flush_cached(&mut self, recipe: RecipeId) -> io::Result<()> {
        let Some(buf) = self.buffers.get_mut(&recipe) else { return Ok(()) };
        let held = std::mem::take(&mut buf.cached_rows);
        for row in held {
            writeln!(self.out, "{row}")?;
        }
        Ok(())
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
                // Zero-node aggregators and internal tooling recipes add no
                // information to the queued list.
                for r in recipes {
                    if r.expected_nodes == 0 || is_internal_recipe(&r.name) { continue; }
                    writeln!(self.out, "  {:24} queued  ({} nodes)", r.name, r.expected_nodes)?;
                }
            }
            ProgressEvent::RecipeStarted { .. } => {}
            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total, .. } => {
                let probes_ran = self.flush_probes(state, *recipe)?;
                if *total == 0 || self.is_internal(state, *recipe) {
                    self.buffers.remove(recipe);
                    return Ok(());
                }
                // No real work beyond probes: drop the held cached rows —
                // the summary row alone tells the warm-build story.
                if cached + probes_ran >= *total {
                    self.buffers.remove(recipe);
                } else {
                    self.flush_cached(*recipe)?;
                }
                let name = self.name(state, *recipe);
                let detail = if *cached > 0 {
                    format!("({cached}/{total} cached)")
                } else {
                    format!("({total}/{total})")
                };
                writeln!(self.out, "  {:24} done     {:24} {}", name, detail, fmt_secs(*elapsed))?;
            }
            ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => {
                self.flush_recipe(state, *recipe)?;
                let name = self.name(state, *recipe);
                writeln!(self.out, "  {:24} FAILED   ({}/{} steps) {}", name, completed, total, fmt_secs(*elapsed))?;
            }
            ProgressEvent::RecipeSkipped { recipe, elapsed, completed, total, .. } => {
                self.flush_recipe(state, *recipe)?;
                let name = self.name(state, *recipe);
                writeln!(self.out, "  {:24} skipped  ({}/{} ran, upstream-failed) {}", name, completed, total, fmt_secs(*elapsed))?;
            }
            // COOK-276: a warm re-run announces its cause at start of work.
            ProgressEvent::NodeStarted { recipe, node, cause: Some(cause), .. } => {
                self.flush_recipe(state, *recipe)?;
                let rname = self.name(state, *recipe);
                let nname = self.node_display(state, recipe, node);
                writeln!(self.out, "  {}/{:40}rebuild ({cause})", rname, nname)?;
            }
            ProgressEvent::NodeStarted { .. } => {}
            ProgressEvent::NodeCompleted { recipe, node, elapsed, kind: _ } => {
                let nname = self.node_display(state, recipe, node);
                if let Some(module) = probe_module(&nname) {
                    let module = module.to_string();
                    let buf = self.buffers.entry(*recipe).or_default();
                    buf.probes_ran += 1;
                    buf.probes_elapsed += *elapsed;
                    buf.probe_module.get_or_insert(module);
                    return Ok(());
                }
                self.flush_recipe(state, *recipe)?;
                let rname = self.name(state, *recipe);
                writeln!(self.out, "  {}/{:40}{}", rname, nname, fmt_secs(*elapsed))?;
            }
            ProgressEvent::NodeFailed { recipe, node, elapsed, error } => {
                self.flush_recipe(state, *recipe)?;
                let rname = self.name(state, *recipe);
                let nname = self.node_display(state, recipe, node);
                writeln!(self.out, "  {}/{:40}FAILED {}", rname, nname, fmt_secs(*elapsed))?;
                for line in error.lines() {
                    writeln!(self.out, "  [{rname}/{nname}] {line}")?;
                }
            }
            ProgressEvent::NodeCacheHit { recipe, node, .. } => {
                let nname = self.node_display(state, recipe, node);
                if let Some(module) = probe_module(&nname) {
                    let module = module.to_string();
                    let buf = self.buffers.entry(*recipe).or_default();
                    buf.probes_cached += 1;
                    buf.probe_module.get_or_insert(module);
                    return Ok(());
                }
                // Held: prints only if the recipe turns out to do real work.
                let rname = self.name(state, *recipe);
                let row = format!("  {}/{:40}cached", rname, nname);
                self.buffers.entry(*recipe).or_default().cached_rows.push(row);
            }
            ProgressEvent::NodeSkipped { recipe, name: nname, reason, .. } => {
                self.flush_recipe(state, *recipe)?;
                let rname = self.name(state, *recipe);
                let reason_str = match reason {
                    SkipReason::UpstreamFailed => "upstream-failed",
                    SkipReason::ConditionFalse => "condition-false",
                    SkipReason::Disabled => "disabled",
                };
                writeln!(self.out, "  {}/{:40}skipped ({reason_str})", rname, nname)?;
            }
            ProgressEvent::NodeOutput { recipe, node, line, stream } => {
                self.flush_recipe(state, *recipe)?;
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
        // Flush anything still held (recipes cut short by a failure).
        let pending: Vec<RecipeId> = self.buffers.keys().copied().collect();
        for r in pending {
            self.flush_recipe(state, r)?;
        }
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
    use crate::event::{NodeId, NodeKind, RecipeTopo};

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
                cause: None,
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
        // A held cached row, once flushed by real work in the same recipe,
        // must show the unit's own full declared output path, not the raw
        // shell command that produced it and not just the output's basename —
        // the distinguishing directory segment must survive.
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "build", 2)]), total_nodes: 2,
        });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "wc -w < a.txt > build/counts/alpha.count".into(),
            artifact: Some(std::path::PathBuf::from("build/counts/alpha.count")),
            fallback_label: "wc -w < a.txt > build/counts/alpha.count".into(),
            kind: crate::event::NodeKind::Cooked,
                cause: None,
            });
        let hit = ProgressEvent::NodeCacheHit {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "wc -w < a.txt > build/counts/alpha.count".into(),
            artifact: Some(std::path::PathBuf::from("build/counts/alpha.count")),
            kind: NodeKind::Cooked,
        };
        state.apply(&hit);
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(1),
            name: "beta".into(),
            artifact: Some(std::path::PathBuf::from("build/counts/beta.count")),
            fallback_label: "wc -w < b.txt > build/counts/beta.count".into(),
            kind: crate::event::NodeKind::Cooked,
                cause: None,
            });
        let completed = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(1),
            elapsed: Duration::from_millis(50),
            kind: crate::event::NodeKind::Cooked,
        };
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            r.handle(&state, &hit).unwrap();
            state.apply(&completed);
            r.handle(&state, &completed).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("build/build/counts/alpha.count"), "got: {s}");
        assert!(s.contains("cached"), "got: {s}");
        assert!(!s.contains("wc -w"), "raw command leaked into label: {s}");
    }

    #[test]
    fn all_cached_recipe_drops_held_rows_keeps_summary() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", 2)]), total_nodes: 2,
        });
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            for i in 0..2u32 {
                state.apply(&ProgressEvent::NodeStarted {
                    recipe: RecipeId::new(0), node: NodeId::new(i),
                    name: format!("a{i}.c"),
                    artifact: Some(format!("a{i}.o").into()),
                    fallback_label: format!("cc a{i}.c"),
                    kind: crate::event::NodeKind::Compile,
                cause: None,
            });
                let hit = ProgressEvent::NodeCacheHit {
                    recipe: RecipeId::new(0), node: NodeId::new(i),
                    name: format!("a{i}.o"), artifact: Some(format!("a{i}.o").into()), kind: NodeKind::Cooked,
                };
                state.apply(&hit);
                r.handle(&state, &hit).unwrap();
            }
            let done = ProgressEvent::RecipeCompleted {
                recipe: RecipeId::new(0),
                elapsed: Duration::from_millis(5),
                cached: 2, total: 2,
                kind: crate::event::RecipeKind::Recipe,
            };
            state.apply(&done);
            r.handle(&state, &done).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.lines().count(), 1, "warm no-op recipe must be one row, got: {s}");
        assert!(s.contains("deps"), "got: {s}");
        assert!(s.contains("(2/2 cached)"), "got: {s}");
        assert!(!s.contains("a0.o"), "per-node cached rows must be dropped: {s}");
    }

    #[test]
    fn queued_list_skips_zero_node_and_internal_recipes() {
        let mut state = BuildState::new();
        let ev = ProgressEvent::BuildStarted {
            recipes: topo(&[
                (0, "idLib", 3),
                (1, "game", 0),
                (2, "__cc_config_header__x", 1),
            ]),
            total_nodes: 4,
        };
        state.apply(&ev);
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            r.handle(&state, &ev).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("idLib"), "got: {s}");
        assert!(!s.contains("game"), "zero-node aggregator must not queue: {s}");
        assert!(!s.contains("__cc"), "internal recipe must not queue: {s}");
    }

    #[test]
    fn internal_recipe_node_row_uses_module_tag_and_no_done_row() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "__cc_config_header__x", 1)]), total_nodes: 1,
        });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "config.h".into(),
            artifact: Some(std::path::PathBuf::from("build/config.h")),
            fallback_label: "render config.h".into(),
            kind: crate::event::NodeKind::Generate,
                cause: None,
            });
        let completed = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(10),
            kind: crate::event::NodeKind::Generate,
        };
        let done = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(15),
            cached: 0, total: 1,
            kind: crate::event::RecipeKind::Recipe,
        };
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            state.apply(&completed);
            r.handle(&state, &completed).unwrap();
            state.apply(&done);
            r.handle(&state, &done).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("cc/build/config.h"), "got: {s}");
        assert!(!s.contains("__cc"), "raw minted name must not leak: {s}");
        assert!(!s.contains("done"), "internal recipes have no summary row: {s}");
    }

    #[test]
    fn probes_collapse_to_one_row() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "idLib", 3)]), total_nodes: 3,
        });
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            for (i, key) in ["probe:cc:compiler:auto", "probe:cc:find:sdl2"].iter().enumerate() {
                state.apply(&ProgressEvent::NodeStarted {
                    recipe: RecipeId::new(0), node: NodeId::new(i as u32),
                    name: (*key).into(), artifact: None,
                    fallback_label: (*key).into(),
                    kind: crate::event::NodeKind::Resolve,
                cause: None,
            });
                let completed = ProgressEvent::NodeCompleted {
                    recipe: RecipeId::new(0), node: NodeId::new(i as u32),
                    elapsed: Duration::from_millis(10),
                    kind: crate::event::NodeKind::Resolve,
                };
                state.apply(&completed);
                r.handle(&state, &completed).unwrap();
            }
            state.apply(&ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(2),
                name: "x.c".into(), artifact: Some("x.o".into()),
                fallback_label: "cc x.c".into(),
                kind: crate::event::NodeKind::Compile,
                cause: None,
            });
            let completed = ProgressEvent::NodeCompleted {
                recipe: RecipeId::new(0), node: NodeId::new(2),
                elapsed: Duration::from_millis(100),
                kind: crate::event::NodeKind::Compile,
            };
            state.apply(&completed);
            r.handle(&state, &completed).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("idLib/probe:cc (2 probes)"), "got: {s}");
        assert!(!s.contains("probe:cc:compiler"), "raw probe keys must not leak: {s}");
        let probe_count = s.lines().filter(|l| l.contains("probe:")).count();
        assert_eq!(probe_count, 1, "got: {s}");
    }

    #[test]
    fn interactive_label_drops_internal_line_tag() {
        // `@N` is an internal source-line tag; never expose it in frames.
        assert_eq!(interactive_label("greet", "@23"), "greet");
        assert_eq!(interactive_label("greet", "shell"), "greet/shell");
    }
}
