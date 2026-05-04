//! Append-only event renderer.
//!
//! Turns each `(BuildState, ProgressEvent)` into 0 or 1 lines of stderr
//! output. Cargo-style: 12-col right-aligned past-tense verb, then subject,
//! then `in <duration>` (or `(detail)` for recipe summaries). No symbols,
//! no live frame, no library — just `writeln!`.
//!
//! Stateful only for two reasons:
//! - **Cached-line collapsing**: per-recipe counter to render `… (N more
//!   cached)` after a threshold of explicit `Cached` lines.
//! - **Cascaded skip collapsing**: a buffer of pending `Skipped(UpstreamFailed)`
//!   events flushed when a non-skip event arrives or `Finished` fires.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::Duration;

use crate::event::{NodeKind, ProgressEvent, RecipeId, SkipReason, Stream};
use crate::model::build::BuildState;
use crate::style::{format_verb, verb_for, LineKind};

/// Indent for stderr lines below a `Failed` verb line. 12-col verb + 1 sep + 2 indent = 15 spaces.
const STDERR_INDENT: &str = "               ";
/// Prefix for the "(N more cached)" summary line that follows a cached collapse.
/// 9 spaces + ellipsis + 2 spaces = aligned roughly under the verb's right margin.
const SUPPRESSION_PREFIX: &str = "         …  ";

#[derive(Debug, Clone, Copy)]
pub struct EventWriterOptions {
    /// Emit ANSI colour codes.
    pub colored: bool,
    /// `--quiet`: drop per-node verb lines; only recipe + build summaries.
    pub quiet: bool,
    /// `--verbose`: stream per-node stdout/stderr inline with `[recipe/node]` prefix.
    pub verbose: bool,
    /// Threshold beyond which Cached lines collapse to `… (N more cached)` per recipe.
    pub cached_inline_threshold: usize,
}

impl Default for EventWriterOptions {
    fn default() -> Self {
        Self { colored: true, quiet: false, verbose: false, cached_inline_threshold: 8 }
    }
}

/// Per-recipe counter used by the cached-collapse rule.
#[derive(Debug, Default, Clone, Copy)]
struct CachedCounter {
    printed: usize,
    suppressed: usize,
}

pub struct EventWriter {
    opts: EventWriterOptions,
    cached: BTreeMap<RecipeId, CachedCounter>,
    /// Pending UpstreamFailed skips per recipe, flushed as a collapsed line.
    pending_upstream_skips: Vec<(RecipeId, String)>,
}

impl EventWriter {
    pub fn new(opts: EventWriterOptions) -> Self {
        Self { opts, cached: BTreeMap::new(), pending_upstream_skips: Vec::new() }
    }

