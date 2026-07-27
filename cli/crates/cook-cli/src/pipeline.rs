//! CLI dispatch: thin wrappers around `cook_engine::pipeline` orchestration.
//!
//! Heavy lifting (parse, workspace load, registry assembly, dep inference)
//! lives in `cook_engine::pipeline`. This module owns CLI-specific glue:
//!   * mapping `menu` / `init` / `serve` / `dag` subcommands to the right
//!     engine entry point
//!   * bridging `cook_engine::EngineEvent` to `cook_progress::ProgressEvent`
//!     and wiring the renderer into a background thread
//!   * mapping `EngineError` / `PipelineError` into `CookError` for exit-code
//!     classification and human-facing diagnostics
//!
//! In particular: nothing in this file consumes a `cook_lang::ast::Cookfile`
//! directly anymore — that's the engine's concern.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{OnceLock, mpsc};

use cook_contracts::CommandFailure;
use cook_engine::pipeline::{self, ParsedCookfile, PipelineError, RegisterMode, Workspace};
use cook_engine::RegisteredWorkspace;

use crate::cli::Globals;
use crate::error::CookError;
use crate::progress::spawn_new_renderer;
use crate::watcher::CookWatcher;

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// Map `cook_engine::pipeline::PipelineError` onto `CookError` for the CLI's
/// exit-code classification.
fn pipeline_error_to_cook_error(e: PipelineError) -> CookError {
    match e {
        PipelineError::Io { path, source } => {
            CookError::Other(format!("cannot read {}: {source}", path.display()))
        }
        PipelineError::Parse(msg) => CookError::ParseError(msg),
        PipelineError::Codegen(msg) => CookError::Other(msg),
        PipelineError::RecipeCollision { name, sites } => {
            // Multi-line diagnostic per spec §8: name each registration site
            // by line + kind label. The CLI's outer harness will print this
            // verbatim via `Display` and exit with code 3.
            let mut msg = format!("error: recipe '{name}' is registered more than once:\n");
            for s in &sites {
                let kind_str = match s.kind {
                    cook_engine::cook_register::RegistrationSiteKind::SurfaceRecipe => {
                        "as a `recipe` block"
                    }
                    cook_engine::cook_register::RegistrationSiteKind::SurfaceChore => {
                        "as a `chore` block"
                    }
                    cook_engine::cook_register::RegistrationSiteKind::Dynamic => {
                        "by cook.recipe at register-phase"
                    }
                    cook_engine::cook_register::RegistrationSiteKind::DynamicChore => {
                        "by cook.chore at register-phase"
                    }
                };
                // CS-0176: a module-registered chore's line points into the
                // MODULE, not the Cookfile, so prefixing it with `Cookfile:`
                // would send the reader to an unrelated line of their own file.
                // Name the site without a false location instead.
                if s.kind == cook_engine::cook_register::RegistrationSiteKind::DynamicChore {
                    msg.push_str(&format!("  - {}\n", kind_str));
                } else {
                    msg.push_str(&format!("  - Cookfile:{}: {}\n", s.line, kind_str));
                }
            }
            msg.push_str("rename one of them.");
            CookError::RecipeCollision(msg)
        }
        // An ambiguous graph, not a runtime failure: two units cannot both
        // produce one path. Classified with RecipeCollision — both are "the
        // Cookfile declares two things sharing one identity", and both are
        // fixed by editing the Cookfile, not by re-running.
        PipelineError::DuplicateOutput { .. } => {
            CookError::RecipeCollision(format!("error: {e}"))
        }
        PipelineError::UnknownConfig { .. }
        | PipelineError::Workspace(_)
        | PipelineError::InvalidSet(_)
        | PipelineError::Other(_) => CookError::Other(e.to_string()),
    }
}

/// Parse the entry Cookfile (AST + Lua + warnings), mapping errors onto
/// `CookError`.
///
/// §5.5 codegen warnings are NOT printed here — every workspace-loading
/// command surfaces them via [`load_workspace`] (the one load each command
/// performs), so a command that both parses and loads prints them exactly
/// once. `cmd_emit_lua`, which never loads a workspace, prints
/// `parsed.warnings` itself.
fn read_and_parse(globals: &Globals) -> Result<ParsedCookfile, CookError> {
    pipeline::read_and_parse(&globals.file).map_err(pipeline_error_to_cook_error)
}

// ---------------------------------------------------------------------------
// EngineEvent → ProgressEvent bridge
// ---------------------------------------------------------------------------

/// Translate the engine's `NodeKind` mirror onto `cook_progress::NodeKind`.
/// The two enums are isomorphic by design — keeping them separate lets
/// `cook-engine` stay free of a `cook-progress` dependency.
fn translate_kind(k: cook_engine::NodeKind) -> cook_progress::NodeKind {
    match k {
        cook_engine::NodeKind::Compile => cook_progress::NodeKind::Compile,
        cook_engine::NodeKind::Link => cook_progress::NodeKind::Link,
        cook_engine::NodeKind::Resolve => cook_progress::NodeKind::Resolve,
        cook_engine::NodeKind::Generate => cook_progress::NodeKind::Generate,
        cook_engine::NodeKind::Write => cook_progress::NodeKind::Write,
        cook_engine::NodeKind::Test => cook_progress::NodeKind::Test,
        cook_engine::NodeKind::Cooked => cook_progress::NodeKind::Cooked,
    }
}

/// Bridge cook-engine events to the new cook-progress ProgressEvent stream.
/// Interns recipe names and node names into stable `RecipeId` / `NodeId`.
fn bridge_engine_to_progress_events(
    engine_rx: mpsc::Receiver<cook_engine::EngineEvent>,
    progress_tx: mpsc::Sender<cook_progress::ProgressEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use cook_progress::{NodeId, RecipeId, RecipeTopo, SkipReason, Stream};
        use std::collections::BTreeMap;

        let mut recipe_ids: BTreeMap<String, RecipeId> = BTreeMap::new();
        // Keyed by (recipe, executor DAG node index) — the engine's `unit`
        // field is unique per run, so this is a correct identity even when
        // two units display identical text (same-shaped steps, or N Lua
        // fan-out members that all `display_name() == "lua"`). The prior
        // scheme keyed by (recipe, node_name STRING) collapsed all such
        // units onto one NodeId and overwrote one another's NodeState.
        let mut node_ids: BTreeMap<(String, usize), NodeId> = BTreeMap::new();
        // `NodeSkipped` / `InteractiveStart` / `InteractiveEnd` are out of
        // scope for the per-unit identity fix (they carry no `unit` field —
        // §22.8 interactive/chore windows and cancellation fan-out aren't
        // the display-collision bug this bridges): keep the old name-keyed
        // interning for exactly these three so `InteractiveStart`/`End`
        // (which already share one `node_name` per window) keep resolving
        // to the same `NodeId` pair they always have.
        let mut interactive_node_ids: BTreeMap<(String, String), NodeId> = BTreeMap::new();
        let mut next_recipe: u32 = 0;
        let mut next_node: u32 = 0;

        fn intern_recipe(
            name: &str,
            recipe_ids: &mut BTreeMap<String, RecipeId>,
            next_recipe: &mut u32,
        ) -> RecipeId {
            if let Some(id) = recipe_ids.get(name) {
                return *id;
            }
            let id = RecipeId::new(*next_recipe);
            *next_recipe += 1;
            recipe_ids.insert(name.to_string(), id);
            id
        }

        /// Find-or-create the stable `NodeId` for one `(recipe, unit)` pair.
        /// Order-independent: whichever event mentions a given unit first
        /// mints its `NodeId`; every later event for the same unit — whether
        /// `NodeStarted`, `NodeCompleted`, or `OutputLine` — resolves to the
        /// same id regardless of arrival order relative to sibling units.
        fn intern_node(
            recipe: &str,
            unit: usize,
            node_ids: &mut BTreeMap<(String, usize), NodeId>,
            next_node: &mut u32,
        ) -> NodeId {
            let key = (recipe.to_string(), unit);
            if let Some(id) = node_ids.get(&key) {
                return *id;
            }
            let id = NodeId::new(*next_node);
            *next_node += 1;
            node_ids.insert(key, id);
            id
        }

        /// Legacy name-keyed interning, retained only for the three events
        /// this task doesn't touch (see `interactive_node_ids` above).
        fn intern_interactive_node(
            recipe: &str,
            node: &str,
            interactive_node_ids: &mut BTreeMap<(String, String), NodeId>,
            next_node: &mut u32,
        ) -> NodeId {
            let key = (recipe.to_string(), node.to_string());
            if let Some(id) = interactive_node_ids.get(&key) {
                return *id;
            }
            let id = NodeId::new(*next_node);
            *next_node += 1;
            interactive_node_ids.insert(key, id);
            id
        }

        while let Ok(event) = engine_rx.recv() {
            let pe = match event {
                cook_engine::EngineEvent::BuildStarted { recipes, total_nodes } => {
                    let topos: Vec<RecipeTopo> = recipes
                        .into_iter()
                        .map(|r| {
                            let id = intern_recipe(&r.name, &mut recipe_ids, &mut next_recipe);
                            let deps: Vec<RecipeId> = r
                                .deps
                                .iter()
                                .map(|d| intern_recipe(d, &mut recipe_ids, &mut next_recipe))
                                .collect();
                            RecipeTopo {
                                id,
                                name: r.name,
                                deps,
                                expected_nodes: r.expected_nodes,
                            }
                        })
                        .collect();
                    cook_progress::ProgressEvent::BuildStarted {
                        recipes: topos,
                        total_nodes,
                    }
                }
                cook_engine::EngineEvent::RecipeQueued { .. } => continue,
                cook_engine::EngineEvent::RecipeStarted { name, .. } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeStarted { recipe: id }
                }
                cook_engine::EngineEvent::RecipeCompleted {
                    name,
                    elapsed,
                    cached_nodes,
                    total_nodes,
                    kind,
                } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeCompleted {
                        recipe: id,
                        elapsed,
                        cached: cached_nodes,
                        total: total_nodes,
                        kind: match kind {
                            cook_engine::RecipeKind::Recipe => cook_progress::event::RecipeKind::Recipe,
                            cook_engine::RecipeKind::Chore => cook_progress::event::RecipeKind::Chore,
                        },
                    }
                }
                cook_engine::EngineEvent::RecipeFailed {
                    name,
                    elapsed,
                    completed_nodes,
                    total_nodes,
                } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeFailed {
                        recipe: id,
                        elapsed,
                        completed: completed_nodes,
                        total: total_nodes,
                    }
                }
                cook_engine::EngineEvent::RecipeSkipped {
                    name,
                    elapsed,
                    skipped_nodes,
                    completed_nodes,
                    total_nodes,
                } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeSkipped {
                        recipe: id,
                        elapsed,
                        skipped: skipped_nodes,
                        completed: completed_nodes,
                        total: total_nodes,
                    }
                }
                cook_engine::EngineEvent::NodeStarted {
                    recipe,
                    unit,
                    node_name,
                    artifact,
                    fallback_label,
                    kind,
                    cause,
                    cache_key,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, unit, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeStarted {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        artifact,
                        fallback_label,
                        kind: translate_kind(kind),
                        cause,
                        cache_key,
                    }
                }
                cook_engine::EngineEvent::NodeCompleted {
                    recipe,
                    unit,
                    node_name: _,
                    elapsed,
                    kind,
                    cache_key,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, unit, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeCompleted {
                        recipe: rid,
                        node: nid,
                        elapsed,
                        kind: translate_kind(kind),
                        cache_key,
                    }
                }
                cook_engine::EngineEvent::NodeFailed {
                    recipe,
                    unit,
                    node_name: _,
                    elapsed,
                    error,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, unit, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeFailed {
                        recipe: rid,
                        node: nid,
                        elapsed,
                        error,
                    }
                }
                cook_engine::EngineEvent::NodeCacheHit {
                    recipe,
                    unit,
                    node_name,
                    artifact,
                    kind,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, unit, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeCacheHit {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        artifact,
                        kind: translate_kind(kind),
                    }
                }
                cook_engine::EngineEvent::NodeSkipped { recipe, node_name } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_interactive_node(&recipe, &node_name, &mut interactive_node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeSkipped {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        reason: SkipReason::UpstreamFailed,
                    }
                }
                cook_engine::EngineEvent::OutputLine {
                    recipe,
                    unit,
                    node_name,
                    line,
                    stream,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    // The engine's `unit` is the executor DAG node index —
                    // unique per run — so interning by (recipe, unit) is
                    // correct regardless of arrival order relative to sibling
                    // units. If this is the first event ever seen for this
                    // unit (e.g. output forwarded from a chore-window Lua
                    // chunk, which never gets its own `NodeStarted`),
                    // register it with a real label via a synthetic
                    // `NodeStarted` so renderers/log-store don't fall back to
                    // a blank/placeholder name.
                    let is_new_unit = !node_ids.contains_key(&(recipe.clone(), unit));
                    let nid = intern_node(&recipe, unit, &mut node_ids, &mut next_node);
                    if is_new_unit {
                        let _ = progress_tx.send(cook_progress::ProgressEvent::NodeStarted {
                            recipe: rid,
                            node: nid,
                            name: node_name.clone(),
                            artifact: None,
                            fallback_label: node_name,
                            kind: cook_progress::NodeKind::Cooked,
                            cause: None,
                            // Synthetic: this stands in for a unit that never
                            // emitted a NodeStarted of its own, so there is no
                            // cache identity to attach.
                            cache_key: None,
                        });
                    }
                    // CS-0035: map cook-contracts::OutputStream → cook-progress::Stream
                    // (the wire-format enum). The two enums are isomorphic; this
                    // is the renderer-side adapter, not a value mutation.
                    let stream = match stream {
                        cook_engine::Stream::Stdout => Stream::Stdout,
                        cook_engine::Stream::Stderr => Stream::Stderr,
                        // CS-0049: `OutputStream` is `#[non_exhaustive]`; future
                        // variants (e.g. PTY-tagged output) default to stdout
                        // until a CS adds them to the wire enum mapping.
                        _ => Stream::Stdout,
                    };
                    cook_progress::ProgressEvent::NodeOutput {
                        recipe: rid,
                        node: nid,
                        line,
                        stream,
                    }
                }
                cook_engine::EngineEvent::InteractiveStart { recipe, node_name, chore_step_count } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_interactive_node(&recipe, &node_name, &mut interactive_node_ids, &mut next_node);
                    cook_progress::ProgressEvent::InteractiveStart {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        chore_step_count,
                    }
                }
                cook_engine::EngineEvent::InteractiveEnd {
                    recipe,
                    node_name,
                    elapsed,
                    success,
                    is_terminal,
                    failed_step,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_interactive_node(&recipe, &node_name, &mut interactive_node_ids, &mut next_node);
                    cook_progress::ProgressEvent::InteractiveEnd {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        elapsed,
                        success,
                        is_terminal,
                        failed_step,
                    }
                }
                cook_engine::EngineEvent::Finished { success, .. } => {
                    cook_progress::ProgressEvent::Finished { success }
                }
                // Phase 4 will wire real progress handlers for test events.
                cook_engine::EngineEvent::TestStarted { .. }
                | cook_engine::EngineEvent::TestPassed { .. }
                | cook_engine::EngineEvent::TestFailed { .. }
                | cook_engine::EngineEvent::TestBlocked { .. }
                | cook_engine::EngineEvent::TestTimedOut { .. } => continue,
            };
            let is_finished = matches!(pe, cook_progress::ProgressEvent::Finished { .. });
            let _ = progress_tx.send(pe);
            if is_finished {
                break;
            }
        }
    })
}

