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
#[path = "tests/event_writer_tests.rs"]
mod tests;