    /// Render an event to `out`. Returns whether anything was written.
    pub fn handle<W: Write>(
        &mut self,
        out: &mut W,
        state: &BuildState,
        event: &ProgressEvent,
    ) -> io::Result<bool> {
        // Flush any pending cascaded-skip buffer when the next event is not
        // an UpstreamFailed skip.
        if !matches!(event,
            ProgressEvent::NodeSkipped { reason: SkipReason::UpstreamFailed, .. })
            && !self.pending_upstream_skips.is_empty()
        {
            self.flush_skips(out)?;
        }

        match event {
            ProgressEvent::BuildStarted { .. } => Ok(false),
            ProgressEvent::RecipeStarted { .. } => Ok(false),

            ProgressEvent::NodeCacheHit { recipe, node, .. } => {
                if self.opts.quiet { return Ok(false); }
                let counter = self.cached.entry(*recipe).or_default();
                if counter.printed < self.opts.cached_inline_threshold {
                    counter.printed += 1;
                    let rname = recipe_name(state, *recipe);
                    let nname = node_display(state, *recipe, *node);
                    let v = verb_for(LineKind::NodeCached, NodeKind::Cooked);
                    writeln!(out, "{} {rname}/{nname}", format_verb(v, self.opts.colored))?;
                    Ok(true)
                } else {
                    counter.suppressed += 1;
                    Ok(false)
                }
            }

            ProgressEvent::NodeCompleted { recipe, node, elapsed, kind } => {
                if self.opts.quiet { return Ok(false); }
                let rname = recipe_name(state, *recipe);
                let nname = node_display(state, *recipe, *node);
                let v = verb_for(LineKind::NodeCompleted, *kind);
                writeln!(out, "{} {rname}/{nname} in {}",
                    format_verb(v, self.opts.colored), fmt_secs(*elapsed))?;
                Ok(true)
            }

            ProgressEvent::NodeFailed { recipe, node, elapsed, error } => {
                let rname = recipe_name(state, *recipe);
                let nname = node_display(state, *recipe, *node);
                let v = verb_for(LineKind::NodeFailed, NodeKind::Cooked);
                writeln!(out, "{} {rname}/{nname} in {}",
                    format_verb(v, self.opts.colored), fmt_secs(*elapsed))?;
                // Indent stderr to one space past the verb's right margin (15 spaces).
                for line in error.lines() {
                    writeln!(out, "{STDERR_INDENT}{line}")?;
                }
                Ok(true)
            }

            ProgressEvent::NodeSkipped { recipe, name, reason, .. } => match reason {
                SkipReason::UpstreamFailed => {
                    self.pending_upstream_skips.push((*recipe, name.clone()));
                    Ok(false)
                }
                _ => {
                    if self.opts.quiet { return Ok(false); }
                    let rname = recipe_name(state, *recipe);
                    let v = verb_for(LineKind::NodeSkipped, NodeKind::Cooked);
                    writeln!(out, "{} {rname}/{name} ({})",
                        format_verb(v, self.opts.colored), reason.as_str())?;
                    Ok(true)
                }
            },

            ProgressEvent::NodeOutput { recipe, node, line, stream } => {
                if !self.opts.verbose { return Ok(false); }
                let rname = recipe_name(state, *recipe);
                let nlabel = state.recipes.get(recipe)
                    .and_then(|r| r.nodes.get(node))
                    .map(|n| n.label().to_string())
                    .unwrap_or_else(|| format!("node#{}", node.raw()));
                let tag = match stream { Stream::Stderr => " (stderr)", _ => "" };
                writeln!(out, "[{rname}/{nlabel}]{tag} {line}")?;
                Ok(true)
            }

            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total } => {
                self.flush_cached_suppression(out, *recipe)?;
                let rname = recipe_name(state, *recipe);
                let v = verb_for(LineKind::RecipeFinished, NodeKind::Cooked);
                let detail = if *cached > 0 {
                    format!("({} nodes, {} cached)", total, cached)
                } else {
                    format!("({} nodes)", total)
                };
                writeln!(out, "{} {rname} in {}   {}",
                    format_verb(v, self.opts.colored), fmt_secs(*elapsed), detail)?;
                Ok(true)
            }

            ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => {
                self.flush_cached_suppression(out, *recipe)?;
                let rname = recipe_name(state, *recipe);
                let v = verb_for(LineKind::RecipeFailed, NodeKind::Cooked);
                writeln!(out, "{} {rname} in {}   ({}/{} nodes)",
                    format_verb(v, self.opts.colored), fmt_secs(*elapsed), completed, total)?;
                Ok(true)
            }

            ProgressEvent::InteractiveStart { recipe, name, .. } => {
                let rname = recipe_name(state, *recipe);
                let v = verb_for(LineKind::InteractiveRunning, NodeKind::Cooked);
                let label = if rname.is_empty() || rname == *name {
                    name.to_string()
                } else {
                    format!("{rname}/{name}")
                };
                writeln!(out, "{} {label}", format_verb(v, self.opts.colored))?;
                Ok(true)
            }

            ProgressEvent::InteractiveEnd { .. }
            | ProgressEvent::NodeStarted { .. } => Ok(false),

            ProgressEvent::Finished { success } => {
                self.flush_skips(out)?;
                let line_kind = if *success { LineKind::RecipeFinished } else { LineKind::RecipeFailed };
                let v = verb_for(line_kind, NodeKind::Cooked);
                let elapsed = state.elapsed();
                let totals = &state.totals;
                let detail = if *success {
                    format!("({} nodes, {} cached)", totals.total_nodes, totals.cached_node_count(state))
                } else {
                    format!("({} failed, {} skipped, {}/{} nodes)",
                        totals.failed_node_count(state),
                        totals.skipped_node_count(state),
                        totals.completed_nodes,
                        totals.total_nodes)
                };
                writeln!(out, "{} build in {}   {}",
                    format_verb(v, self.opts.colored), fmt_secs(elapsed), detail)?;
                Ok(true)
            }
        }
    }

    fn flush_cached_suppression<W: Write>(&mut self, out: &mut W, recipe: RecipeId) -> io::Result<()> {
        if self.opts.quiet { return Ok(()); }
        if let Some(c) = self.cached.get(&recipe) {
            if c.suppressed > 0 {
                writeln!(out, "{SUPPRESSION_PREFIX}({} more cached)", c.suppressed)?;
            }
        }
        Ok(())
    }

    fn flush_skips<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        if self.pending_upstream_skips.is_empty() { return Ok(()); }
        let mut by_recipe: BTreeMap<RecipeId, Vec<String>> = BTreeMap::new();
        for (r, n) in self.pending_upstream_skips.drain(..) {
            by_recipe.entry(r).or_default().push(n);
        }
        let total: usize = by_recipe.values().map(|v| v.len()).sum();
        let recipe_count = by_recipe.len();
        let v = verb_for(LineKind::NodeSkipped, NodeKind::Cooked);
        let label = if recipe_count == 1 {
            format!("{} ({} nodes, upstream failed)",
                by_recipe.values().next().unwrap().join(", "), total)
        } else {
            format!("{} recipes ({} nodes, upstream failed)", recipe_count, total)
        };
        writeln!(out, "{} {}", format_verb(v, self.opts.colored), label)?;
        Ok(())
    }
}