/// Reported commands carry codegen's `set -e` prelude; strip it for display.
fn strip_set_e(cmd: &str) -> &str {
    cmd.strip_prefix("set -e\n").unwrap_or(cmd)
}

fn render_command_failure(failure: &CommandFailure) -> String {
    let command = strip_set_e(failure.command());
    let mut message = if failure.line() == 0 {
        format!("command failed (exit {}): {command}", failure.exit_code())
    } else {
        format!(
            "Cookfile:{}: command failed (exit {}): {command}",
            failure.line(),
            failure.exit_code()
        )
    };
    if !failure.stdout().is_empty() {
        message.push_str("\n--- stdout ---\n");
        message.push_str(failure.stdout().as_str());
        if !message.ends_with('\n') {
            message.push('\n');
        }
    }
    if !failure.stderr().is_empty() {
        if !message.ends_with('\n') {
            message.push('\n');
        }
        message.push_str("--- stderr ---\n");
        message.push_str(failure.stderr().as_str());
    }
    message
}

/// Map cook-engine errors to CookError.
fn engine_error_to_cook_error(e: cook_engine::EngineError) -> CookError {
    match e {
        cook_engine::EngineError::TaskFailures { failures, .. } => {
            if let Some((_, _recipe_name, msg)) = failures.first() {
                if let Some(failure) = CommandFailure::from_wire(msg) {
                    CookError::CommandFailed(render_command_failure(&failure))
                } else {
                    CookError::Other(msg.clone())
                }
            } else {
                CookError::Other("unknown engine error".into())
            }
        }
        cook_engine::EngineError::CycleDetected(name) => {
            CookError::Other(format!("dependency cycle involving: {name}"))
        }
        cook_engine::EngineError::UnknownRecipe(name) => CookError::RecipeNotFound(name),
        cook_engine::EngineError::RegistrationFailed { recipe, message } => {
            CookError::Other(format!("registration failed for '{recipe}': {message}"))
        }
        cook_engine::EngineError::CacheError(msg) => CookError::Other(msg),
        cook_engine::EngineError::OutputCollision { path, recipes } => CookError::Other(format!(
            "output collision: recipes [{}] all declare output {} with no dependency edge between them; \
             add an explicit `: <recipe>` dep or merge into one recipe",
            recipes.join(", "),
            path.display(),
        )),
        cook_engine::EngineError::GlobbedOutputCrossRecipeEdge {
            upstream,
            downstream,
            input,
            pattern,
        } => CookError::Other(format!(
            "recipe '{downstream}' reads input '{input}' which is matched by recipe '{upstream}' \
             output pattern '{pattern}'. file-level cross-recipe edges to globbed outputs are not \
             supported. either narrow '{upstream}' outputs[] to the specific file, or depend on \
             '{upstream}' via a requires edge (recipe {downstream}: {upstream})."
        )),
        cook_engine::EngineError::LiteralReadAfterWrite {
            producer,
            consumer,
            path,
        } => CookError::Other(format!(
            "read-after-write with no ordering edge: recipe '{consumer}' reads literal input \
             '{path}', which recipe '{producer}' declares as a literal output — but '{consumer}' \
             does not require '{producer}', so nothing orders the write before the read (a \
             silent stale read under --jobs > 1). Path equality alone creates no ordering edge; \
             name the producer explicitly: add `: {producer}` to the '{consumer}' recipe header, \
             refer to the output as $<{producer}>, or call cook.require_recipe(\"{producer}\") \
             from its body."
        )),
        cook_engine::EngineError::DanglingDepEdge { referring_recipe, dep_name } => {
            CookError::Other(format!(
                "recipe '{referring_recipe}' has a fine-grained dependency (dep_edges) on recipe \
                 '{dep_name}', which is not present in the build closure. Add `: {dep_name}` to \
                 the '{referring_recipe}' recipe header, or call \
                 cook.require_recipe(\"{dep_name}\") from its body."
            ))
        }
    }
}

#[cfg(test)]
#[path = "tests/engine_error_to_cook_error_tests.rs"]
mod engine_error_to_cook_error_tests;

// ---------------------------------------------------------------------------
// Run-with-progress glue
// ---------------------------------------------------------------------------

/// Run the engine with progress rendering wired up.
///
/// Print a one-line-per-path summary of stale-output reconciliation (§17.7):
/// orphaned outputs Cook swept, and orphans kept because the user changed them
/// since Cook wrote them. Suppressed under `--quiet`; no-op when both are empty.
fn report_reconciliation(
    globals: &Globals,
    swept: &[std::path::PathBuf],
    kept_modified: &[std::path::PathBuf],
) {
    if globals.quiet || (swept.is_empty() && kept_modified.is_empty()) {
        return;
    }
    for p in swept {
        eprintln!("cook: swept orphaned output: {}", p.display());
    }
    for p in kept_modified {
        eprintln!(
            "cook: {} changed since Cook wrote it — not removing",
            p.display()
        );
    }
}

/// Whether stale-output reconciliation (§17.7) is disabled for this run, via
/// `--no-prune` or the `COOK_NO_PRUNE` environment variable (any non-empty
/// value other than `0`).
fn no_prune_enabled(globals: &Globals) -> bool {
    globals.no_prune
        || std::env::var("COOK_NO_PRUNE")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
}

/// Pure env-value semantics for the `COOK_NO_AUTO_GC` escape hatch: any
/// non-empty value other than `"0"` enables it. Factored out from
/// `no_auto_gc_enabled` so the five semantic cases (unset, `"1"`, `"0"`,
/// `""`, an arbitrary non-empty value) can be unit-tested against a plain
/// `Option<&str>` without any test touching the shared process environment.
fn no_auto_gc_env_value_enables(value: Option<&str>) -> bool {
    value.map(|v| !v.is_empty() && v != "0").unwrap_or(false)
}

/// Whether the automatic `[cache] auto_gc` store-budget sweep (COOK-235) is
/// disabled for this run, via `--no-auto-gc` or the `COOK_NO_AUTO_GC`
/// environment variable (any non-empty value other than `0`). This only
/// suppresses the deletion — the over-budget warning still prints — so CI can
/// stay deterministic while an operator still learns the store needs a sweep.
fn no_auto_gc_enabled(globals: &Globals) -> bool {
    globals.no_auto_gc
        || no_auto_gc_env_value_enables(std::env::var("COOK_NO_AUTO_GC").ok().as_deref())
}

/// True when this invocation must run in publish-off / read-only mode, set
/// by `--no-publish` or a non-empty `COOK_NO_PUBLISH` env var. This only
/// turns publishing OFF; it never overrides a `[cloud] publish = false`
/// config back on. (COOK-168.)
fn no_publish_enabled(globals: &Globals) -> bool {
    globals.no_publish
        || std::env::var("COOK_NO_PUBLISH")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
}

