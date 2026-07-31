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
    // COOK-392: THE duration law.
    cook_contracts::render::duration_ms(d.as_millis() as u64)
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
            ProgressEvent::NodeCompleted { recipe, node, elapsed, kind: _, cache_key: _ } => {
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
#[path = "tests/plain_tests.rs"]
mod tests;
