//! Append-only event renderer.
//!
//! Turns each `(BuildState, ProgressEvent)` into 0 or 1 lines of stderr
//! output. Cargo-style: 12-col right-aligned past-tense verb, then subject,
//! then `in <duration>` (or `(detail)` for recipe summaries). No symbols,
//! no live frame, no library — just `writeln!`.
//!
//! Stateful for three reasons:
//! - **Cached-line holding**: `Cached` lines are held per recipe and only
//!   flushed (up to a threshold, then `… (N more cached)`) when the recipe
//!   does real work. A recipe that finishes with no real work collapses to
//!   a single dim `Cached <recipe> (N nodes)` line — the dominant warm-build
//!   case prints one line per recipe instead of one per node.
//! - **Probe grouping**: `probe:<module>:<key>` nodes collapse into a single
//!   `Resolved <module> toolchain` line per recipe; fully-cached probe sets
//!   stay silent.
//! - **Cascaded skip collapsing**: a buffer of pending `Skipped(UpstreamFailed)`
//!   events flushed when a non-skip event arrives or `Finished` fires.
//!
//! Internal recipes (double-underscore convention, e.g. `__cc_*`) display
//! their module tag and never print recipe summaries — their node lines
//! (with the friendly tag) are the whole story.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::Duration;

use crate::event::{NodeKind, ProgressEvent, RecipeId, SkipReason, Stream};
use crate::model::build::BuildState;
use crate::naming::{display_recipe_name, is_internal_recipe, probe_module};
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

/// Per-recipe hold buffer: cached lines waiting on evidence of real work,
/// plus the recipe's grouped toolchain probes.
#[derive(Debug, Default)]
struct RecipeBuffer {
    /// Held `Cached` labels (already `rname/nname`-formatted), printed only
    /// if the recipe turns out to do real work.
    cached: Vec<String>,
    /// Explicit `Cached` lines printed so far — the threshold is per recipe,
    /// not per flush burst.
    cached_printed: usize,
    /// Cached lines collapsed so far; reported once, at the recipe's final
    /// flush, as `… (N more cached)`.
    cached_suppressed: usize,
    probes_ran: usize,
    probes_cached: usize,
    probes_elapsed: Duration,
    probe_module: Option<String>,
}

pub struct EventWriter {
    opts: EventWriterOptions,
    buffers: BTreeMap<RecipeId, RecipeBuffer>,
    /// Pending UpstreamFailed skips per recipe, flushed as a collapsed line.
    pending_upstream_skips: Vec<(RecipeId, String)>,
    /// Set when a terminal `InteractiveEnd` fires (chore-style handoff that's
    /// the last work in the DAG). Suppresses all subsequent event lines so the
    /// chore's own output remains the user's last view — same shape as
    /// `cargo run`.
    after_terminal_chore: bool,
}

impl EventWriter {
    pub fn new(opts: EventWriterOptions) -> Self {
        Self {
            opts,
            buffers: BTreeMap::new(),
            pending_upstream_skips: Vec::new(),
            after_terminal_chore: false,
        }
    }

    /// Render an event to `out`. Returns whether anything was written.
    pub fn handle<W: Write>(
        &mut self,
        out: &mut W,
        state: &BuildState,
        event: &ProgressEvent,
    ) -> io::Result<bool> {
        // After a terminal chore handoff, cook is silent — the chore body
        // had terminal control and any subsequent NodeCompleted /
        // RecipeCompleted / Finished events would just be cleanup noise the
        // user shouldn't see.
        if self.after_terminal_chore {
            return Ok(false);
        }

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
                let nname = node_display(state, *recipe, *node);
                if !self.opts.verbose && let Some(module) = probe_module(&nname) {
                    let buf = self.buffers.entry(*recipe).or_default();
                    buf.probes_cached += 1;
                    buf.probe_module.get_or_insert_with(|| module.to_string());
                    return Ok(false);
                }
                let n = state.recipes.get(recipe).and_then(|r| r.nodes.get(node));
                let has_artifact = n.is_some_and(|n| n.artifact.is_some());
                if !has_artifact && !self.opts.verbose { return Ok(false); }

                let rname = recipe_name(state, *recipe);
                if self.opts.verbose {
                    // Verbose is the escape hatch: cached lines print live,
                    // unheld and uncollapsed.
                    let v = verb_for(LineKind::NodeCached, NodeKind::Cooked);
                    writeln!(out, "{} {rname}/{nname}", format_verb(v, self.opts.colored))?;
                    return Ok(true);
                }
                // Hold the line: it only prints if this recipe turns out to
                // do real work. An all-cached recipe collapses to one line.
                self.buffers.entry(*recipe).or_default()
                    .cached.push(format!("{rname}/{nname}"));
                Ok(false)
            }