/// Walks the unified work-unit DAG built from `registered_workspace`. The
/// recipe-level edge map (and therefore the reachable closure) is computed
/// here from `recipe_infos` via `analyzer::dependency_edges_multi`; the
/// engine takes responsibility for the topological order downstream.
fn run_with_progress(
    globals: &Globals,
    recipe_infos: &BTreeMap<String, cook_engine::analyzer::RecipeInfo>,
    targets: &[String],
    registered_workspace: &RegisteredWorkspace,
    num_jobs: usize,
) -> Result<cook_engine::run::RunResult, CookError> {
    let project_root = resolve_project_root(globals)?;

    // Recipe-level dependency edges across the reachable closure. The engine
    // toposorts internally; we just need a complete edge map keyed by every
    // reachable recipe name.
    let mut edges = cook_engine::analyzer::dependency_edges_multi(recipe_infos, targets).map_err(
        |e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                CookError::RecipeNotFound(name)
            }
            other => CookError::Other(other.to_string()),
        },
    )?;
    let mut reachable: BTreeSet<String> = edges.keys().cloned().collect();

    if globals.affected {
        let since = globals.since.as_deref().ok_or_else(|| {
            CookError::Other("--affected requires --since=<git-ref>".to_string())
        })?;
        let changed = cook_engine::affected::git::changed_paths(&project_root, since)
            .map_err(|e| CookError::Other(format!("git diff failed: {e}")))?;
        reachable = cook_engine::affected::compute_affected(
            &changed,
            registered_workspace,
            &edges,
            &reachable,
            &project_root,
        );
        if reachable.is_empty() {
            let recipe_name = targets.first().map(String::as_str).unwrap_or("?");
            eprintln!("cook: nothing affected for recipe '{recipe_name}' since {since}");
            return Ok(cook_engine::run::RunResult {
                test_results: vec![],
                swept: vec![],
                kept_modified: vec![],
                output_glob_warnings: vec![],
                // No work ran, so nothing was published.
                published_count: 0,
            });
        }
        edges.retain(|k, _| reachable.contains(k));
        for deps in edges.values_mut() {
            deps.retain(|d| reachable.contains(d));
        }
    }

    let (progress_tx, progress_rx) = mpsc::channel::<cook_progress::ProgressEvent>();
    let render_thread = spawn_new_renderer(globals, project_root.clone(), progress_rx);

    let bridge_tx = progress_tx.clone();
    let (engine_tx, engine_rx) = mpsc::channel::<cook_engine::EngineEvent>();
    let bridge_thread = bridge_engine_to_progress_events(engine_rx, bridge_tx);

    let no_prune = no_prune_enabled(globals);
    let no_publish = no_publish_enabled(globals);
    let result = cook_engine::run::run(
        &project_root,
        registered_workspace,
        &edges,
        &reachable,
        num_jobs,
        &[],
        no_prune,
        no_publish,
        move |event| {
            let _ = engine_tx.send(event);
        },
    );

    // Wait for bridge to drain and forward Finished, then renderer to exit.
    let _ = bridge_thread.join();
    // Drop progress_tx before joining the render thread so the channel is
    // closed even if the engine exited abnormally without emitting Finished.
    drop(progress_tx);
    let _success = render_thread.join().unwrap_or(false);

    // Report stale-output reconciliation (§17.7) after the renderer has
    // released the terminal, so the lines don't fight the progress display.
    if let Ok(r) = &result {
        report_reconciliation(globals, &r.swept, &r.kept_modified);
        for warning in &r.output_glob_warnings {
            eprintln!(
                "cook: warning: output \"{}\" matched 0 files (recipe {})",
                warning.pattern, warning.recipe
            );
        }

        // COOK-235. Same placement rationale as the lines above: after the
        // renderer released the terminal. Inside the `Ok` arm because a
        // failed run has no trustworthy publish count — and the check is a
        // no-op unless this run actually published something.
        crate::cache_budget::check_budget_after_run(
            &project_root,
            r.published_count,
            no_auto_gc_enabled(globals),
        );
    }

    result.map_err(engine_error_to_cook_error)
}

/// Resolve num_jobs from globals or system parallelism.
fn resolve_num_jobs(globals: &Globals) -> usize {
    globals.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    })
}

// ---------------------------------------------------------------------------
// build_registered_workspace helper
// ---------------------------------------------------------------------------

/// Resolve the workspace root and load the workspace — root + every
/// transitive import. A Cookfile with no imports loads as a workspace of
/// one member with prefix "" (workspace-of-one); there is no separate
/// single-Cookfile path.
fn load_workspace(globals: &Globals) -> Result<Workspace, CookError> {
    let workspace_root = pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
        .map_err(pipeline_error_to_cook_error)?;
    let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set)
        .map_err(pipeline_error_to_cook_error)?;
    // §5.5 codegen warnings for the entry Cookfile — printed here, the one
    // workspace load every command performs, so they appear exactly once per
    // invocation (including under upward entry discovery).
    for w in &workspace.warnings {
        eprintln!("cook: warning: {w}");
    }
    Ok(workspace)
}

/// Root-anchored project root (§20.2.3 / CS-0120): every command that
/// anchors cache state (`.cook/cloud.toml`, probe store, logs, test state)
/// or git-diff scope resolves the SAME workspace root the sigil/import
/// machinery uses — never the invocation cwd. cwd only selects the entry
/// Cookfile (via upward discovery in main.rs).
pub(crate) fn resolve_project_root(globals: &Globals) -> Result<std::path::PathBuf, CookError> {
    pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
        .map_err(pipeline_error_to_cook_error)
}

/// Register every Cookfile in the workspace and return the loaded
/// `Workspace` (post module-recipe codegen) together with the resulting
/// `RegisteredWorkspace`. The sole registration path for every command.
///
/// `mode` names the register-layer target semantics — see
/// [`pipeline::RegisterMode`]. Most callers only need the
/// `RegisteredWorkspace`; `cmd_serve` also consumes the `Workspace` for its
/// watch paths.
fn build_registered_workspace(
    globals: &Globals,
    config: Option<&str>,
    mode: RegisterMode<'_>,
) -> Result<(Workspace, RegisteredWorkspace), CookError> {
    let mut workspace = load_workspace(globals)?;
    // §11.6 / CS-0165 (R3): validate `@PRESET` against the UNION of the named
    // config blocks of every loaded Cookfile — not just the entry Cookfile.
    // This is the one place selection is validated; every command routes here.
    pipeline::validate_selected_config_workspace(&workspace, config)
        .map_err(pipeline_error_to_cook_error)?;
    // §10.2 step 2: re-classify $<NAME> against the full register-phase
    // recipe set before the register pass runs bodies.
    pipeline::codegen_with_module_recipes(&mut workspace, config, &globals.set)
        .map_err(pipeline_error_to_cook_error)?;
    let registered = pipeline::register_workspace(
        &workspace,
        config,
        &globals.set,
        mode,
        /*cache_ctx*/ None,
    )
    .map_err(pipeline_error_to_cook_error)?;
    for warning in &registered.warnings { eprintln!("cook: warning: {warning}"); }
    warn_if_invoked_builtin_is_registered(
        registered
            .names
            .iter()
            .map(|name| (name.name.as_str(), &name.kind)),
    );
    Ok((workspace, registered))
}

// ---------------------------------------------------------------------------
// cmd_run
// ---------------------------------------------------------------------------

pub fn cmd_cache_verify(
    globals: &Globals,
    args: &crate::cli::CacheVerifyArgs,
) -> Result<(), CookError> {
    let recipe_name = args.recipe.as_deref().unwrap_or("build");
    let config = args.config.as_deref();
    // Selection is validated once, in `build_registered_workspace`, against
    // the union of all loaded Cookfiles (§11.6 / CS-0165).
    let num_jobs = resolve_num_jobs(globals);
    let targets = vec![recipe_name.to_string()];

    let (_, registered) = build_registered_workspace(
        globals,
        config,
        RegisterMode::Dispatch { name: recipe_name, argv: &[] },
    )?;
    let recipe_infos = pipeline::build_recipe_infos_from_registered(&registered);

    let edges = cook_engine::analyzer::dependency_edges_multi(&recipe_infos, &targets)
        .map_err(|e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                CookError::RecipeNotFound(name)
            }
            other => CookError::Other(other.to_string()),
        })?;
    let reachable: std::collections::BTreeSet<String> = edges.keys().cloned().collect();

    let project_root = resolve_project_root(globals)?;

    let report = cook_engine::verify::verify_cache(
        &project_root, &registered, &edges, &reachable, num_jobs,
    )
    .map_err(CookError::Other)?;

    if args.json {
        print_verify_json(&report);
    } else {
        print_verify_human(&report);
    }
    if report.exit_code() != 0 {
        std::process::exit(report.exit_code());
    }
    Ok(())
}

fn print_verify_human(report: &cook_engine::verify::VerifyReport) {
    use cook_engine::verify::UnitVerdict;
    for u in &report.units {
        let extra = match &u.verdict {
            UnitVerdict::Divergence { detail } | UnitVerdict::Error { detail } => {
                format!(" — {detail}")
            }
            _ => String::new(),
        };
        println!("[{}] {}/{}{}", u.verdict.label(), u.recipe, u.unit, extra);
    }
    println!(
        "verify: {} unit(s), {} divergence(s), {} error(s)",
        report.units.len(),
        report.divergences(),
        report.errors()
    );
}

fn print_verify_json(report: &cook_engine::verify::VerifyReport) {
    use cook_engine::verify::UnitVerdict;
    // Hand-rolled JSON to avoid leaking serde derives into the engine crate.
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut items = Vec::new();
    for u in &report.units {
        let (verdict, detail) = match &u.verdict {
            UnitVerdict::Pass => ("pass", String::new()),
            UnitVerdict::RecordExempt => ("record_exempt", String::new()),
            UnitVerdict::Divergence { detail } => ("divergence", detail.clone()),
            UnitVerdict::Error { detail } => ("error", detail.clone()),
        };
        items.push(format!(
            "{{\"recipe\":\"{}\",\"unit\":\"{}\",\"key\":\"{}\",\"verdict\":\"{}\",\"detail\":\"{}\"}}",
            esc(&u.recipe), esc(&u.unit), esc(&u.key), verdict, esc(&detail)
        ));
    }
    println!("{{\"units\":[{}],\"divergences\":{},\"errors\":{}}}",
        items.join(","), report.divergences(), report.errors());
}