fn recipe_name(state: &BuildState, recipe: RecipeId) -> String {
    state.recipes.get(&recipe)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| format!("recipe#{}", recipe.raw()))
}

fn node_display(state: &BuildState, recipe: RecipeId, node: crate::event::NodeId) -> String {
    state.recipes.get(&recipe)
        .and_then(|r| r.nodes.get(&node))
        .map(|n| n.display())
        .unwrap_or_else(|| format!("node#{}", node.raw()))
}

fn fmt_secs(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.2}s")
    } else if secs < 3600.0 {
        let m = (secs as u64) / 60;
        let s = (secs as u64) % 60;
        format!("{m}m{s:02}s")
    } else {
        let h = (secs as u64) / 3600;
        let m = ((secs as u64) % 3600) / 60;
        let s = (secs as u64) % 60;
        format!("{h}h{m:02}m{s:02}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{NodeId, RecipeTopo};
    use std::time::Duration;

    fn empty_state() -> BuildState {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 1,
            }],
            total_nodes: 1,
        });
        s
    }

    fn render_one(state: &BuildState, ev: &ProgressEvent, opts: EventWriterOptions) -> String {
        let mut buf = Vec::new();
        let mut w = EventWriter::new(opts);
        w.handle(&mut buf, state, ev).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn node_completed_compile_kind_emits_compiled_verb() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "lvm.c".into(),
            artifact: Some("build/obj/liblua/lvm.o".into()),
            fallback_label: "clang -c lvm.c".into(),
            kind: NodeKind::Compile,
        });
        let ev = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(880),
            kind: NodeKind::Compile,
        };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert_eq!(out, "    Compiled lib/lvm.o in 0.88s\n");
    }

    #[test]
    fn cached_lines_collapse_after_threshold() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes: 12,
            }],
            total_nodes: 12,
        });

        let mut buf = Vec::new();
        let mut w = EventWriter::new(EventWriterOptions { colored: false, cached_inline_threshold: 3, ..Default::default() });

        for i in 0..6 {
            let ev = ProgressEvent::NodeCacheHit {
                recipe: RecipeId::new(0), node: NodeId::new(i),
                name: format!("a{i}.o"), artifact: None,
            };
            state.apply(&ev);
            w.handle(&mut buf, &state, &ev).unwrap();
        }
        let out = String::from_utf8(buf.clone()).unwrap();
        let cached_lines = out.lines().filter(|l| l.contains("Cached")).count();
        assert_eq!(cached_lines, 3, "got: {out}");

        let ev = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(400),
            cached: 6, total: 6,
        };
        state.apply(&ev);
        w.handle(&mut buf, &state, &ev).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("(3 more cached)"), "got: {out}");
        assert!(out.contains("Finished deps"), "got: {out}");
    }

    #[test]
    fn node_failed_dumps_indented_stderr() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "lvm.c".into(), artifact: None,
            fallback_label: "clang lvm.c".into(),
            kind: NodeKind::Compile,
        });
        let ev = ProgressEvent::NodeFailed {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(1820),
            error: "lvm.c:42:9: error: 'bar' was not declared\n    int foo = bar(x);".into(),
        };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        let expected = "      Failed lib/$clang in 1.82s\n               lvm.c:42:9: error: 'bar' was not declared\n                   int foo = bar(x);\n";
        assert_eq!(out, expected, "got: {out}");
    }

    #[test]
    fn quiet_suppresses_per_node_lines_but_keeps_recipe_summary() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        let opts = EventWriterOptions { colored: false, quiet: true, ..Default::default() };

        let started = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "x.c".into(), artifact: Some("x.o".into()),
            fallback_label: "x".into(), kind: NodeKind::Compile,
        };
        state.apply(&started);
        let completed = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(100), kind: NodeKind::Compile,
        };
        state.apply(&completed);

        let mut buf = Vec::new();
        let mut w = EventWriter::new(opts);
        w.handle(&mut buf, &state, &completed).unwrap();

        let recipe_done = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(200),
            cached: 0, total: 1,
        };
        state.apply(&recipe_done);
        w.handle(&mut buf, &state, &recipe_done).unwrap();

        let out = String::from_utf8(buf).unwrap();
        assert!(!out.contains("Compiled"), "quiet should suppress per-node verbs: {out}");
        assert!(out.contains("Finished lib"), "got: {out}");
    }

    #[test]
    fn verbose_emits_node_output_lines() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "lvm.c".into(), artifact: None, fallback_label: "x".into(),
            kind: NodeKind::Compile,
        });
        let ev = ProgressEvent::NodeOutput {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            line: "warning: unused".into(), stream: Stream::Stderr,
        };
        let opts = EventWriterOptions { colored: false, verbose: true, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert_eq!(out, "[lib/lvm.c] (stderr) warning: unused\n");
    }

    #[test]
    fn upstream_failed_skips_collapse_to_one_line() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: vec![
                RecipeTopo { id: RecipeId::new(1), name: "lua".into(), deps: vec![], expected_nodes: 2 },
                RecipeTopo { id: RecipeId::new(2), name: "luac".into(), deps: vec![], expected_nodes: 2 },
            ],
            total_nodes: 4,
        });

        let mut buf = Vec::new();
        let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

        for (rid, n) in [(1u32, "lua.o"), (1, "lua"), (2, "luac.o"), (2, "luac")] {
            let ev = ProgressEvent::NodeSkipped {
                recipe: RecipeId::new(rid), node: NodeId::new(0),
                name: n.into(), reason: SkipReason::UpstreamFailed,
            };
            state.apply(&ev);
            w.handle(&mut buf, &state, &ev).unwrap();
        }
        let fin = ProgressEvent::Finished { success: false };
        state.apply(&fin);
        w.handle(&mut buf, &state, &fin).unwrap();

        let out = String::from_utf8(buf).unwrap();
        let skipped_lines = out.lines().filter(|l| l.contains("Skipped")).count();
        assert_eq!(skipped_lines, 1, "expected 1 collapsed line, got: {out}");
        assert!(out.contains("upstream failed"), "got: {out}");
    }
}