            ProgressEvent::NodeCompleted { recipe, node, elapsed, kind } => {
                if self.opts.quiet { return Ok(false); }
                let nname = node_display(state, *recipe, *node);
                if !self.opts.verbose && let Some(module) = probe_module(&nname) {
                    let buf = self.buffers.entry(*recipe).or_default();
                    buf.probes_ran += 1;
                    buf.probes_elapsed += *elapsed;
                    buf.probe_module.get_or_insert_with(|| module.to_string());
                    return Ok(false);
                }
                let n = state.recipes.get(recipe).and_then(|r| r.nodes.get(node));
                let has_artifact = n.is_some_and(|n| n.artifact.is_some());
                if !has_artifact && !self.opts.verbose { return Ok(false); }
                self.flush_recipe(out, state, *recipe, false)?;
                let rname = recipe_name(state, *recipe);
                let v = verb_for(LineKind::NodeCompleted, *kind);
                writeln!(out, "{} {rname}/{nname} in {}",
                    format_verb(v, self.opts.colored), fmt_secs(*elapsed))?;
                Ok(true)
            }

            ProgressEvent::NodeFailed { recipe, node, elapsed, error } => {
                self.flush_recipe(out, state, *recipe, false)?;
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
                    self.flush_recipe(out, state, *recipe, false)?;
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
                // Same label as the completion line (own full output path,
                // or a clean fallback) — not the raw node name/command text.
                let nlabel = node_display(state, *recipe, *node);
                let tag = match stream { Stream::Stderr => " (stderr)", _ => "" };
                writeln!(out, "[{rname}/{nlabel}]{tag} {line}")?;
                Ok(true)
            }

            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total, kind } => {
                let probes_ran = self.flush_probes(out, state, *recipe)?;
                if *total == 0 { return Ok(false); }
                let internal = raw_recipe_name(state, *recipe)
                    .is_some_and(|n| is_internal_recipe(&n));
                // No real work: everything was cached except (at most) the
                // toolchain probes, which just printed their own group line.
                // The dominant warm-build case — one dim line per recipe.
                if cached + probes_ran >= *total {
                    self.buffers.remove(recipe);
                    if internal { return Ok(probes_ran > 0); }
                    let rname = recipe_name(state, *recipe);
                    let v = verb_for(LineKind::NodeCached, NodeKind::Cooked);
                    writeln!(out, "{} {rname} ({} nodes)",
                        format_verb(v, self.opts.colored), total)?;
                    return Ok(true);
                }
                self.flush_cached(out, *recipe, true)?;
                if internal {
                    // Internal tooling recipe: its node lines (tagged with the
                    // module name) are the whole story; no summary row.
                    return Ok(false);
                }
                let rname = recipe_name(state, *recipe);
                let v = verb_for(LineKind::RecipeFinished, NodeKind::Cooked);
                let detail = match kind {
                    crate::event::RecipeKind::Chore => "(chore)".to_string(),
                    crate::event::RecipeKind::Recipe => {
                        if *cached > 0 {
                            format!("({} nodes, {} cached)", total, cached)
                        } else {
                            format!("({} nodes)", total)
                        }
                    }
                };
                writeln!(out, "{} {rname} in {}   {}",
                    format_verb(v, self.opts.colored), fmt_secs(*elapsed), detail)?;
                Ok(true)
            }

            ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => {
                self.flush_recipe(out, state, *recipe, true)?;
                let rname = recipe_name(state, *recipe);
                let v = verb_for(LineKind::RecipeFailed, NodeKind::Cooked);
                writeln!(out, "{} {rname} in {}   ({}/{} nodes)",
                    format_verb(v, self.opts.colored), fmt_secs(*elapsed), completed, total)?;
                Ok(true)
            }

            ProgressEvent::RecipeSkipped { recipe, elapsed, completed, total, .. } => {
                self.flush_recipe(out, state, *recipe, true)?;
                let rname = recipe_name(state, *recipe);
                let v = verb_for(LineKind::NodeSkipped, NodeKind::Cooked);
                writeln!(out, "{} {rname} in {}   ({}/{} ran, upstream-failed)",
                    format_verb(v, self.opts.colored), fmt_secs(*elapsed), completed, total)?;
                Ok(true)
            }

            ProgressEvent::InteractiveStart { recipe, name, chore_step_count, .. } => {
                let rname = recipe_name(state, *recipe);
                let v = verb_for(LineKind::InteractiveRunning, NodeKind::Cooked);
                // For chore windows, the subject is always the chore name —
                // the head step's display_name (`@<line>` or `lua`) is an
                // implementation detail.
                let is_chore_window = *chore_step_count > 0;
                let label = if rname.is_empty() {
                    name.to_string()
                } else if is_chore_window || rname == *name || name.starts_with('@') {
                    rname
                } else {
                    format!("{rname}/{name}")
                };
                writeln!(out, "{} {label}", format_verb(v, self.opts.colored))?;
                Ok(true)
            }

            ProgressEvent::InteractiveEnd { is_terminal, .. } => {
                if *is_terminal {
                    self.after_terminal_chore = true;
                }
                Ok(false)
            }

            // COOK-276: a warm re-run announces its cause at start of work —
            // the moment the user is staring at an unexplained rebuild.
            ProgressEvent::NodeStarted { recipe, node, cause: Some(cause), .. } => {
                if self.opts.quiet { return Ok(false); }
                self.flush_recipe(out, state, *recipe, false)?;
                let rname = recipe_name(state, *recipe);
                let nname = node_display(state, *recipe, *node);
                let v = verb_for(LineKind::NodeRebuilding, NodeKind::Cooked);
                writeln!(out, "{} {rname}/{nname} — {cause}",
                    format_verb(v, self.opts.colored))?;
                Ok(true)
            }
            ProgressEvent::NodeStarted { .. } => Ok(false),

            ProgressEvent::Finished { success } => {
                // Flush anything still held (recipes cut short by a failure).
                let pending: Vec<RecipeId> = self.buffers.keys().copied().collect();
                for r in pending {
                    self.flush_recipe(out, state, r, true)?;
                }
                self.flush_skips(out)?;
                let line_kind = if *success { LineKind::RecipeFinished } else { LineKind::RecipeFailed };
                let v = verb_for(line_kind, NodeKind::Cooked);
                let elapsed = state.elapsed();
                let totals = &state.totals;
                let total = totals.completed_nodes.max(totals.total_nodes);
                let detail = if *success {
                    let cached = totals.cached_node_count(state);
                    if total > 0 && cached == total {
                        format!("({} nodes, all cached)", total)
                    } else {
                        format!("({} nodes, {} cached)", total, cached)
                    }
                } else {
                    format!("({} failed, {} skipped, {}/{} nodes)",
                        totals.failed_node_count(state),
                        totals.skipped_node_count(state),
                        totals.completed_nodes,
                        total)
                };
                writeln!(out, "{} in {}   {}",
                    format_verb(v, self.opts.colored), fmt_secs(elapsed), detail)?;
                Ok(true)
            }
        }
    }

    /// Flush a recipe's grouped probe line, then its held cached lines.
    /// Called on evidence of real work (or a failure) so held output lands
    /// in front of the line that triggered it. `terminal` marks the recipe's
    /// last flush, which also reports the collapsed-line count.
    fn flush_recipe<W: Write>(&mut self, out: &mut W, state: &BuildState, recipe: RecipeId, terminal: bool) -> io::Result<()> {
        self.flush_probes(out, state, recipe)?;
        self.flush_cached(out, recipe, terminal)
    }

    /// Print the grouped `Resolved <module> toolchain` line if any of the
    /// recipe's probes actually ran; a fully-cached probe set stays silent.
    /// Returns how many probes ran (consumed either way).
    fn flush_probes<W: Write>(&mut self, out: &mut W, state: &BuildState, recipe: RecipeId) -> io::Result<usize> {
        let Some(buf) = self.buffers.get_mut(&recipe) else { return Ok(0) };
        let (ran, cached) = (buf.probes_ran, buf.probes_cached);
        let elapsed = buf.probes_elapsed;
        let module = buf.probe_module.take().unwrap_or_default();
        buf.probes_ran = 0;
        buf.probes_cached = 0;
        buf.probes_elapsed = Duration::ZERO;
        if ran == 0 { return Ok(0); }
        let rname = recipe_name(state, recipe);
        let v = verb_for(LineKind::NodeCompleted, NodeKind::Resolve);
        let subject = if module.is_empty() {
            format!("toolchain for {rname}")
        } else {
            format!("{module} toolchain for {rname}")
        };
        let noun = if ran + cached == 1 { "probe" } else { "probes" };
        let detail = if cached > 0 {
            format!("({} {noun}, {cached} cached)", ran + cached)
        } else {
            format!("({ran} {noun})")
        };
        writeln!(out, "{} {subject} {detail} in {}",
            format_verb(v, self.opts.colored), fmt_secs(elapsed))?;
        Ok(ran)
    }

    /// Print a recipe's held cached lines, up to the per-recipe threshold;
    /// overflow accumulates and is reported once, on the `terminal` flush,
    /// as `… (N more cached)`.
    fn flush_cached<W: Write>(&mut self, out: &mut W, recipe: RecipeId, terminal: bool) -> io::Result<()> {
        let Some(buf) = self.buffers.get_mut(&recipe) else { return Ok(()) };
        let held = std::mem::take(&mut buf.cached);
        let allowance = self.opts.cached_inline_threshold.saturating_sub(buf.cached_printed);
        buf.cached_printed += held.len().min(allowance);
        buf.cached_suppressed += held.len().saturating_sub(allowance);
        let suppressed = if terminal { std::mem::take(&mut buf.cached_suppressed) } else { 0 };
        let v = verb_for(LineKind::NodeCached, NodeKind::Cooked);
        for label in held.iter().take(allowance) {
            writeln!(out, "{} {label}", format_verb(v, self.opts.colored))?;
        }
        if suppressed > 0 {
            writeln!(out, "{SUPPRESSION_PREFIX}({suppressed} more cached)")?;
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

/// Display name for a recipe: internal recipes (`__cc_*`) show their module
/// tag, user recipes show as declared.
fn recipe_name(state: &BuildState, recipe: RecipeId) -> String {
    raw_recipe_name(state, recipe)
        .map(|n| display_recipe_name(&n))
        .unwrap_or_else(|| format!("recipe#{}", recipe.raw()))
}

fn raw_recipe_name(state: &BuildState, recipe: RecipeId) -> Option<String> {
    state.recipes.get(&recipe).map(|r| r.name.clone())
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
                cause: None,
            });
        let ev = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(880),
            kind: NodeKind::Compile,
        };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        // Full declared output path, not the artifact basename.
        assert_eq!(out, "    Compiled lib/build/obj/liblua/lvm.o in 0.88s\n");
    }

    /// Seed `state` + `w` with `n` cache hits (artifact-bearing) on recipe 0.
    /// Mirrors the engine: a cached node gets no `NodeStarted`, the hit event
    /// itself registers the node (and its artifact) in state.
    fn apply_cache_hits(state: &mut BuildState, w: &mut EventWriter, buf: &mut Vec<u8>, n: u32) {
        for i in 0..n {
            let ev = ProgressEvent::NodeCacheHit {
                recipe: RecipeId::new(0), node: NodeId::new(i),
                name: format!("a{i}.o"), artifact: Some(format!("a{i}.o").into()), kind: NodeKind::Cooked,
            };
            state.apply(&ev);
            w.handle(buf, state, &ev).unwrap();
        }
    }

    fn deps_state(expected_nodes: usize) -> BuildState {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes,
            }],
            total_nodes: expected_nodes,
        });
        state
    }

    #[test]
    fn cached_lines_held_until_real_work_then_collapse_after_threshold() {
        let mut state = deps_state(12);
        let mut buf = Vec::new();
        let mut w = EventWriter::new(EventWriterOptions { colored: false, cached_inline_threshold: 3, ..Default::default() });

        apply_cache_hits(&mut state, &mut w, &mut buf, 6);
        // Nothing prints while the recipe might still be a no-op.
        assert!(buf.is_empty(), "cached lines must be held, got: {}", String::from_utf8_lossy(&buf));

        // Real work arrives — the held lines flush in front of it.
        let started = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(6),
            name: "b.c".into(), artifact: Some("b.o".into()),
            fallback_label: "cc b.c".into(), kind: NodeKind::Compile,
                cause: None,
            };
        state.apply(&started);
        let ev = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(6),
            elapsed: Duration::from_millis(120), kind: NodeKind::Compile,
        };
        state.apply(&ev);
        w.handle(&mut buf, &state, &ev).unwrap();

        let out = String::from_utf8(buf.clone()).unwrap();
        let cached_lines = out.lines().filter(|l| l.contains("Cached")).count();
        assert_eq!(cached_lines, 3, "got: {out}");
        assert!(out.contains("Compiled deps/b.o"), "got: {out}");
        let compiled_pos = out.find("Compiled").unwrap();
        let cached_pos = out.find("Cached").unwrap();
        assert!(cached_pos < compiled_pos, "held lines must flush before the trigger: {out}");
        // The collapse count is deferred to the recipe's final flush — one
        // report per recipe, not one per flush burst.
        assert!(!out.contains("more cached"), "got: {out}");

        let done = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(200),
            cached: 6, total: 7,
            kind: crate::event::RecipeKind::Recipe,
        };
        state.apply(&done);
        w.handle(&mut buf, &state, &done).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("(3 more cached)"), "got: {out}");
        assert!(out.contains("Finished deps"), "got: {out}");
    }

    #[test]
    fn all_cached_recipe_collapses_to_single_line() {
        let mut state = deps_state(6);
        let mut buf = Vec::new();
        let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

        apply_cache_hits(&mut state, &mut w, &mut buf, 6);
        let ev = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(400),
            cached: 6, total: 6,
            kind: crate::event::RecipeKind::Recipe,
        };
        state.apply(&ev);
        w.handle(&mut buf, &state, &ev).unwrap();

        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.lines().count(), 1, "warm no-op recipe must be one line, got: {out}");
        assert!(out.contains("Cached deps (6 nodes)"), "got: {out}");
        assert!(!out.contains("a0.o"), "per-node cached lines must be dropped: {out}");
        assert!(!out.contains("Finished"), "got: {out}");
    }

    #[test]
    fn probes_group_into_single_resolved_line() {
        let mut state = deps_state(3);
        let mut buf = Vec::new();
        let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

        for (i, key) in ["probe:cc:compiler:auto", "probe:cc:find:sdl2"].iter().enumerate() {
            let started = ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(i as u32),
                name: (*key).into(), artifact: None,
                fallback_label: (*key).into(), kind: NodeKind::Resolve,
                cause: None,
            };
            state.apply(&started);
            let ev = ProgressEvent::NodeCompleted {
                recipe: RecipeId::new(0), node: NodeId::new(i as u32),
                elapsed: Duration::from_millis(10), kind: NodeKind::Resolve,
            };
            state.apply(&ev);
            w.handle(&mut buf, &state, &ev).unwrap();
        }
        assert!(buf.is_empty(), "probes must group, got: {}", String::from_utf8_lossy(&buf));

        // A real node flushes the group in front of itself.
        let started = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(2),
            name: "x.c".into(), artifact: Some("x.o".into()),
            fallback_label: "cc x.c".into(), kind: NodeKind::Compile,
                cause: None,
            };
        state.apply(&started);
        let ev = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(2),
            elapsed: Duration::from_millis(100), kind: NodeKind::Compile,
        };
        state.apply(&ev);
        w.handle(&mut buf, &state, &ev).unwrap();

        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Resolved cc toolchain for deps (2 probes) in 0.02s"), "got: {out}");
        assert!(!out.contains("probe:cc:compiler"), "raw probe keys must not leak: {out}");
    }

    #[test]
    fn fully_cached_probe_set_stays_silent() {
        let mut state = deps_state(2);
        let mut buf = Vec::new();
        let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

        let started = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "probe:cc:compiler:auto".into(), artifact: None,
            fallback_label: "probe:cc:compiler:auto".into(), kind: NodeKind::Resolve,
                cause: None,
            };
        state.apply(&started);
        let hit = ProgressEvent::NodeCacheHit {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "probe:cc:compiler:auto".into(), artifact: None, kind: NodeKind::Cooked,
        };
        state.apply(&hit);
        w.handle(&mut buf, &state, &hit).unwrap();

        let done = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(5),
            cached: 2, total: 2,
            kind: crate::event::RecipeKind::Recipe,
        };
        state.apply(&done);
        w.handle(&mut buf, &state, &done).unwrap();

        let out = String::from_utf8(buf).unwrap();
        assert!(!out.contains("Resolved"), "cached probes must stay silent: {out}");
        assert!(out.contains("Cached deps (2 nodes)"), "got: {out}");
    }

    #[test]
    fn probes_only_work_still_collapses_recipe_summary() {
        // Probes re-ran but every real node was cached: still a warm no-op —
        // one Resolved line plus the dim collapsed summary.
        let mut state = deps_state(3);
        let mut buf = Vec::new();
        let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

        apply_cache_hits(&mut state, &mut w, &mut buf, 2);
        let started = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(2),
            name: "probe:cc:compiler:auto".into(), artifact: None,
            fallback_label: "probe:cc:compiler:auto".into(), kind: NodeKind::Resolve,
                cause: None,
            };
        state.apply(&started);
        let probe = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(2),
            elapsed: Duration::from_millis(20), kind: NodeKind::Resolve,
        };
        state.apply(&probe);
        w.handle(&mut buf, &state, &probe).unwrap();

        let done = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(30),
            cached: 2, total: 3,
            kind: crate::event::RecipeKind::Recipe,
        };
        state.apply(&done);
        w.handle(&mut buf, &state, &done).unwrap();

        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Resolved cc toolchain for deps (1 probe) in 0.02s"), "got: {out}");
        assert!(out.contains("Cached deps (3 nodes)"), "got: {out}");
        assert!(!out.contains("a0.o"), "held cached lines must be dropped: {out}");
        assert!(!out.contains("Finished"), "got: {out}");
    }

    #[test]
    fn internal_recipe_shows_module_tag_and_no_summary() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0),
                name: "__cc_config_header__build_dhewm3_config_h".into(),
                deps: vec![], expected_nodes: 1,
            }],
            total_nodes: 1,
        });
        let mut buf = Vec::new();
        let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

        let started = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "config.h".into(),
            artifact: Some("build/dhewm3/config.h".into()),
            fallback_label: "render config.h".into(), kind: NodeKind::Generate,
                cause: None,
            };
        state.apply(&started);
        let ev = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(10), kind: NodeKind::Generate,
        };
        state.apply(&ev);
        w.handle(&mut buf, &state, &ev).unwrap();

        let done = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(15),
            cached: 0, total: 1,
            kind: crate::event::RecipeKind::Recipe,
        };
        state.apply(&done);
        w.handle(&mut buf, &state, &done).unwrap();

        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Generated cc/build/dhewm3/config.h"), "got: {out}");
        assert!(!out.contains("__cc_config_header"), "raw minted name must not leak: {out}");
        assert!(!out.contains("Finished"), "internal recipes have no summary row: {out}");
    }

    #[test]
    fn node_started_with_cause_prints_rebuilding_line() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        let ev = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "next build".into(),
            artifact: Some("apps/web/.next".into()),
            fallback_label: "next build".into(),
            kind: NodeKind::Cooked,
            cause: Some("input changed: apps/web/app/.well-known/workflow/v1/manifest.json (+2 more)".into()),
        };
        state.apply(&ev);
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert_eq!(
            out,
            "  Rebuilding lib/apps/web/.next — input changed: apps/web/app/.well-known/workflow/v1/manifest.json (+2 more)\n",
            "got: {out:?}"
        );
    }

    #[test]
    fn node_started_without_cause_stays_silent() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        let ev = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "x.c".into(), artifact: Some("x.o".into()),
            fallback_label: "cc x.c".into(), kind: NodeKind::Compile,
            cause: None,
        };
        state.apply(&ev);
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert_eq!(out, "", "cold start must not print an attribution line");
    }

    #[test]
    fn cause_line_flushes_held_cached_lines_first() {
        let mut state = deps_state(3);
        let mut buf = Vec::new();
        let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });
        apply_cache_hits(&mut state, &mut w, &mut buf, 2);
        let ev = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(2),
            name: "c.c".into(), artifact: Some("c.o".into()),
            fallback_label: "cc c.c".into(), kind: NodeKind::Compile,
            cause: Some("input changed: c.c".into()),
        };
        state.apply(&ev);
        w.handle(&mut buf, &state, &ev).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let cached_pos = out.find("Cached").expect("held cached lines flushed");
        let rebuild_pos = out.find("Rebuilding").expect("cause line printed");
        assert!(cached_pos < rebuild_pos, "got: {out}");
    }

    #[test]
    fn finished_all_cached_says_all_cached() {
        let mut state = deps_state(6);
        let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });
        let mut buf = Vec::new();
        apply_cache_hits(&mut state, &mut w, &mut buf, 6);
        let done = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(5),
            cached: 6, total: 6,
            kind: crate::event::RecipeKind::Recipe,
        };
        state.apply(&done);
        w.handle(&mut buf, &state, &done).unwrap();
        let fin = ProgressEvent::Finished { success: true };
        state.apply(&fin);
        w.handle(&mut buf, &state, &fin).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("all cached"), "got: {out}");
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
                cause: None,
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
                cause: None,
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
            kind: crate::event::RecipeKind::Recipe,
        };
        state.apply(&recipe_done);
        w.handle(&mut buf, &state, &recipe_done).unwrap();

        let out = String::from_utf8(buf).unwrap();
        assert!(!out.contains("Compiled"), "quiet should suppress per-node verbs: {out}");
        assert!(out.contains("Finished lib"), "got: {out}");
    }

    #[test]
    fn verbose_emits_node_output_lines() {
        // The live-stdout tag must use the node's own full output path
        // (its `display()` label) — not its raw node name/command text.
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "lvm.c".into(),
            artifact: Some("build/obj/lvm.o".into()),
            fallback_label: "x".into(),
            kind: NodeKind::Compile,
                cause: None,
            });
        let ev = ProgressEvent::NodeOutput {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            line: "warning: unused".into(), stream: Stream::Stderr,
        };
        let opts = EventWriterOptions { colored: false, verbose: true, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert_eq!(out, "[lib/build/obj/lvm.o] (stderr) warning: unused\n");
    }

    #[test]
    fn finished_success_emits_subjectless_summary() {
        let mut state = empty_state();
        state.totals.completed_nodes = 47;
        let ev = ProgressEvent::Finished { success: true };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        // No "build" subject, no collision with a recipe of the same name.
        assert!(out.starts_with("    Finished in "), "got: {out}");
        assert!(out.contains("(47 nodes, 0 cached)"), "got: {out}");
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

    #[test]
    fn terminal_interactive_end_suppresses_subsequent_output() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });

        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let mut w = EventWriter::new(opts);
        let mut buf = Vec::new();

        // Chore-style sequence: InteractiveStart → InteractiveEnd(terminal) → trailing events.
        let start = ProgressEvent::InteractiveStart {
            recipe: RecipeId::new(0), node: NodeId::new(0), name: "@45".into(),
            chore_step_count: 0,
        };
        state.apply(&start);
        w.handle(&mut buf, &state, &start).unwrap();

        let end = ProgressEvent::InteractiveEnd {
            recipe: RecipeId::new(0), node: NodeId::new(0), name: "@45".into(),
            elapsed: Duration::from_millis(10),
            success: true,
            is_terminal: true,
            failed_step: None,
        };
        state.apply(&end);
        w.handle(&mut buf, &state, &end).unwrap();

        // These would normally print but should be suppressed after a terminal chore end.
        for ev in [
            ProgressEvent::NodeCompleted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                elapsed: Duration::from_millis(10), kind: NodeKind::Cooked,
            },
            ProgressEvent::RecipeCompleted {
                recipe: RecipeId::new(0),
                elapsed: Duration::from_millis(15),
                cached: 0, total: 1,
                kind: crate::event::RecipeKind::Recipe,
            },
            ProgressEvent::Finished { success: true },
        ] {
            state.apply(&ev);
            w.handle(&mut buf, &state, &ev).unwrap();
        }

        let out = String::from_utf8(buf).unwrap();
        // Only one Running line; nothing else.
        let line_count = out.lines().count();
        assert_eq!(line_count, 1, "expected only the Running line; got: {out}");
        assert!(out.contains("Running"), "got: {out}");
        assert!(!out.contains("Cooked"), "Cooked should be suppressed: {out}");
        assert!(!out.contains("Finished"), "Finished should be suppressed: {out}");
    }

    #[test]
    fn node_completed_no_artifact_emits_no_line() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "@45".into(),
            artifact: None,
            fallback_label: "@45".into(),
            kind: NodeKind::Cooked,
                cause: None,
            });
        let ev = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(100),
            kind: NodeKind::Cooked,
        };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert_eq!(out, "", "anonymous shell step (no artifact) must emit nothing, got: {out:?}");
    }

    #[test]
    fn node_completed_no_artifact_verbose_still_prints() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "@45".into(), artifact: None, fallback_label: "@45".into(),
            kind: NodeKind::Cooked,
                cause: None,
            });
        let ev = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(100),
            kind: NodeKind::Cooked,
        };
        let opts = EventWriterOptions { colored: false, verbose: true, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert!(out.contains("Cooked"), "verbose path should still print Cooked line, got: {out:?}");
    }

    #[test]
    fn node_cache_hit_no_artifact_emits_no_line() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "@45".into(), artifact: None, fallback_label: "@45".into(),
            kind: NodeKind::Cooked,
                cause: None,
            });
        let ev = ProgressEvent::NodeCacheHit {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "@45".into(),
            artifact: None,
            kind: NodeKind::Cooked,
        };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert_eq!(out, "");
    }

    #[test]
    fn recipe_completed_zero_nodes_emits_no_line() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        let ev = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(0),
            cached: 0, total: 0,
            kind: crate::event::RecipeKind::Recipe,
        };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert_eq!(out, "", "aggregator (total=0) must emit nothing, got: {out:?}");
    }

    #[test]
    fn recipe_completed_one_node_still_prints() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        let ev = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(100),
            cached: 0, total: 1,
            kind: crate::event::RecipeKind::Recipe,
        };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert!(out.contains("Finished lib"), "single-node recipe should still print, got: {out:?}");
    }

    #[test]
    fn recipe_completed_chore_kind_uses_chore_detail() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        let ev = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(4910),
            cached: 0, total: 4,
            kind: crate::event::RecipeKind::Chore,
        };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        assert!(out.contains("(chore)"), "chore recipe summary should show (chore), got: {out:?}");
        assert!(!out.contains("nodes"), "chore detail must not mention node math, got: {out:?}");
    }

    #[test]
    fn chore_window_failure_renders_step_index_and_chore_name() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        // Apply NodeStarted with the chore name as both name and (no) artifact.
        // In the real engine flow, the chore-window failure path emits NodeFailed
        // with `name = chore_recipe`; here we synthesize that view of the state.
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0),
            node: NodeId::new(0),
            name: "play".into(),
            artifact: None,
            fallback_label: "play".into(),
            kind: NodeKind::Cooked,
                cause: None,
            });
        let ev = ProgressEvent::NodeFailed {
            recipe: RecipeId::new(0),
            node: NodeId::new(0),
            elapsed: Duration::from_millis(400),
            error: "step 2/4: exit 130".into(),
        };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        // node_display for an artifact-less node with fallback_label "play" (no leading '$'):
        // stripped = "play", first = "play", not starting with '@', so returns "$play".
        assert!(out.contains("Failed lib/$play") || out.contains("Failed lib/play"),
            "expected 'Failed lib/$play' (with optional $-prefix from node_display fallback), got: {out:?}");
        assert!(out.contains("step 2/4: exit 130"), "got: {out:?}");
    }

    #[test]
    fn interactive_start_with_at_tag_drops_the_tag() {
        let mut state = empty_state();
        state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });

        let ev = ProgressEvent::InteractiveStart {
            recipe: RecipeId::new(0), node: NodeId::new(0), name: "@45".into(),
            chore_step_count: 0,
        };
        let opts = EventWriterOptions { colored: false, ..Default::default() };
        let out = render_one(&state, &ev, opts);
        // Should be "Running lib" (the recipe name in empty_state), not "Running lib/@45".
        assert_eq!(out, "     Running lib\n", "got: {out:?}");
    }
}