pub fn cmd_run(
    globals: &Globals,
    recipe_name: &str,
    argv: &[String],
    config: Option<&str>,
) -> Result<(), CookError> {
    // Selection is validated once, in `build_registered_workspace`, against
    // the union of all loaded Cookfiles (§11.6 / CS-0165).
    let num_jobs = resolve_num_jobs(globals);
    let targets = vec![recipe_name.to_string()];

    // Register every Cookfile in the workspace (root + imports) up front.
    // The resulting `RegisteredWorkspace` carries every recipe — including
    // Lua-registered ones from `cook.add_unit` / module helpers like
    // `cook_cc.bin` — under their qualified names, with `RecipeUnits` and
    // `dep_edges` already wired. Phase 5 Task 5.3: this replaces the prior
    // per-wave register loop. `cache_ctx` lifting is deferred to a follow-up;
    // for now register sees `None` and the executor builds its own.
    let (_, registered) = build_registered_workspace(
        globals,
        config,
        RegisterMode::Dispatch { name: recipe_name, argv },
    )?;

    // `inferred_deps` / `*_dep_conflicts` are obsolete in the unified-DAG
    // model: cross-recipe edges come from `RecipeUnits.dep_edges` (recorded
    // directly by `cook.dep_output` / `cook.add_unit` during the register
    // pass), and recipe-level coarse deps come from `RegisteredRecipePub.requires`.
    let recipe_infos = pipeline::build_recipe_infos_from_registered(&registered);

    let run_result = run_with_progress(globals, &recipe_infos, &targets, &registered, num_jobs)?;

    // §19.2 (CS-0124): a failing test step fails the run. The engine records
    // test failures as "soft" results (executor.rs — dependents are not
    // cancelled so `cook test` can run the whole suite); the runner still
    // must exit non-zero. Blocked cannot occur on the Ok path today (blocked
    // rows ride EngineError::TaskFailures), but match it for parity with
    // cmd_test's any_failed check.
    let failed_tests = run_result
        .test_results
        .iter()
        .filter(|r| {
            matches!(
                r.outcome,
                cook_engine::TestOutcome::Failed
                    | cook_engine::TestOutcome::Blocked
                    | cook_engine::TestOutcome::TimedOut
            )
        })
        .count();
    if failed_tests > 0 {
        // main.rs suppresses TestFailure's Display (test-runner output
        // design §3.4, see test_reporter/summary.rs) — the per-node
        // FAILED lines are already on screen; print the one-line summary
        // here, after the renderer has released the terminal.
        eprintln!("cook: {failed_tests} failing test step(s)");
        return Err(CookError::TestFailure(format!(
            "{failed_tests} failing test step(s)"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_emit_lua
// ---------------------------------------------------------------------------

pub fn cmd_emit_lua(globals: &Globals) -> Result<(), CookError> {
    let parsed = read_and_parse(globals)?;
    // emit-lua never loads a workspace, so it surfaces the §5.5 warnings
    // itself (every other command prints them via `load_workspace`).
    for w in &parsed.warnings {
        eprintln!("cook: warning: {w}");
    }
    println!("{}", parsed.lua_source);
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_test
// ---------------------------------------------------------------------------

pub fn cmd_test(
    globals: &Globals,
    args: &crate::cli::TestArgs,
) -> Result<(), crate::error::CookError> {
    use cook_engine::TestScope;
    use std::sync::{Arc, Mutex};

    let project_root = resolve_project_root(globals)?;

    // Determine scope from positional `scope` argument.
    //
    // To distinguish a recipe name from a namespace prefix we use the cheap
    // `list_workspace_names` path: it loads each Cookfile, runs register-phase
    // Lua, and returns just the qualified names + kinds — no recipe bodies
    // run, no probe queries fire. This sees Lua-registered recipes (e.g.
    // `cook_cc.bin`) the same way `cook list` does. We tolerate listing
    // failures (fall back to the raw arg as a Recipe scope) so a malformed
    // Cookfile still gets a single, recognisable error from the engine
    // instead of a duplicated diagnostic from the CLI.
    let scope: Option<TestScope> = match args.scope.as_deref() {
        None => None,
        Some(name) => {
            let recipe_names = collect_workspace_recipe_names(globals).unwrap_or_default();
            Some(resolve_test_scope(name, &recipe_names)?)
        }
    };

    // Phase 7 stub for --rerun-failed
    let rerun_failed_set = if args.rerun_failed {
        match crate::test_state::load_failed_set(&project_root) {
            Ok(set) if !set.is_empty() => Some(set),
            Ok(_) => {
                eprintln!("cook: warning: no previously-failed tests recorded");
                eprintln!("cook: hint: run `cook test` first to populate state");
                return Ok(());
            }
            Err(e) => {
                eprintln!("cook: warning: {e}");
                eprintln!("cook: hint: run `cook test` first to populate state");
                return Ok(());
            }
        }
    } else {
        None
    };

    let rerun_patterns: Vec<String> = args.rerun.clone().unwrap_or_default();
    let num_jobs = resolve_num_jobs(globals);

    // ── Register the workspace ────────────────────────────────────────────────
    // Same path as `cmd_run`: build a unified `RegisteredWorkspace` covering
    // every Cookfile (root + imports), then derive recipe_infos. This sees
    // Lua-registered recipes (cook_cc.bin, dynamic chores, …) under their
    // qualified names with `RecipeUnits` and `dep_edges` already wired.
    let (_, registered) = build_registered_workspace(globals, None, RegisterMode::Enumerate)?;
    let recipe_infos = pipeline::build_recipe_infos_from_registered(&registered);

    // Chore names — chores are excluded from `cook test` because they are
    // destructive by design (e.g. `cook clean` deletes build artefacts) and
    // have no test steps. Including them would cause unintended side-effects.
    let chore_names: std::collections::BTreeSet<String> = registered
        .names
        .iter()
        .filter(|n| matches!(n.kind, cook_engine::cook_register::RecipeKind::Chore))
        .map(|n| n.name.clone())
        .collect();

    // A recipe is a test ROOT only if it declares at least one test step.
    // Bare / namespace-scoped `cook test` previously rooted the run at EVERY
    // non-chore recipe, silently executing (and cache-recording) the whole
    // workspace graph under the test reporter — a later `cook <recipe>` then
    // reported `cached` on work the user never saw run. Roots are now
    // test-bearing recipes only; their dependency closures (the builds the
    // tests actually need) are pulled in by dependency_edges_multi below.
    let test_bearing: std::collections::BTreeSet<String> = registered
        .units_by_recipe
        .iter()
        .filter(|(_, ru)| {
            ru.units.iter().any(|u| {
                matches!(u.payload, cook_engine::cook_contracts::WorkPayload::Test { .. })
            })
        })
        .map(|(name, _)| name.clone())
        .collect();

    // ── Determine candidate recipe names from scope ──────────────────────────
    let candidate_recipe_names: Vec<String> = match &scope {
        None => recipe_infos
            .keys()
            .filter(|n| !chore_names.contains(*n))
            .filter(|n| test_bearing.contains(*n))
            .cloned()
            .collect(),
        Some(TestScope::Recipe(name)) => {
            cook_engine::analyzer::dependency_edges(&recipe_infos, name)
                .map_err(|e| match e {
                    cook_engine::analyzer::GraphError::CycleDetected(s) => {
                        crate::error::CookError::Other(format!("dependency cycle involving: {s}"))
                    }
                    cook_engine::analyzer::GraphError::UnknownRecipe(s) => {
                        crate::error::CookError::RecipeNotFound(s)
                    }
                    other => crate::error::CookError::Other(other.to_string()),
                })?
                .keys()
                .filter(|n| !chore_names.contains(*n))
                .cloned()
                .collect()
        }
        Some(TestScope::Namespace(ns)) => {
            let prefix = format!("{ns}.");
            recipe_infos
                .keys()
                .filter(|n| !chore_names.contains(*n))
                .filter(|n| n.starts_with(&prefix) || *n == ns)
                .filter(|n| test_bearing.contains(*n))
                .cloned()
                .collect()
        }
    };

    // ── Recipe-level pre-filter by --filter glob ─────────────────────────────
    // When filter_patterns are present, limit the target recipe set to those
    // whose recipe name could plausibly match the glob. The glob pattern uses
    // `<recipe>:<test_name>` format; we match the recipe portion by checking
    // if any pattern matches `<recipe>:*`. This avoids running unrelated
    // recipes whose build steps may fail hard when we only care about specific
    // tests. Post-execution, we still apply the full per-TestId filter to
    // handle the test_name portion.
    let candidate_recipe_names: Vec<String> = if !args.filter.is_empty() {
        candidate_recipe_names
            .into_iter()
            .filter(|recipe_name| {
                args.filter.iter().any(|pat| {
                    let recipe_pat = if let Some(colon_pos) = pat.find(':') {
                        pat[..colon_pos].to_string()
                    } else {
                        pat.clone()
                    };
                    let wildcard_id_for_recipe = format!("{}:", recipe_name);
                    let full_match = globset::Glob::new(pat)
                        .map(|g| g.compile_matcher().is_match(&wildcard_id_for_recipe))
                        .unwrap_or(false);
                    let recipe_match = globset::Glob::new(&recipe_pat)
                        .map(|g| g.compile_matcher().is_match(recipe_name.as_str()))
                        .unwrap_or(false);
                    full_match || recipe_match
                })
            })
            .collect()
    } else {
        candidate_recipe_names
    };

    let reporter = {
        let mut r = crate::test_reporter::Reporter::new(globals);
        // Fix single- vs multi-namespace label display from the run plan, not
        // from the order events happen to stream in (see seed_run_namespaces).
        r.seed_run_namespaces(&candidate_recipe_names);
        Arc::new(Mutex::new(r))
    };

    // ── Drive the unified-DAG executor ───────────────────────────────────────
    // The `on_event` closure clones the reporter Arc. It MUST be dropped
    // before we reclaim the inner reporter via `Arc::try_unwrap` below; the
    // simplest way to guarantee that is to scope the closure inside the
    // run-or-skip branch so it falls out of scope by the end of the
    // expression.
    //
    // COOK-235: `cook test` publishes artifacts exactly like `cook build`, so
    // it owes the same end-of-run budget check. The `Ok` arm below discards
    // its `RunResult` down to `test_results`, so the publish count has to be
    // captured out of that arm before the value is dropped. It stays 0 on
    // every other path: the skip-the-executor branch ran nothing, and the
    // `TaskFailures` arm carries no `RunResult` — a failed run has no
    // trustworthy publish count.
    let mut published_count: u64 = 0;
    let test_results: Vec<cook_engine::TestResult> = if candidate_recipe_names.is_empty() {
        // Nothing in the candidate set (e.g. `--filter` matched no recipe).
        // Skip the executor and return an empty result. The reporter still
        // gets `finish` called below with an empty slice.
        Vec::new()
    } else {
        let edges = cook_engine::analyzer::dependency_edges_multi(
            &recipe_infos,
            &candidate_recipe_names,
        )
        .map_err(|e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                crate::error::CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                crate::error::CookError::RecipeNotFound(name)
            }
            other => crate::error::CookError::Other(other.to_string()),
        })?;
        let reachable: std::collections::BTreeSet<String> = edges.keys().cloned().collect();

        let reporter_for_cb = reporter.clone();
        let on_event = move |evt: cook_engine::EngineEvent| {
            if let Ok(mut r) = reporter_for_cb.lock() {
                r.on_event(evt);
            }
        };

        // In test mode, a cook-step failure should not short-circuit with
        // EngineError::TaskFailures. The executor's cancel_subtree already
        // pushed Blocked TestResult rows for every downstream test node into
        // `partial_test_results`; carry them through so we return Ok with the
        // Blocked results rather than propagating the error.
        match cook_engine::run::run(
            &project_root,
            &registered,
            &edges,
            &reachable,
            num_jobs,
            &rerun_patterns,
            no_prune_enabled(globals),
            no_publish_enabled(globals),
            on_event,
        ) {
            Ok(r) => {
                published_count = r.published_count;
                r.test_results
            }
            Err(cook_engine::EngineError::TaskFailures {
                partial_test_results,
                ..
            }) => partial_test_results,
            Err(other) => return Err(engine_error_to_cook_error(other)),
        }
    };

    // SAFETY: the `on_event` closure (if any) was moved into run() above and
    // has been dropped by now; no other Arc references remain.
    let mut reporter = Arc::try_unwrap(reporter)
        .unwrap_or_else(|_| panic!("reporter Arc still has other references after run returned"))
        .into_inner()
        .expect("reporter Mutex is poisoned");

    // Post-execution: filter test_results by --filter globs and --rerun-failed set.
    let test_results: Vec<cook_engine::TestResult> = test_results
        .into_iter()
        .filter(|r| {
            let id_matches = if args.filter.is_empty() {
                true
            } else {
                args.filter.iter().any(|pat| {
                    globset::Glob::new(pat)
                        .map(|g| g.compile_matcher().is_match(&r.id.0))
                        .unwrap_or(false)
                })
            };
            let rerun_matches = if let Some(failed_set) = rerun_failed_set.as_ref() {
                failed_set.contains(&r.id)
            } else {
                true
            };
            id_matches && rerun_matches
        })
        .collect();

    // Phase 7 stub: persist last-run state (no-op)
    let _ = crate::test_state::save(&project_root, &test_results);

    // Phase 8: write JSON/JUnit sidecars.
    let _ = crate::test_reporter::write_json_sidecar(
        &project_root,
        args.report_json.as_deref(),
        &test_results,
    );
    if let Some(path) = &args.report_junit {
        let _ = crate::test_reporter::write_junit_sidecar(path, &test_results);
    }

    let any_failed = test_results.iter().any(|r| {
        matches!(
            r.outcome,
            cook_engine::TestOutcome::Failed
                | cook_engine::TestOutcome::Blocked
                | cook_engine::TestOutcome::TimedOut
        )
    });

    reporter.finish(&test_results);

    // COOK-235, after the reporter released the terminal. Deliberately before
    // the failed-tests return: a red test run still published whatever its
    // upstream cook-steps produced, so the store grew either way and the
    // budget applies either way.
    crate::cache_budget::check_budget_after_run(
        &project_root,
        published_count,
        no_auto_gc_enabled(globals),
    );

    if any_failed {
        Err(crate::error::CookError::TestFailure(
            "one or more tests failed".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// Resolve a `cook test <scope>` argument against the known recipe set.
///
/// Resolution order:
///   1. Exact match against a fully-qualified recipe name → `TestScope::Recipe`.
///   2. Otherwise, if any recipe name starts with `<scope>.` → `TestScope::Namespace`.
///   3. Otherwise, return a useful diagnostic that mentions both options and
///      the `--filter` escape hatch.
///
/// The recipe set passed in is the engine's view: dotted, fully-qualified
/// names (e.g. `apps.web.build`). For an empty recipe set (e.g. workspace
/// failed to load) the function still treats the arg as a Recipe so the
/// engine's "unknown recipe" path produces the canonical error.
fn resolve_test_scope(
    name: &str,
    recipe_names: &std::collections::BTreeSet<String>,
) -> Result<cook_engine::TestScope, crate::error::CookError> {
    use cook_engine::TestScope;

    // Empty set → defer to the engine, which has the authoritative diagnostic.
    if recipe_names.is_empty() {
        return Ok(TestScope::Recipe(name.to_string()));
    }

    // 1. Recipe match wins (preserves existing behaviour for `sub.pass` etc.)
    if recipe_names.contains(name) {
        return Ok(TestScope::Recipe(name.to_string()));
    }

    // 2. Namespace match: any recipe under `<name>.`
    let ns_prefix = format!("{name}.");
    if recipe_names.iter().any(|r| r.starts_with(&ns_prefix)) {
        return Ok(TestScope::Namespace(name.to_string()));
    }

    // 3. Neither — produce a diagnostic that explains the two valid forms
    // and points at `--filter` for glob-shaped arguments.
    let mut suggestions: Vec<String> = recipe_names
        .iter()
        .filter(|r| r.starts_with(name) || r.contains(name))
        .take(5)
        .cloned()
        .collect();
    suggestions.sort();
    suggestions.dedup();

    let mut msg = format!(
        "unknown test scope: '{name}'\n\
         hint: scope must be a recipe name (e.g. `cook test apps.web.build`)\n\
         hint: or an import-namespace prefix (e.g. `cook test apps.web`)\n\
         hint: for glob patterns use --filter (e.g. `cook test --filter '{name}.*'`)"
    );
    if !suggestions.is_empty() {
        msg.push_str("\nsimilar recipes:");
        for s in &suggestions {
            msg.push_str("\n  - ");
            msg.push_str(s);
        }
    }
    Err(crate::error::CookError::Other(msg))
}

/// Return the set of fully-qualified recipe names (recipes only, not chores —
/// chores are filtered from `cook test` anyway) using the cheap
/// `list_names` path: register-phase Lua runs but no recipe bodies execute
/// and no probe queries fire.
///
/// Returns `None` when the workspace cannot be loaded so the caller can fall
/// back to deferring to the engine's diagnostic path.
///
/// Lua-registered recipes (e.g. via `cook_cc.bin`) appear here the same way
/// they do in `cook list` — register-phase is enough to materialise their
/// names.
fn collect_workspace_recipe_names(
    globals: &Globals,
) -> Option<std::collections::BTreeSet<String>> {
    let workspace_root =
        pipeline::resolve_workspace_root(&globals.file, globals.root.clone()).ok()?;
    let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set).ok()?;
    let names = pipeline::list_workspace_names(&workspace, /*config*/ None, &globals.set).ok()?;
    Some(
        names
            .into_iter()
            .filter(|r| matches!(r.kind, cook_engine::cook_register::RecipeKind::Recipe))
            .map(|r| r.name)
            .collect(),
    )
}

static INVOKED_BUILTIN: OnceLock<&'static str> = OnceLock::new();

pub fn set_invoked_builtin(name: &'static str) {
    let _ = INVOKED_BUILTIN.set(name);
}

fn warn_if_invoked_builtin_is_registered<'a>(
    names: impl IntoIterator<Item = (&'a str, &'a cook_engine::cook_register::RecipeKind)>,
) {
    let Some(name) = INVOKED_BUILTIN.get().copied() else {
        return;
    };
    if names.into_iter().any(|(candidate, kind)| {
        candidate == name && matches!(kind, cook_engine::cook_register::RecipeKind::Recipe)
    }) {
        eprintln!("cook: notice: a recipe named '{name}' exists; use cook +{name} to build it");
    }
}

pub fn warn_if_builtin_shadows_recipe(globals: &Globals) {
    let workspace_root = match pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
    {
        Ok(root) => root,
        Err(_) => return,
    };
    let workspace = match Workspace::load(&globals.file, &workspace_root, &globals.set) {
        Ok(workspace) => workspace,
        Err(_) => return,
    };
    let Ok(names) = pipeline::list_workspace_names(&workspace, None, &globals.set) else {
        return;
    };
    warn_if_invoked_builtin_is_registered(names.iter().map(|r| (r.name.as_str(), &r.kind)));
}

// ---------------------------------------------------------------------------
// cmd_menu
// ---------------------------------------------------------------------------

/// Print recipe and chore names, one per kind-prefixed line, annotating any
/// recipe a module minted with the origin it declared (CS-0143):
///
/// ```text
///   recipe build
///   recipe web:build         (from cook_pnpm.workspace)
///   recipe cc:config-header  (from cook_cc.config_header)
/// ```
///
/// An annotation is attribution only — it never gates dispatch, so every
/// name listed here runs when typed (Standard §22.6).
///
/// Backed by the cheap `cook_register::list_names` path through the
/// pipeline-layer `list_workspace_names` helper, so Lua-registered recipes
/// (e.g. `cook_cc.bin`) appear in the menu alongside surface `recipe NAME`
/// blocks — no recipe body runs and no probes fire. The previous AST-walk
/// only saw `parsed.cookfile.recipes` / `parsed.cookfile.chores`, missing
/// every dynamically-registered recipe.
pub fn cmd_menu(globals: &Globals) -> Result<(), CookError> {
    let workspace = load_workspace(globals)?;
    let names = pipeline::list_workspace_names(&workspace, /*config*/ None, &globals.set)
        .map_err(pipeline_error_to_cook_error)?;
    warn_if_invoked_builtin_is_registered(names.iter().map(|r| (r.name.as_str(), &r.kind)));

    // Pass 1: render `{name}{suffix}` per entry.
    let rendered: Vec<(String, &cook_engine::cook_register::RegisteredRecipePub)> = names
        .iter()
        .map(|r| {
            let suffix = if r.params.is_empty() {
                String::new()
            } else {
                format!(
                    " {}",
                    r.params
                        .iter()
                        .map(|p| p.display_token())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            };
            (format!("{}{suffix}", r.name), r)
        })
        .collect();

    // Pass 2: the annotation column — the max rendered width across
    // *annotated* entries only. Unannotated entries never contribute, so a
    // long user recipe name cannot push the `(from …)` column out, and a
    // workspace with no annotations renders byte-identically to one from
    // before annotations existed.
    //
    // Derived from `rendered` rather than accumulated during pass 1: this
    // is what guarantees the `width - len` subtraction in pass 3 cannot
    // underflow, since the max is taken over exactly the same strings, and
    // exactly the same `origin.is_some()` subset, that pass 3 pads.
    let annotated_width: Option<usize> = rendered
        .iter()
        .filter(|(_, r)| r.origin.is_some())
        .map(|(name_and_suffix, _)| name_and_suffix.chars().count())
        .max();

    // Pass 3: print.
    for (name_and_suffix, r) in &rendered {
        let label = match r.kind {
            cook_engine::cook_register::RecipeKind::Recipe => "recipe ",
            cook_engine::cook_register::RecipeKind::Chore => "chore  ",
        };
        match (&r.origin, annotated_width) {
            (Some(origin), Some(width)) => {
                // `width` is the max over the annotated set, which includes
                // this entry, so `width >= len` holds by construction.
                let gutter = width - name_and_suffix.chars().count() + 2;
                println!("  {label}{name_and_suffix}{:gutter$}(from {origin})", "");
            }
            // An annotated entry always contributes to `annotated_width`, so
            // `(Some(_), None)` is unreachable; it degrades to the plain line
            // rather than panicking on a listing surface.
            _ => {
                println!("  {label}{name_and_suffix}");
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_init
// ---------------------------------------------------------------------------

pub fn cmd_init() -> Result<(), CookError> {
    let cookfile_path = std::path::Path::new("Cookfile");
    if cookfile_path.exists() {
        return Err(CookError::Other("Cookfile already exists".to_string()));
    }
    // A deliberately minimal starter: one `build` recipe that produces a
    // single cached artifact, plus a `clean` chore. No sample inputs to seed
    // — the build step generates its output from a constant command, so
    // `cook` works in an empty directory and the second run is a pure cache
    // hit. Keep it tiny; users grow it from here.
    std::fs::write(
        cookfile_path,
        r#"# Your first Cook build. `cook` fingerprints every input and caches every
# output, so the second run does zero work until something changes.
#
#   cook            # builds build/hello.txt
#   cook            # cached — 0 work
#   cook clean      # remove build/

recipe build
    cook "build/hello.txt" { echo "built with cook" > $<out> }

chore clean
    rm -rf build
"#,
    )
    .map_err(|e| CookError::Other(format!("failed to write Cookfile: {e}")))?;
    println!("Created Cookfile");

    let gitignore_path = std::path::Path::new(".gitignore");
    let existing = match std::fs::read_to_string(gitignore_path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(CookError::Other(format!(
                "failed to read .gitignore: {e}"
            )));
        }
    };
    match merge_cook_gitignore_section(existing.as_deref()) {
        GitignoreMerge::Unchanged => {
            println!(".gitignore already has Cook entries");
        }
        GitignoreMerge::Created(content) => {
            std::fs::write(gitignore_path, content)
                .map_err(|e| CookError::Other(format!("failed to write .gitignore: {e}")))?;
            println!("Created .gitignore");
        }
        GitignoreMerge::Appended(content) => {
            std::fs::write(gitignore_path, content)
                .map_err(|e| CookError::Other(format!("failed to write .gitignore: {e}")))?;
            println!("Updated .gitignore with Cook entries");
        }
    }
    Ok(())
}

/// Marker line that identifies a Cook-managed `.gitignore` section. Used to
/// keep `cook init` idempotent across re-runs.
const COOK_GITIGNORE_MARKER: &str = "# Cook artifacts (added by cook init)";

/// The Cook-managed `.gitignore` block. Only entries that are unambiguously
/// Cook-specific go here — language/toolchain ignores (target/, node_modules/)
/// are the user's call.
const COOK_GITIGNORE_SECTION: &str = "\
# Cook artifacts (added by cook init)
# .cook/ holds caches and per-project state; cloud.toml is the one tracked
.cook/**
**/.cook/**
!**/.cook/
!**/.cook/cloud.toml
# Project-local luarocks tree populated by `cook modules install`. Pinned by
# cook.lock + the registry, so it's build output, not source. Top-level
# user-authored lua files in cook_modules/ stay tracked.
cook_modules/lib/
";

#[derive(Debug, PartialEq, Eq)]
enum GitignoreMerge {
    Unchanged,
    Created(String),
    Appended(String),
}

/// Pure helper: given the current contents of `.gitignore` (or `None` if
/// missing), decide what the file should look like after `cook init`. The
/// result is a [`GitignoreMerge`] the caller can act on.
fn merge_cook_gitignore_section(existing: Option<&str>) -> GitignoreMerge {
    match existing {
        None => GitignoreMerge::Created(COOK_GITIGNORE_SECTION.to_string()),
        Some(s) if s.contains(COOK_GITIGNORE_MARKER) => GitignoreMerge::Unchanged,
        Some(s) => {
            let mut out = String::with_capacity(s.len() + COOK_GITIGNORE_SECTION.len() + 2);
            out.push_str(s);
            if !s.is_empty() && !s.ends_with('\n') {
                out.push('\n');
            }
            if !s.is_empty() {
                out.push('\n');
            }
            out.push_str(COOK_GITIGNORE_SECTION);
            GitignoreMerge::Appended(out)
        }
    }
}

#[cfg(test)]
#[path = "tests/cmd_init_tests.rs"]
mod cmd_init_tests;

#[cfg(test)]
#[path = "tests/resolve_test_scope_tests.rs"]
mod resolve_test_scope_tests;

// ---------------------------------------------------------------------------
// cmd_serve
// ---------------------------------------------------------------------------

/// Join relative watch-glob patterns onto `dir` (the entry Cookfile's
/// directory), leaving absolute patterns untouched. Under upward discovery
/// (§20.2) cwd may be a subdirectory of the entry dir, and the watcher
/// matches patterns against absolute notify event paths.
fn anchor_globs(globs: Vec<String>, dir: &std::path::Path) -> Vec<String> {
    globs
        .into_iter()
        .map(|g| {
            if std::path::Path::new(&g).is_absolute() {
                g
            } else {
                dir.join(&g).to_string_lossy().into_owned()
            }
        })
        .collect()
}

pub fn cmd_serve(
    globals: &Globals,
    recipe_name: &str,
    config: Option<&str>,
) -> Result<(), CookError> {
    let parsed = read_and_parse(globals)?;
    // Selection is validated once, in `build_registered_workspace`, against
    // the union of all loaded Cookfiles (§11.6 / CS-0165).

    // Check for interactive steps -- not supported under cook serve
    for recipe in &parsed.cookfile.recipes {
        for step in &recipe.steps {
            if let cook_lang::ast::Step::Shell {
                interactive: true,
                line,
                ..
            } = step
            {
                return Err(CookError::Other(format!(
                    "line {}: interactive '@' steps are not supported under 'cook serve'",
                    line
                )));
            }
        }
    }

    // Resolve execution order via engine analyzer for glob collection. The
    // analyzer's `recipe_infos` map now comes from the unified register
    // pass over the full workspace (root + every transitive import), so a
    // root recipe with a cross-Cookfile dependency resolves correctly here
    // too. Glob collection below still walks only the root AST
    // (`parsed.cookfile`) — imports only contribute file paths to watch.
    // `cmd_serve` also needs the loaded `Workspace` (unlike the other
    // commands): the imports' Cookfile paths feed the watcher below.
    let (workspace, serve_registered) =
        build_registered_workspace(globals, config, RegisterMode::Enumerate)?;
    let recipe_infos = pipeline::build_recipe_infos_from_registered(&serve_registered);
    let order =
        cook_engine::analyzer::topological_sort(&recipe_infos, recipe_name).map_err(|e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                CookError::RecipeNotFound(name)
            }
            // Io/Parse cannot be produced by topological_sort (pure graph op).
            e => CookError::Other(e.to_string()),
        })?;

    let globs = CookWatcher::collect_globs_for_recipes(&parsed.cookfile, &order);
    if globs.is_empty() {
        return Err(CookError::Other(
            "nothing to watch: no recipes in the chain have ingredients".to_string(),
        ));
    }

    let cookfile_path = std::fs::canonicalize(&globals.file)
        .map_err(|e| CookError::Other(format!("cannot resolve Cookfile path: {e}")))?;

    let entry_dir = cookfile_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let globs = anchor_globs(globs, &entry_dir);

    let mut cookfile_paths = vec![cookfile_path];

    // Collect all imported Cookfile paths for watching (a Cookfile with no
    // imports has an empty `workspace.imports`, so this loop is a no-op).
    for (_canonical_path, loaded) in &workspace.imports {
        let import_cookfile = loaded.dir.join("Cookfile");
        if let Ok(canonical) = std::fs::canonicalize(&import_cookfile) {
            cookfile_paths.push(canonical);
        }
    }

    let watcher = CookWatcher::new(globs, cookfile_paths);

    eprintln!("cook serve: initial build...");
    let _ = cmd_run(globals, recipe_name, &[], config);

    eprintln!("cook serve: watching for changes...");
    watcher
        .watch(|cookfile_changed| {
            if cookfile_changed {
                eprintln!("cook serve: Cookfile changed, rebuilding...");
            }
            cmd_run(globals, recipe_name, &[], config).map_err(|e| e.to_string())?;
            Ok(())
        })
        .map_err(|e| CookError::Other(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
#[path = "tests/serve_glob_tests.rs"]
mod serve_glob_tests;

/// Split off the namespace prefix from a qualified recipe name.
///
/// `"backend.proto.generate"` → `"backend.proto"`
/// `"build"` → `""`
fn split_recipe_prefix(name: &str) -> &str {
    name.rfind('.').map(|p| &name[..p]).unwrap_or("")
}

// ---------------------------------------------------------------------------
// cmd_affected — list recipes that would be invalidated since --since=<ref>
// ---------------------------------------------------------------------------

pub fn cmd_affected(
    globals: &Globals,
    args: &crate::cli::AffectedArgs,
) -> Result<(), CookError> {
    let since = globals.since.as_deref().ok_or_else(|| {
        CookError::Other("cook affected requires --since=<git-ref>".to_string())
    })?;
    let project_root = resolve_project_root(globals)?;

    let (_, registered) = build_registered_workspace(globals, None, RegisterMode::Introspect)?;
    let recipe_infos = pipeline::build_recipe_infos_from_registered(&registered);

    // All recipe names in the workspace, or only those matching --recipe.
    let all_names: Vec<String> = recipe_infos.keys().cloned().collect();
    let targets: Vec<String> = if let Some(filter) = &args.recipe {
        all_names
            .iter()
            .filter(|n| {
                // Match the bare name after either qualification vocabulary:
                // pnpm-style task names qualify with ':' ("web:build"), import
                // prefixes with '.' ("web.build", or "app.web:build" for an
                // imported pnpm workspace).
                n.as_str() == filter
                    || n.rsplit(':').next() == Some(filter.as_str())
                    || n.rsplit('.').next() == Some(filter.as_str())
            })
            .cloned()
            .collect()
    } else {
        all_names
    };

    if targets.is_empty() {
        if args.json {
            println!(
                "{}",
                serde_json::json!({
                    "changed_files": Vec::<String>::new(),
                    "affected_recipes": Vec::<String>::new(),
                    "since_ref": since,
                })
            );
        }
        return Ok(());
    }

    let edges = cook_engine::analyzer::dependency_edges_multi(&recipe_infos, &targets)
        .map_err(|e| CookError::Other(e.to_string()))?;
    let reachable: BTreeSet<String> = edges.keys().cloned().collect();

    let changed = cook_engine::affected::git::changed_paths(&project_root, since)
        .map_err(|e| CookError::Other(format!("git diff failed: {e}")))?;
    let affected = cook_engine::affected::compute_affected(
        &changed,
        &registered,
        &edges,
        &reachable,
        &project_root,
    );

    if args.json {
        let changed_files: Vec<String> = changed
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let affected_recipes: Vec<String> = affected.iter().cloned().collect();
        println!(
            "{}",
            serde_json::json!({
                "changed_files": changed_files,
                "affected_recipes": affected_recipes,
                "since_ref": since,
            })
        );
    } else {
        for name in &affected {
            println!("{name}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_why — explain the cache key per unit (read-only; runs nothing)
// ---------------------------------------------------------------------------


/// Derive the `(edges, reachable)` pair that `cook run` would consume for
/// `recipe_name`, using the EXACT derivation `run_with_progress` / `cmd_run`
/// rely on: `build_recipe_infos_from_registered` → `dependency_edges_multi`
/// over `targets = [recipe_name]` → `reachable = edges.keys()`. Reusing this
/// derivation verbatim guarantees `cook why` and `cook run` agree on the
/// closure.
fn resolve_reachable_closure(
    registered: &RegisteredWorkspace,
    recipe_name: &str,
) -> Result<(BTreeMap<String, Vec<String>>, BTreeSet<String>), CookError> {
    let recipe_infos = pipeline::build_recipe_infos_from_registered(registered);
    let targets = vec![recipe_name.to_string()];
    let edges = cook_engine::analyzer::dependency_edges_multi(&recipe_infos, &targets).map_err(
        |e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                CookError::RecipeNotFound(name)
            }
            other => CookError::Other(other.to_string()),
        },
    )?;
    let reachable: BTreeSet<String> = edges.keys().cloned().collect();
    Ok((edges, reachable))
}

/// CS-0171: `cook why` — the one read-only transparency query.
///
/// Structure and determinants arrive from two places and are joined here:
/// `cook_graph` supplies the graph and collapses it to `args.level`,
/// `cook_engine::why` supplies each unit's key, tier verdict, and determinant
/// values, and `cook_engine::timings` supplies what any of it was last
/// observed to cost. The join key throughout is `(recipe, cache_key)`.
///
/// The two halves used to be two commands. `cook dag` printed the structure
/// with a private, local-index-only cache check, and `cook why` printed the
/// determinants as an unaggregated, uncapped, edgeless list. Neither could
/// answer the question the other was for.
pub fn cmd_why(globals: &Globals, args: &crate::cli::WhyArgs) -> Result<(), CookError> {
    use std::sync::Arc;

    let recipe_name = args.recipe.as_deref().unwrap_or("build");
    let config = args.config.as_deref();
    let level = parse_level(&args.level)?;
    let format = parse_format(&args.format)?;

    // Selection is validated once, in `build_registered_workspace`, against
    // the union of all loaded Cookfiles (§11.6 / CS-0165).
    //
    // `Dispatch`, not `Enumerate`: §17.1.6 requires this query to register the
    // workspace by the same path a run of the same target would, so that the
    // key it reports is the key a run would compute. `cook dag` used
    // `Enumerate`, which is why its graph was not an explanation of any
    // particular run.
    let (_, registered) = build_registered_workspace(
        globals,
        config,
        RegisterMode::Dispatch { name: recipe_name, argv: &[] },
    )?;

    let (edges, reachable) = resolve_reachable_closure(&registered, recipe_name)?;

    // `cook why` MUST recompute the same cache key K as `cook run` would, so it
    // MUST anchor the cache context at the SAME project_root the executor uses.
    // Both sides now resolve it via `resolve_project_root` (the workspace root,
    // §20.2.3 / CS-0120), so `why` and `run` cannot diverge regardless of which
    // directory inside the workspace the user invoked from (supersedes the
    // earlier cwd-parity rule that anchored both at current_dir()).
    let project_root = resolve_project_root(globals)?;
    let cache_ctx = cook_engine::build_cache_ctx_for_cli(&project_root, globals.no_publish)
        .map_err(engine_error_to_cook_error)?;
    let cache_managers = cook_engine::cache_managers_for_cli(&registered, &reachable);
    let probes_dir = project_root.join(".cook").join("probes");

    let report = cook_engine::why::explain(
        recipe_name,
        &registered,
        &edges,
        &reachable,
        &cache_ctx,
        &cache_managers,
        &probes_dir,
    )
    .map_err(engine_error_to_cook_error)?;

    let timings = cook_engine::timings::Timings::load(&project_root);

    // A `--unit` selector is a determinant query, not a graph query: the caller
    // has already found the unit and wants everything known about it. Answer it
    // directly rather than making them render the whole closure at unit level.
    if let Some(pattern) = &args.unit {
        return render_selected_units(&report, pattern, format, &timings);
    }

    let annotations = annotations_from(&report, &timings);

    let all_units: Vec<(String, cook_engine::cook_contracts::RecipeUnits)> = reachable
        .iter()
        .map(|name| {
            let units = registered
                .units_by_recipe
                .get(name)
                .cloned()
                // A zero-unit meta-target still belongs in the graph as a node,
                // so it gets an empty stub rather than being dropped.
                .unwrap_or_else(|| cook_engine::cook_contracts::RecipeUnits {
                    recipe_name: name.clone(),
                    deps: edges.get(name).cloned().unwrap_or_default(),
                    units: Vec::new(),
                    step_groups: Vec::new(),
                    working_dir: registered
                        .working_dir_by_prefix
                        .get(split_recipe_prefix(name))
                        .cloned()
                        .unwrap_or_else(|| std::path::PathBuf::from(".")),
                    env_vars: std::collections::BTreeMap::new(),
                    terminal_outputs: Vec::new(),
                    dep_edges: Vec::new(),
                    probes: Vec::new(),
                });
            (name.clone(), units)
        })
        .collect();

    // Per-recipe cache managers anchored at each recipe's prefix's working_dir.
    // The graph uses these only for input-file staleness; the cache verdict
    // comes from `report` above.
    let graph_cache_managers: BTreeMap<String, Arc<cook_engine::cook_cache::ThreadSafeCacheManager>> =
        reachable
            .iter()
            .map(|name| {
                let prefix = split_recipe_prefix(name);
                let wd = registered
                    .working_dir_by_prefix
                    .get(prefix)
                    .cloned()
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let cache_dir = wd.join(".cook").join("cache");
                (
                    name.clone(),
                    Arc::new(cook_engine::cook_cache::ThreadSafeCacheManager::new(cache_dir)),
                )
            })
            .collect();

    let dag = cook_graph::build_dag(&cook_graph::DagInputs {
        target: recipe_name,
        all_units: &all_units,
        explicit_edges: &edges,
        cache_managers: &graph_cache_managers,
    });
    let graph = cook_graph::emit::aggregate(&dag, level, args.max_nodes, &annotations)
        .map_err(|e| CookError::Other(e.to_string()))?;

    // §17.1.6.5: the machine surface is ONE document carrying both halves.
    // Emitting only the graph would drop every key and determinant value, and
    // a consumer would be back to running two commands to learn one thing.
    if format == cook_graph::emit::Format::Json {
        let mut doc = cook_graph::emit::json_value(&graph);
        doc["units"] = serde_json::Value::Array(
            report.units.iter().map(|u| why_unit_json(u, &timings)).collect(),
        );
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return Ok(());
    }

    print!("{}", cook_graph::emit::render(&graph, format));

    // At unit level the closure is already node-capped, so the full determinant
    // detail fits underneath the graph. This is what keeps CS-0112's fidelity
    // reachable after the merge: the graph is the index, this is the body.
    if level == cook_graph::emit::Level::Unit && format == cook_graph::emit::Format::Text {
        print!("\n{}", render_why_plain(&report, &timings));
    }
    Ok(())
}

fn parse_level(s: &str) -> Result<cook_graph::emit::Level, CookError> {
    match s {
        "recipe" => Ok(cook_graph::emit::Level::Recipe),
        "group" => Ok(cook_graph::emit::Level::Group),
        "unit" => Ok(cook_graph::emit::Level::Unit),
        other => Err(CookError::Other(format!(
            "unknown --level '{other}'; expected recipe, group, or unit"
        ))),
    }
}

fn parse_format(s: &str) -> Result<cook_graph::emit::Format, CookError> {
    match s {
        "text" => Ok(cook_graph::emit::Format::Text),
        "mermaid" => Ok(cook_graph::emit::Format::Mermaid),
        "dot" => Ok(cook_graph::emit::Format::Dot),
        "json" => Ok(cook_graph::emit::Format::Json),
        other => Err(CookError::Other(format!(
            "unknown --format '{other}'; expected text, mermaid, dot, or json"
        ))),
    }
}

/// Fold the determinant report and the retained timings into the fact map the
/// graph aggregates over.
///
/// "Served" is the union of both tiers, deliberately: a locally-warm unit does
/// not rebuild merely because the shared store has never heard of it, and
/// COOK-276 exists because conflating the two read as "this will rebuild".
fn annotations_from(
    report: &cook_engine::why::WhyReport,
    timings: &cook_engine::timings::Timings,
) -> cook_graph::Annotations {
    let mut a = cook_graph::Annotations::new();
    for u in &report.units {
        a.insert(
            &u.recipe_name,
            &u.cache_key,
            cook_graph::UnitFacts {
                served: u.local_hit || u.shared_present == Some(true),
                observed_ms: timings
                    .get(&u.recipe_name, &u.cache_key)
                    .map(|o| o.elapsed_ms),
                observed_builds_ago: timings
                    .get(&u.recipe_name, &u.cache_key)
                    .map(|o| o.builds_ago)
                    .unwrap_or(0),
            },
        );
    }
    a
}

/// `--unit <pattern>`: full determinants for the units whose recipe name,
/// cache key, or output paths contain `pattern`.
fn render_selected_units(
    report: &cook_engine::why::WhyReport,
    pattern: &str,
    format: cook_graph::emit::Format,
    timings: &cook_engine::timings::Timings,
) -> Result<(), CookError> {
    let matched: Vec<_> = report
        .units
        .iter()
        .filter(|u| {
            u.recipe_name.contains(pattern)
                || u.cache_key.contains(pattern)
                || u.determinants
                    .output_paths
                    .iter()
                    .any(|p| p.contains(pattern))
        })
        .cloned()
        .collect();
    if matched.is_empty() {
        // A selector that matches nothing is a user error worth naming, not an
        // empty report that reads as "nothing to explain".
        return Err(CookError::Other(format!(
            "--unit '{pattern}' matched no unit in {}'s closure; \
             run without --unit to see the units available",
            report.recipe
        )));
    }
    let selected = cook_engine::why::WhyReport {
        recipe: report.recipe.clone(),
        units: matched,
    };
    match format {
        cook_graph::emit::Format::Json => print!("{}", render_why_json(&selected, timings)),
        _ => print!("{}", render_why_plain(&selected, timings)),
    }
    Ok(())
}

fn render_why_plain(
    report: &cook_engine::why::WhyReport,
    timings: &cook_engine::timings::Timings,
) -> String {
    use cook_engine::why::CacheStatus;
    let mut s = String::new();
    s.push_str(&format!("why {}\n", report.recipe));
    for u in &report.units {
        // COOK-276: label both tiers explicitly. A bare `MISS (shared)` on a
        // locally-warm unit reads as "this will rebuild" when it only means
        // "absent from the shared store tier".
        let status = match &u.status {
            CacheStatus::MissingInput { path } => format!("MISS (input '{path}' missing)"),
            CacheStatus::PinnedColdMiss => "MISS (local), MISS (shared) — pinned, fetch-only".to_string(),
            // CS-0173: name the upstream, not the symptom. This unit does not
            // "miss" in any cache sense; it has no key yet to hit or miss with.
            CacheStatus::ForcedByUpstream { producer, .. } => {
                format!("REBUILD (forced by {producer})")
            }
            _ => {
                let local = if u.local_hit { "HIT (local)" } else { "MISS (local)" };
                match u.shared_present {
                    None => local.to_string(),
                    Some(true) => format!("{local}, HIT (shared)"),
                    Some(false) => format!("{local}, MISS (shared)"),
                }
            }
        };
        // CS-0173: print no key for a forced unit. There is no honest number to
        // put here, and printing one would suggest a lookup that never happened.
        let key_field = if u.key_hex.is_empty() {
            "key not computable until then".to_string()
        } else {
            format!("key {}", u.key_hex)
        };
        s.push_str(&format!(
            "\n{} :: {} [{}]  {}\n",
            u.recipe_name, u.cache_key, status, key_field
        ));
        s.push_str(&format!("  command_hash      {:016x}\n", u.determinants.command_hash));
        s.push_str(&format!("  env_contribution  {:016x}\n", u.determinants.env_contribution));
        s.push_str(&format!("  seal_contribution {:016x}\n", u.determinants.seal_contribution));
        if !u.determinants.inputs.is_empty() {
            s.push_str("  inputs:\n");
            for (p, h) in &u.determinants.inputs {
                s.push_str(&format!("    {p}  {h:016x}\n"));
            }
        }
        // CS-0173: shown separately from `inputs` and without a hash, because
        // there is no hash yet — the producing unit has not run.
        if !u.determinants.pending_inputs.is_empty() {
            s.push_str("  inputs (not determined yet):\n");
            for (p, producer) in &u.determinants.pending_inputs {
                s.push_str(&format!("    {p}  pending {producer}\n"));
            }
        }
        if !u.determinants.output_paths.is_empty() {
            s.push_str("  outputs:\n");
            for p in &u.determinants.output_paths {
                s.push_str(&format!("    {p}\n"));
            }
        }
        if !u.determinants.consulted_env.is_empty() {
            s.push_str("  env (consulted):\n");
            for (k, v) in &u.determinants.consulted_env {
                s.push_str(&format!("    {k} = {v}\n"));
            }
        }
        if !u.determinants.sealed_probes.is_empty() {
            s.push_str("  sealed probes:\n");
            for (k, v) in &u.determinants.sealed_probes {
                s.push_str(&format!("    {k} = {v}{}\n", tools_probe_paths(v)));
            }
        }
        // CS-0174: the local tier's answer to the question the shared tier
        // answers with a manifest diff. Printed before it, because a unit that
        // misses locally is asking "what changed since I last ran this" and
        // that is the nearer question.
        if let Some(cause) = &u.local_cause {
            s.push_str(&format!("  local-miss cause: {cause}\n"));
        }
        // CS-0174: history, kept plainly separate from the live verdict above.
        // For a unit that is currently a hit this is the only causal answer
        // available, and it is the one that answers "why did this rebuild
        // overnight when I changed nothing".
        if let Some(obs) = timings.get(&u.recipe_name, &u.cache_key) {
            if let Some(cause) = &obs.cause {
                let ago = match obs.builds_ago {
                    0 => "last build".to_string(),
                    1 => "1 build ago".to_string(),
                    n => format!("{n} builds ago"),
                };
                s.push_str(&format!("  last ran because: {cause} ({ago})\n"));
            }
        }
        match &u.manifest_diff {
            Some(diffs) if diffs.is_empty() => {
                s.push_str(
                    "  shared-miss diff: producer manifest determinants are identical to ours \
                     (artifact not published, or absent from this backend)\n",
                );
            }
            Some(diffs) => {
                s.push_str("  shared-miss diff vs producer manifest:\n");
                for d in diffs {
                    s.push_str(&format!("    {}\n", render_diff(d)));
                }
            }
            None => {
                if matches!(u.status, CacheStatus::SharedMiss | CacheStatus::PinnedColdMiss) {
                    s.push_str(
                        "  shared-miss diff: no producer manifest published for this key\n",
                    );
                }
            }
        }
    }
    s
}

/// Extract a " (cc→/usr/bin/cc, …)" suffix for a tools-probe JSON value
/// ({"NAME":{"hash":...}}). Empty for non-tools probe values.
///
/// CS-0157: the sealed value carries identity only (content hash) — path is
/// location metadata and is resolved FRESH at query time, so the display
/// shows where each tool resolves now rather than where it lived when the
/// value was produced. A tool no longer on PATH annotates as `<not found>`.
fn tools_probe_paths(value: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(value) else {
        return String::new();
    };
    let Some(obj) = v.as_object() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (name, entry) in obj {
        let tool_shaped = entry
            .as_object()
            .is_some_and(|e| e.get("hash").is_some_and(|h| h.is_string()));
        if tool_shaped {
            match cook_engine::why::resolve_tool_path(name) {
                Some(p) => parts.push(format!("{name}→{p}")),
                None => parts.push(format!("{name}→<not found>")),
            }
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  ({})", parts.join(", "))
    }
}

fn render_diff(d: &cook_engine::why::DeterminantDiff) -> String {
    use cook_engine::why::DeterminantDiff::*;
    match d {
        CommandHash { ours, theirs } => {
            format!("command_hash: ours {ours:016x} != producer {theirs:016x}")
        }
        EnvContribution { ours, theirs } => {
            format!("env_contribution: ours {ours:016x} != producer {theirs:016x}")
        }
        SealContribution { ours, theirs } => {
            format!("seal_contribution: ours {ours:016x} != producer {theirs:016x}")
        }
        Input { path, ours, theirs } => {
            format!("input {path}: ours {ours:?} != producer {theirs:?}")
        }
        Env { key, ours, theirs } => format!("env {key}: ours {ours:?} != producer {theirs:?}"),
        Probe { key, ours, theirs } => {
            format!("probe {key}: ours {ours:?} != producer {theirs:?}")
        }
        OutputPaths { ours, theirs } => format!("outputs: ours {ours:?} != producer {theirs:?}"),
    }
}

// Deterministic JSON output (sorted object keys, §17.1.6 informative note)
// relies on `serde_json` being built WITHOUT the `preserve_order` feature, so
// `serde_json::Map` is BTreeMap-backed and serialises keys sorted regardless of
// insertion order. The determinant maps themselves are already `BTreeMap` in the
// engine; this note covers the per-unit object keys assembled here.
fn render_why_json(
    report: &cook_engine::why::WhyReport,
    timings: &cook_engine::timings::Timings,
) -> String {
    let units: Vec<serde_json::Value> =
        report.units.iter().map(|u| why_unit_json(u, timings)).collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "recipe": report.recipe,
        "units": units,
    }))
    .unwrap_or_default()
        + "\n"
}

fn why_unit_json(
    u: &cook_engine::why::WhyUnit,
    timings: &cook_engine::timings::Timings,
) -> serde_json::Value {
    use cook_engine::why::CacheStatus;

    let mut status_obj = serde_json::Map::new();
    let status_str = match &u.status {
        CacheStatus::LocalHit => "local_hit",
        CacheStatus::SharedHit => "shared_hit",
        CacheStatus::SharedMiss => "shared_miss",
        CacheStatus::LocalOnlyMiss => "local_only_miss",
        CacheStatus::PinnedColdMiss => "pinned_cold_miss",
        CacheStatus::MissingInput { path } => {
            status_obj.insert(
                "missing_input_path".to_string(),
                serde_json::Value::String(path.clone()),
            );
            "missing_input"
        }
        // CS-0173: no `key` field is emitted for this status (see below) — the
        // unit's key is not computable, and a consumer must be able to tell that
        // apart from a key that happens to miss.
        CacheStatus::ForcedByUpstream { producer, path } => {
            status_obj.insert(
                "forced_by".to_string(),
                serde_json::Value::String(producer.clone()),
            );
            status_obj.insert(
                "pending_input_path".to_string(),
                serde_json::Value::String(path.clone()),
            );
            "forced_by_upstream"
        }
    };

    let disposition = match u.disposition {
        cook_engine::why::Disposition::Unannotated => "unannotated",
        cook_engine::why::Disposition::Local => "local",
        cook_engine::why::Disposition::Pinned => "pinned",
    };

    let inputs: serde_json::Map<String, serde_json::Value> = u
        .determinants
        .inputs
        .iter()
        .map(|(p, h)| {
            (
                p.clone(),
                serde_json::Value::String(format!("{h:016x}")),
            )
        })
        .collect();

    let consulted_env: serde_json::Map<String, serde_json::Value> = u
        .determinants
        .consulted_env
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();

    let sealed_probes: serde_json::Map<String, serde_json::Value> = u
        .determinants
        .sealed_probes
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();

    let pending_inputs: serde_json::Map<String, serde_json::Value> = u
        .determinants
        .pending_inputs
        .iter()
        .map(|(p, producer)| (p.clone(), serde_json::Value::String(producer.clone())))
        .collect();

    let determinants = serde_json::json!({
        "command_hash": format!("{:016x}", u.determinants.command_hash),
        "env_contribution": format!("{:016x}", u.determinants.env_contribution),
        "seal_contribution": format!("{:016x}", u.determinants.seal_contribution),
        "inputs": serde_json::Value::Object(inputs),
        // CS-0173: path → producing recipe, for inputs whose content is not
        // determined yet. Disjoint from `inputs` by construction.
        "pending_inputs": serde_json::Value::Object(pending_inputs),
        "output_paths": u.determinants.output_paths.clone(),
        "consulted_env": serde_json::Value::Object(consulted_env),
        "sealed_probes": serde_json::Value::Object(sealed_probes),
    });

    let manifest_diff = match &u.manifest_diff {
        None => serde_json::Value::Null,
        Some(diffs) => {
            serde_json::Value::Array(diffs.iter().map(determinant_diff_json).collect())
        }
    };

    let mut obj = serde_json::Map::new();
    obj.insert("recipe".to_string(), serde_json::Value::String(u.recipe_name.clone()));
    obj.insert("cache_key".to_string(), serde_json::Value::String(u.cache_key.clone()));
    // CS-0173: a forced unit has no computable key, and the wire format says so
    // with null rather than an empty string, so a consumer cannot mistake
    // "not computable" for "computed, and it is the empty key".
    obj.insert(
        "key".to_string(),
        if u.key_hex.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(u.key_hex.clone())
        },
    );
    obj.insert("line".to_string(), serde_json::json!(u.line));
    obj.insert("status".to_string(), serde_json::Value::String(status_str.to_string()));
    for (k, v) in status_obj {
        obj.insert(k, v);
    }
    // COOK-276: explicit per-tier answers alongside the legacy single status.
    obj.insert("local_hit".to_string(), serde_json::Value::Bool(u.local_hit));
    obj.insert(
        "shared_present".to_string(),
        match u.shared_present {
            Some(b) => serde_json::Value::Bool(b),
            None => serde_json::Value::Null,
        },
    );
    obj.insert("disposition".to_string(), serde_json::Value::String(disposition.to_string()));
    obj.insert("determinants".to_string(), determinants);
    obj.insert("manifest_diff".to_string(), manifest_diff);
    // CS-0174: local-tier attribution, the counterpart of `manifest_diff`.
    obj.insert(
        "local_cause".to_string(),
        match &u.local_cause {
            Some(c) => serde_json::Value::String(c.clone()),
            None => serde_json::Value::Null,
        },
    );
    // CS-0174: history, and labelled as history. `last_cause` says why the unit
    // ran on a past build; `local_cause` says why it will run now. A consumer
    // must not read one for the other, so they are separate keys and the age of
    // the observation rides alongside.
    let last = timings.get(&u.recipe_name, &u.cache_key);
    obj.insert(
        "last_cause".to_string(),
        match last.and_then(|o| o.cause.as_ref()) {
            Some(c) => serde_json::Value::String(c.clone()),
            None => serde_json::Value::Null,
        },
    );
    obj.insert(
        "last_cause_builds_ago".to_string(),
        match last.filter(|o| o.cause.is_some()) {
            Some(o) => serde_json::json!(o.builds_ago),
            None => serde_json::Value::Null,
        },
    );
    serde_json::Value::Object(obj)
}

fn determinant_diff_json(d: &cook_engine::why::DeterminantDiff) -> serde_json::Value {
    use cook_engine::why::DeterminantDiff::*;
    let hexopt = |o: &Option<u64>| match o {
        Some(h) => serde_json::Value::String(format!("{h:016x}")),
        None => serde_json::Value::Null,
    };
    let stropt = |o: &Option<String>| match o {
        Some(s) => serde_json::Value::String(s.clone()),
        None => serde_json::Value::Null,
    };
    match d {
        CommandHash { ours, theirs } => serde_json::json!({
            "determinant": "command_hash",
            "ours": format!("{ours:016x}"),
            "producer": format!("{theirs:016x}"),
        }),
        EnvContribution { ours, theirs } => serde_json::json!({
            "determinant": "env_contribution",
            "ours": format!("{ours:016x}"),
            "producer": format!("{theirs:016x}"),
        }),
        SealContribution { ours, theirs } => serde_json::json!({
            "determinant": "seal_contribution",
            "ours": format!("{ours:016x}"),
            "producer": format!("{theirs:016x}"),
        }),
        Input { path, ours, theirs } => serde_json::json!({
            "determinant": format!("input:{path}"),
            "ours": hexopt(ours),
            "producer": hexopt(theirs),
        }),
        Env { key, ours, theirs } => serde_json::json!({
            "determinant": format!("env:{key}"),
            "ours": stropt(ours),
            "producer": stropt(theirs),
        }),
        Probe { key, ours, theirs } => serde_json::json!({
            "determinant": format!("probe:{key}"),
            "ours": stropt(ours),
            "producer": stropt(theirs),
        }),
        OutputPaths { ours, theirs } => serde_json::json!({
            "determinant": "output_paths",
            "ours": ours.clone(),
            "producer": theirs.clone(),
        }),
    }
}

/// `cook cache dump <recipe>` — print a recipe's cache index as readable TOML
/// (CS-0166).
///
/// The v7 index is binary, so this is the on-demand readable view that
/// replaces opening the file. Purely a reader: it resolves the workspace root,
/// loads the index, and prints. It deliberately does NOT register the
/// workspace — dumping an index must work even when the Cookfile no longer
/// parses, which is exactly when someone is inspecting cache state by hand.
pub fn cmd_cache_dump(
    globals: &Globals,
    args: &crate::cli::CacheDumpArgs,
) -> Result<(), CookError> {
    let recipe_name = args.recipe.as_deref().unwrap_or("build");
    let project_root = resolve_project_root(globals)?;
    let cache_dir = project_root.join(".cook").join("cache");

    match cook_engine::cook_cache::RecipeCache::load(&cache_dir, recipe_name) {
        Some(cache) => {
            print!("{}", cache.to_readable_toml());
            Ok(())
        }
        None => Err(CookError::Other(format!(
            "no readable cache index for recipe '{recipe_name}' in {}\n\
             (the recipe may never have run, or its index predates the current \
             cache version and was swept)",
            cache_dir.display()
        ))),
    }
}

#[cfg(test)]
#[path = "tests/pipeline_tests.rs"]
mod tests;
