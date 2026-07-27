//! DAG execution loop.
//!
//! Executes all nodes in a `Dag<WorkNode>` respecting dependency order.
//! Pre-satisfied (cached) nodes are completed immediately. Real work nodes
//! are dispatched to the `cook_luaotp::WorkerPool`. Interactive nodes are
//! queued and run on the main thread after the pool drains.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cook_cache::{CacheContext, TestCache, TestCacheEntry, TestCacheOutcome, ThreadSafeCacheManager};
use cook_contracts::{CapturedStream, CommandFailure, WorkPayload};
use cook_fingerprint::backend::DeterminantManifest;
use cook_fingerprint::{
    artifact_key, cloud_key, needs_rebuild_cook, recipe_namespace, ArtifactMeta, CloudKeyInputs,
    RebuildResult, RestoreCtx, CACHE_VERSION,
};
use cook_dag::Dag;
use cook_luaotp::{WorkItem, WorkerPool};

use crate::{EngineError, EngineEvent, NodeKind, RecipeKind, WorkNode};

/// COOK-165: expose the cache schema version to the read-only `why::explain`
/// walk so its recomputed cloud_key matches the executor's.
pub(crate) fn cache_version() -> u32 {
    CACHE_VERSION
}

// ---------------------------------------------------------------------------
// RecipeTracker
// ---------------------------------------------------------------------------
//
// Per-recipe accumulator driving unit-driven `RecipeStarted` / `RecipeCompleted`
// / `RecipeFailed` events. The executor seeds one tracker per recipe with at
// least one unit in the DAG (zero-unit meta-targets are handled by synthetic
// emission in `run.rs`). `ensure_recipe_started` fires `RecipeStarted` on the
// first unit's transition out of Waiting and stamps `start` then;
// `finish_recipe_node` fires `RecipeCompleted` / `RecipeFailed` on the last
// unit's completion using `start.elapsed()`. There is no wave-aligned firing.

struct RecipeTracker {
    /// Stamped by `ensure_recipe_started` when the first unit transitions
    /// out of Waiting. `RecipeCompleted.elapsed` is `start.elapsed()` at
    /// the last unit's completion.
    start: Instant,
    total_nodes: usize,
    completed_nodes: usize,
    cached_nodes: usize,
    skipped_nodes: usize,
    has_failure: bool,
    /// True once `RecipeStarted` has been emitted for this recipe.
    started: bool,
    /// CS-0051: marked true when any chore-window step is observed for
    /// this recipe so `RecipeCompleted` can carry `kind: RecipeKind::Chore`.
    is_chore: bool,
}

// ---------------------------------------------------------------------------
// emit helper
// ---------------------------------------------------------------------------

fn emit(tx: &Option<mpsc::Sender<EngineEvent>>, event: EngineEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}

// ---------------------------------------------------------------------------
// node_kind_for_payload — classify a captured work payload for the renderer
// ---------------------------------------------------------------------------
//
// Today only test-step bodies get a non-default kind (`NodeKind::Test` →
// rendered as green "Tested"). All other shell/cook/lua payloads fall
// through to `Cooked`. The Lua stdlib (`cpp.lib`, `cpp.bin`,
// `cpp.compile_commands`) will widen this in a follow-up plan to emit
// Compile/Link/Generate/etc. for individual sub-units, at which point the
// classifier may need access to richer metadata than `WorkPayload` carries.

fn node_kind_for_payload(payload: &WorkPayload) -> NodeKind {
    match payload {
        WorkPayload::Test { .. } => NodeKind::Test,
        WorkPayload::Shell { .. }
        | WorkPayload::Interactive { .. }
        | WorkPayload::LuaChunk { .. } => NodeKind::Cooked,
        // CS-0049: `WorkPayload` is `#[non_exhaustive]`. Future variants
        // default to `Cooked` until they get a dedicated mapping.
        _ => NodeKind::Cooked,
    }
}

/// CS-0171: the recipe-local cache key to stamp on this node's progress
/// records, or `None` for a node with no cache metadata (a bare shell step, a
/// chore body). This is the identity `cook why` joins retained timings on;
/// see `EngineEvent::NodeStarted::cache_key` for why neither the per-run unit
/// index nor the display name can serve.
fn node_cache_key(node: &WorkNode) -> Option<String> {
    node.cache_meta.as_ref().map(|m| m.cache_key.clone())
}

// ---------------------------------------------------------------------------
// is_chore_window_member — admit a node into the chore-window drain
// ---------------------------------------------------------------------------
//
// A chore body's imperative region may mix shell steps (interactive shell
// drain) and Lua-bundle steps (execute-phase Lua coalesced through
// `emit_chore_body_unit`). Both must share the single drain window so the
// CS-0051 "one drain per chore body" property holds when the body is
// shell-only, Lua-only, or any mix. The dispatch site in `process_ready`
// pushes both onto the interactive_queue when `is_chore = true`; the
// chore-window code below uses this helper to identify members during the
// pre-walk, head detection, and execution loop.
fn is_chore_window_member(payload: &Option<WorkPayload>) -> bool {
    matches!(
        payload,
        Some(WorkPayload::Interactive { is_chore: true, .. })
            | Some(WorkPayload::LuaChunk { is_chore: true, .. })
    )
}

// ---------------------------------------------------------------------------
// recipe_node_counts — count how many nodes belong to each recipe
// ---------------------------------------------------------------------------

fn recipe_node_counts(dag: &Dag<WorkNode>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for i in 0..dag.len() {
        let name = &dag.node(i).payload().recipe_name;
        *counts.entry(name.clone()).or_insert(0) += 1;
    }
    counts
}

// ---------------------------------------------------------------------------
// ensure_output_parent_dirs — CS-0050
//
// Before a `cook` step's shell text runs, the engine creates the parent
// directory of every declared output path so authors no longer need
// `mkdir -p` boilerplate in their recipes. The Standard pins this in
// §{exec.output-materialisation}; the gate is a non-empty
// `cache_meta.output_paths`, which is set only for cook steps (plate /
// test set `cache_meta = None`).
//
// Output paths are recorded relative to the unit's working directory and
// resolved against it the same way the cache fingerprint does. The call
// is idempotent (`create_dir_all` returns `Ok(())` if the directory
// already exists) and concurrency-safe under POSIX (multiple step groups
// sharing a parent dir all succeed). When the parent already exists as a
// non-directory the helper returns an error with a CS-0050-tagged
// diagnostic naming the output path and the offending parent.
// ---------------------------------------------------------------------------

fn ensure_output_parent_dirs(work_node: &WorkNode) -> Result<(), String> {
    let meta = match &work_node.cache_meta {
        Some(m) => m,
        None => return Ok(()),
    };
    if meta.output_paths.is_empty() {
        return Ok(());
    }
    for output_path in &meta.output_paths {
        let abs = work_node.working_dir.join(output_path);
        let parent = match abs.parent() {
            // No parent component — the path is a root or empty; nothing to
            // create. (`create_dir_all("")` is a no-op on POSIX but we
            // short-circuit explicitly so the diagnostic only ever names
            // real parent paths.)
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => continue,
        };
        if parent.exists() && !parent.is_dir() {
            return Err(format!(
                "CS-0050: cannot create parent directory of declared output \
                 `{}`: a non-directory already exists at `{}`",
                output_path,
                parent.display()
            ));
        }
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Err(format!(
                "CS-0050: failed to create parent directory `{}` of declared \
                 output `{}`: {}",
                parent.display(),
                output_path,
                e
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// normalize_glob_pattern — CS-0085 trailing-** normalisation
//
// The reference glob crate (`glob = "0.3"`) treats a trailing `**` segment
// as matching DIRECTORIES ONLY, not files. So `build/**` resolves to
// `{build/sub/}` (a subdirectory), and the CS-0064 directory-filter then
// drops it — net result is an empty set. The canonical user-facing pattern
// (`.next/**`, Turborepo/bash-globstar convention) would silently match
// nothing without this normalisation.
//
// Rule (§22.1.2): a pattern whose last path segment is exactly `**` is
// rewritten to append `/*`, producing `<prefix>/**/*`. The bare pattern
// `**` becomes `**/*`. Patterns whose `**` is not the final segment
// (`**/lib/*.so`) are left unchanged.
//
// Returns `Cow::Borrowed` when no rewrite is needed (common case) to avoid
// allocating for patterns like `*.c` or `src/**/*.c`.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// resolve_output_paths — CS-0085 §17.6 glob expansion
//
// Expands glob patterns in `declared` output paths against `working_dir`.
// Literal entries (no `*`, `?`, `[`) pass through unchanged. The returned
// Vec preserves first-occurrence order; a path that matches multiple glob
// entries or appears as both a literal and a glob match is included exactly
// once (§17.6 item 1 deduplication rule).
//
// Glob patterns are normalised via `normalize_glob_pattern` before
// resolution; see that function's documentation for the trailing-`**` rule.
// ---------------------------------------------------------------------------

pub(crate) fn resolve_output_paths(
    declared: &[String],
    working_dir: &std::path::Path,
) -> Vec<String> {
    resolve_output_paths_with_unmatched(declared, working_dir).paths
}

pub(crate) struct ResolvedOutputPaths {
    pub(crate) paths: Vec<String>,
    pub(crate) unmatched_patterns: Vec<String>,
}

pub(crate) fn resolve_output_paths_with_unmatched(
    declared: &[String],
    working_dir: &std::path::Path,
) -> ResolvedOutputPaths {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(declared.len());
    let mut unmatched_patterns = Vec::new();
    for entry in declared {
        if cook_fingerprint::is_terminal_output(entry) {
            let normalized = cook_fingerprint::normalize_glob_pattern(entry);
            let resolved_paths = cook_fingerprint::resolve_glob(working_dir, normalized.as_ref());
            if resolved_paths.is_empty() && cook_fingerprint::has_glob_meta(entry) {
                unmatched_patterns.push(entry.clone());
            }
            for resolved in resolved_paths {
                if seen.insert(resolved.clone()) {
                    out.push(resolved);
                }
            }
        } else if seen.insert(entry.clone()) {
            out.push(entry.clone());
        }
    }
    ResolvedOutputPaths {
        paths: out,
        unmatched_patterns,
    }
}

// ---------------------------------------------------------------------------
// run_interactive_on_main
// ---------------------------------------------------------------------------

fn run_interactive_on_main(
    cmd: &str,
    line: usize,
    working_dir: &std::path::Path,
    env_vars: &BTreeMap<String, String>,
) -> Result<(), String> {
    let mut child_env: std::collections::HashMap<String, String> = std::env::vars().collect();
    for (k, v) in env_vars {
        child_env.insert(k.clone(), v.clone());
    }

    // COOK-306: an executed command may write anywhere in the tree.
    cook_fingerprint::statmemo::disarm();
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(working_dir)
        .envs(&child_env)
        .status()
        .map_err(|e| format!("failed to execute: {e}"))?;

    if !status.success() {
        let code = status.code().unwrap_or(1);
        return Err(CommandFailure::new(
            line,
            code,
            cmd,
            CapturedStream::from_bytes(&[]),
            CapturedStream::from_bytes(&[]),
        )
        .to_wire());
    }

    Ok(())
}

fn progress_error(error: &str) -> String {
    CommandFailure::from_wire(error).map_or_else(
        || error.to_owned(),
        |failure| {
            format!(
                "command at line {} exited with code {}: {}",
                failure.line(),
                failure.exit_code(),
                failure.command()
            )
        },
    )
}

// ---------------------------------------------------------------------------
// iso8601_now — minimal RFC-3339 timestamp without chrono / time deps
// ---------------------------------------------------------------------------

/// Match a test identity string (`<recipe>:<name>`) against a list of
/// `--rerun PATTERN` globs. Returns `true` if any pattern matches; `false`
/// otherwise (including the empty-list case — no rerun patterns means no
/// force-rerun).
fn rerun_matches(test_id: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    patterns.iter().any(|pat| {
        match globset::Glob::new(pat) {
            Ok(g) => g.compile_matcher().is_match(test_id),
            Err(_) => false,
        }
    })
}

fn iso8601_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Convert unix timestamp to YYYY-MM-DDTHH:MM:SSZ.
    // Best-effort formatter — accuracy to ±1 second is sufficient
    // for the `recorded_at` diagnostic field in TestCacheEntry.
    let secs_in_day: u64 = 86400;
    let secs_in_hour: u64 = 3600;
    let secs_in_min: u64 = 60;
    let h = (secs % secs_in_day) / secs_in_hour;
    let m = (secs % secs_in_hour) / secs_in_min;
    let sec = secs % secs_in_min;

    // Simplified date computation from days since 1970-01-01.
    let days = secs / secs_in_day;
    let mut y = 1970u64;
    let mut d = days;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366u64 } else { 365u64 };
        if d < days_in_year { break; }
        d -= days_in_year;
        y += 1;
    }
    let year = y;
    let day_of_year = d;

    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days: [u64; 12] = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut rem = day_of_year;
    let mut month = 1u64;
    for &md in &month_days {
        if rem < md { break; }
        rem -= md;
        month += 1;
    }
    let day = rem + 1;
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{sec:02}Z")
}

// ---------------------------------------------------------------------------
// execute_dag
// ---------------------------------------------------------------------------

/// Execute all nodes in `dag` respecting dependency order.
///
/// Pre-satisfied (cached) nodes are completed immediately without submitting
/// work. Real work nodes are dispatched to a thread pool of `num_workers`
/// workers. Interactive nodes are queued and run on the main thread after the
/// pool drains. If any node fails, all transitive dependents are cancelled.
///
/// Returns `Ok(test_results)` with all test results from this DAG if every node
/// completed successfully (or was pre-satisfied), or `Err(EngineError)` listing
/// each failed node.
///
/// `test_cache` — when `Some`, test nodes check the content-addressed cache
/// before dispatch. Hits emit synthesized `TestPassed { cached: true }` events
/// and do not submit to the worker pool. Passing executions write cache entries.
///
/// Test fingerprints are computed lazily at ready time via
/// [`crate::run::compute_ready_test_fingerprint`] — the point where a test's
/// consumed dependency outputs are materialised on disk and can be
/// content-hashed for early cutoff (COOK-211, §17.4). A source-less test
/// yields `None` there and always runs.
///
/// `probe_units_by_node` — maps dag node id → `ProbeUnit` metadata (declared
/// inputs for fingerprinting). Only nodes whose `WorkPayload` is
/// `WorkPayload::Probe` need entries. When the map is empty or has no entry
/// for a given probe node, probe caching is skipped for that node (the probe
/// always executes). Populated by the call site in `run.rs` from
/// `RecipeUnits.probes` cross-referenced by key.
///
/// `dep_outputs` — read-only terminal-outputs snapshot threaded into each
/// worker VM so execute-phase `cook.dep_output` / `dep_output_list` resolve
/// (§24.7).
///
/// `published` — counter the caller owns and reads back once this returns.
/// Every CAS publish site below bumps it by one. It exists purely as a zero /
/// non-zero gate for the caller's end-of-run store-budget check: a run that
/// published no outputs skips walking the shared store entirely. Deliberately
/// coarse — it counts publish *operations*, not objects or bytes — because
/// nothing consumes the magnitude.
///
/// It counts what *this* function publishes. The register-phase probe pre-pass
/// runs before `execute_dag` and writes probe values to the CAS without
/// consulting `publish_enabled` or this counter (COOK-339), so "counter is 0"
/// means "this run published no outputs", not "the store is byte-identical".
/// See `RunResult::published_count`.
pub fn execute_dag(
    dag: Dag<WorkNode>,
    num_workers: usize,
    cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    event_tx: Option<mpsc::Sender<EngineEvent>>,
    cache_ctx: Arc<CacheContext>,
    test_cache: Option<&TestCache>,
    rerun_patterns: &[String],
    probe_units_by_node: &BTreeMap<usize, cook_contracts::ProbeUnit>,
    dep_outputs: cook_luaotp::WorkerDepOutputs,
    published: &AtomicU64,
) -> Result<Vec<crate::TestResult>, EngineError> {
    // Empty DAG — nothing to do.
    if dag.is_empty() {
        return Ok(Vec::new());
    }

    // Defensive cycle check before spawning any workers. The work-DAG
    // builder cannot introduce cycles by construction (deps only point to
    // already-emitted ids), but if a future builder change does, fail
    // fast with a path-bearing diagnostic instead of deadlocking the pool.
    if let Err(cycle) = dag.validate() {
        return Err(EngineError::CycleDetected(cycle.to_string()));
    }

    // COOK-306: arm the per-run mtime memo. Registration (and every probe
    // capture it ran) is complete by now, so nothing has written to the tree
    // since the last stat; the first write from here on disarms it. See
    // `cook_fingerprint::statmemo`.
    cook_fingerprint::statmemo::arm();

    let total = dag.len();
    let (pool, rx) = WorkerPool::spawn_with_dep_outputs(num_workers, dep_outputs);

    // CS-0102: the per-run store reads through to the canonical probe files.
    pool.probe_value_store()
        .attach_dir(cache_ctx.project_root.join(".cook").join("probes"));

    let mut cancelled = vec![false; total];
    let mut pending: usize = 0; // how many work results we're waiting for
    let mut failures: Vec<(usize, String, String)> = Vec::new();
    let mut test_results: Vec<crate::TestResult> = Vec::new();

    // G4/G5 (CS-0074): probe fingerprint state.
    //
    // `upstream_probe_fingerprints`: populated as each probe completes (in
    // topological order via DAG edges), so that subsequent probe fingerprints
    // can include upstream fingerprints in their hash (§22.5.3 §7).
    //
    // `probe_fingerprint_by_node`: the fingerprint computed at dispatch time
    // (G4) is stored here so the completion handler (G5) can reuse it without
    // recomputation. Keyed by dag node id.
    let mut upstream_probe_fingerprints: BTreeMap<String, [u8; 32]> = BTreeMap::new();
    let mut probe_fingerprint_by_node: BTreeMap<usize, [u8; 32]> = BTreeMap::new();
    // CS-0178 (COOK-343): keys of probes that have NO cache key — those
    // declaring no `env`/`tools`/`files`/`requires`, plus any probe whose
    // `requires` closure reaches one. Their fingerprint is still computed and
    // recorded (downstream folding and `cook why` both read it) but it is
    // never used to GET or PUT: a keyless probe re-produces on every
    // invocation in which it is reached.
    let mut keyless_probes: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    // Collects TestResult entries synthesized from test-cache hits in process_ready.
    let mut cached_test_results: Vec<crate::TestResult> = Vec::new();
    // Collects Blocked TestResult rows synthesized by cancel_subtree when a
    // cook step fails and its downstream test nodes are cancelled. These are
    // included in TaskFailures.partial_test_results so run_for_test_inner can
    // return Ok with Blocked rows instead of propagating the error.
    let mut blocked_results: Vec<crate::TestResult> = Vec::new();

    // ----- Recipe tracking -----
    let mut recipe_trackers: BTreeMap<String, RecipeTracker> = BTreeMap::new();
    for (name, count) in recipe_node_counts(&dag) {
        recipe_trackers.insert(
            name,
            RecipeTracker {
                start: Instant::now(),
                total_nodes: count,
                completed_nodes: 0,
                cached_nodes: 0,
                skipped_nodes: 0,
                has_failure: false,
                started: false,
                is_chore: false,
            },
        );
    }

    // Helper: ensure a recipe is marked as started, emitting RecipeStarted if needed.
    fn ensure_recipe_started(
        trackers: &mut BTreeMap<String, RecipeTracker>,
        recipe_name: &str,
        event_tx: &Option<mpsc::Sender<EngineEvent>>,
    ) {
        if let Some(tracker) = trackers.get_mut(recipe_name) {
            if !tracker.started {
                tracker.started = true;
                tracker.start = Instant::now();
                emit(
                    event_tx,
                    EngineEvent::RecipeStarted {
                        name: recipe_name.to_string(),
                        total_nodes: tracker.total_nodes,
                    },
                );
            }
        }
    }

    // Helper: mark a recipe node as completed and emit recipe-level events if done.
    fn finish_recipe_node(
        trackers: &mut BTreeMap<String, RecipeTracker>,
        recipe_name: &str,
        is_cached: bool,
        is_failure: bool,
        event_tx: &Option<mpsc::Sender<EngineEvent>>,
    ) {
        finish_recipe_node_inner(trackers, recipe_name, is_cached, is_failure, false, event_tx);
    }

    fn finish_recipe_node_inner(
        trackers: &mut BTreeMap<String, RecipeTracker>,
        recipe_name: &str,
        is_cached: bool,
        is_failure: bool,
        is_skipped: bool,
        event_tx: &Option<mpsc::Sender<EngineEvent>>,
    ) {
        if let Some(tracker) = trackers.get_mut(recipe_name) {
            tracker.completed_nodes += 1;
            if is_cached {
                tracker.cached_nodes += 1;
            }
            if is_skipped {
                tracker.skipped_nodes += 1;
            }
            if is_failure {
                tracker.has_failure = true;
            }

            if tracker.completed_nodes == tracker.total_nodes {
                let elapsed = tracker.start.elapsed();
                if tracker.has_failure {
                    emit(
                        event_tx,
                        EngineEvent::RecipeFailed {
                            name: recipe_name.to_string(),
                            elapsed,
                            completed_nodes: tracker.completed_nodes - 1,
                            total_nodes: tracker.total_nodes,
                        },
                    );
                } else if tracker.skipped_nodes > 0 {
                    emit(
                        event_tx,
                        EngineEvent::RecipeSkipped {
                            name: recipe_name.to_string(),
                            elapsed,
                            skipped_nodes: tracker.skipped_nodes,
                            completed_nodes: tracker.completed_nodes - tracker.skipped_nodes,
                            total_nodes: tracker.total_nodes,
                        },
                    );
                } else {
                    let kind = if tracker.is_chore {
                        RecipeKind::Chore
                    } else {
                        RecipeKind::Recipe
                    };
                    emit(
                        event_tx,
                        EngineEvent::RecipeCompleted {
                            name: recipe_name.to_string(),
                            elapsed,
                            cached_nodes: tracker.cached_nodes,
                            total_nodes: tracker.total_nodes,
                            kind,
                        },
                    );
                }
            }
        }
    }

    // ----- helper: cancel a node and all its transitive dependents -----
    //
    // `upstream_name` is the name of the failing node that triggered this
    // cancellation — used to populate TestBlocked.upstream for test-step nodes.
    //
    // `blocked_results` accumulates a TestResult { outcome: Blocked } for every
    // cancelled test node so that run_for_test_inner can report them even when
    // execute_dag returns Err(TaskFailures) due to cook-step failures.
    fn cancel_subtree(
        dag: &Dag<WorkNode>,
        node_id: usize,
        cancelled: &mut Vec<bool>,
        event_tx: &Option<mpsc::Sender<EngineEvent>>,
        trackers: &mut BTreeMap<String, RecipeTracker>,
        upstream_name: &str,
        blocked_results: &mut Vec<crate::TestResult>,
    ) {
        if cancelled[node_id] {
            return;
        }
        cancelled[node_id] = true;
        let node = dag.node(node_id);
        let work_node = node.payload();
        let node_name = work_node
            .payload
            .as_ref()
            .map(|p| p.display_name())
            .unwrap_or_else(|| work_node.recipe_name.clone());
        emit(
            event_tx,
            EngineEvent::NodeSkipped {
                recipe: work_node.recipe_name.clone(),
                node_name: node_name.clone(),
            },
        );
        // Emit TestBlocked and synthesize a Blocked TestResult for test-step nodes.
        if let Some(WorkPayload::Test { test_name, should_fail, line, iteration_item, .. }) = &work_node.payload {
            let id_str = match iteration_item {
                Some(item) if !item.is_empty() => format!("{}:{}[{}]", work_node.recipe_name, test_name, item),
                _ => format!("{}:{}", work_node.recipe_name, test_name),
            };
            let test_id = crate::id::parse_test_id(&id_str);
            emit(
                event_tx,
                EngineEvent::TestBlocked {
                    id: test_id.clone(),
                    upstream: upstream_name.to_string(),
                    line: *line as u32,
                },
            );
            let namespace = crate::id::id_namespace(&test_id);
            let recipe = crate::id::id_recipe(&test_id);
            blocked_results.push(crate::TestResult {
                id: test_id,
                namespace,
                recipe,
                name: test_name.clone(),
                suite: work_node.recipe_name.clone(),
                iteration_item: iteration_item.clone(),
                outcome: crate::TestOutcome::Blocked,
                duration: std::time::Duration::ZERO,
                from_cache: false,
                stdout: String::new(),
                stderr: String::new(),
                fingerprint: None,
                blocked_by: Some(upstream_name.to_string()),
                should_fail: *should_fail,
                timed_out: false,
                line: *line as u32,
                exit_code: None,
            });
        }
        finish_recipe_node_inner(
            trackers,
            &work_node.recipe_name,
            false,
            false,
            true,
            event_tx,
        );
        for &dep_id in dag.node(node_id).dependents() {
            cancel_subtree(dag, dep_id, cancelled, event_tx, trackers, &node_name, blocked_results);
        }
    }

    /// Outcome of the per-node cache check (COOK-162).
    enum CacheDecision {
        /// Served from cache (local hit, drift-restore, or cold fetch-by-key). Skip execution.
        Hit,
        /// Not in cache; dispatch the unit normally. Carries the warm re-run
        /// attribution (COOK-276): which determinant changed since the unit's
        /// previous record, rendered as a one-line cause fragment — `None` for
        /// a genuinely cold unit (no history to diff against).
        Miss(Option<String>),
        /// `pinned` unit absent from both the local index and the shared store. MUST
        /// NOT be rebuilt — the caller raises a hard failure.
        PinnedColdMiss,
    }

    // ----- helper: check cache for a work node -----
    // Returns true if the node can be skipped (cache hit). When `cache_ctx`
    // exposes a backend, a hit-but-drifted entry is restored from the
    // artifact store rather than rebuilt (2026-05-02 addendum spec §5.2).
    //
    // COOK-162 §3/§17 sharing: the disposition (`local`/`pinned`) on the unit's
    // CacheMeta selects which stores are consulted —
    //   - unannotated: local StepEntry, drift-restore from backend, AND a cold
    //     fetch-by-key from the backend; a cold final miss falls through to
    //     rebuild.
    //   - `local`: local StepEntry ONLY. The backend is never consulted (not for
    //     drift restore, not for cold fetch). A cold miss falls through to
    //     rebuild.
    //   - `pinned`: fetch-only. Served from the local index OR a backend
    //     fetch-by-key. A cold miss in BOTH stores is a hard error — the unit
    //     MUST NOT be rebuilt; the caller raises a failure.
    fn check_node_cache(
        work_node: &WorkNode,
        cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
        cache_ctx: &CacheContext,
        probe_store: &cook_luaotp::ProbeValueStore,
    ) -> CacheDecision {
        // COOK-360: these were one answer given for two different reasons.
        // Both still return `Miss(None)`, so behaviour is unchanged — but the
        // reasons are now named and separated, because only one of them is
        // permanent.
        use cook_contracts::cache::record::{cacheability, Cacheability};
        match cacheability(work_node.cache_meta.as_ref()) {
            // Permanent. A chore body or interactive unit is never cached
            // (§7.4); there is no key to look up and never will be.
            Cacheability::Uncacheable => return CacheDecision::Miss(None),
            // NOT permanent. A unit declaring no outputs is cacheable — its
            // hit replays a recorded outcome rather than restoring bytes —
            // but this path only knows how to look up artifacts, so it
            // reports a miss. Test units get their hits from the separate
            // result store instead, which is the duplication COOK-360 exists
            // to remove; folding that store in happens HERE, at this arm, and
            // nowhere else.
            Cacheability::ResultOnly => return CacheDecision::Miss(None),
            Cacheability::Artifacts => {}
        }
        let meta = match &work_node.cache_meta {
            Some(m) => m,
            // Unreachable: `Artifacts` implies a declaration.
            None => return CacheDecision::Miss(None),
        };
        let cm = match cache_managers.get(&work_node.recipe_name) {
            Some(cm) => cm,
            None => return CacheDecision::Miss(None),
        };
        // COOK-306: copy the one keyed entry, never the whole recipe index.
        // At DuckDB scale (1,687 nodes behind a single 648k-record index) the
        // difference is 95% of the process's allocation traffic.
        //
        // `env_moved_key` is computed inside the same lookup: the local step
        // key embeds the env contribution (`<first-output>@<env:x>`), so an
        // env change moves the key and the keyed lookup comes up empty —
        // which would present the dominant "env value flipped" warm re-run as
        // an unattributable cold build. A sibling entry under the same output
        // identity is proof of history: attribute the miss to env (COOK-276).
        let lookup = cm.lookup_step(
            &meta.recipe_name,
            &meta.cache_key,
            meta.output_paths[0].as_str(),
        );
        let entry = lookup.entry.as_ref();
        let env_moved_key = lookup.env_moved_key;
        // CS-0085 §17.6: when any declared output is a glob pattern AND a prior
        // StepEntry exists, derive current_outputs from the recorded concrete
        // paths rather than the raw pattern strings.  Pattern strings don't
        // exist on disk, so passing them directly to needs_rebuild_cook would
        // trigger OutputMissing and force an unnecessary rebuild on every run.
        let any_glob = meta.output_paths.iter().any(|s| cook_fingerprint::is_terminal_output(s));
        let current_outputs_storage: Vec<String> = if any_glob && entry.is_some() {
            entry
                .unwrap()
                .outputs
                .iter()
                .map(|f| f.path.to_string())
                .collect()
        } else {
            meta.output_paths.clone()
        };
        let input_refs: Vec<&str> = meta.input_paths.iter().map(|s| s.as_str()).collect();
        let current_outputs: Vec<&str> = current_outputs_storage.iter().map(|s| s.as_str()).collect();
        let recipe_namespace =
            recipe_namespace(&meta.project_id, &meta.cookfile_path, &meta.recipe_name);
        let restore_ctx = RestoreCtx {
            backend: cache_ctx.backend.as_ref(),
            recipe_namespace: &recipe_namespace,
        };
        // COOK-161: fold the effective seal set's probe values (materialised
        // by now — the unit depends on its sealed probes) into the key.
        let seal_contrib = crate::seal::seal_contribution(&meta.seal_keys, probe_store);
        // COOK-162 §3: a `local` unit MUST NOT consult the shared backend at all
        // — not even on drift restore. Withholding the RestoreCtx confines it to
        // the local StepEntry index.
        let restore_arg = if meta.sharing.is_local() { None } else { Some(&restore_ctx) };
        let (result, updated) = needs_rebuild_cook(
            entry,
            &input_refs,
            &current_outputs,
            meta.command_hash,
            meta.env_contribution,
            seal_contrib,
            &work_node.working_dir,
            restore_arg,
            meta.discovered_inputs.as_ref(),
            meta.record,
        );
        if matches!(result, RebuildResult::Skip) {
            // CS-0119: on a cache hit, reconcile directory outputs to exactly
            // the recorded set so a hit is byte-identical to a fresh build
            // (strays dropped into the dir between runs are swept out).
            if let Some(ref e) = updated {
                let kept: std::collections::BTreeSet<String> =
                    e.outputs.iter().map(|r| r.path.to_string()).collect();
                for entry in &meta.output_paths {
                    if let Some(root) = entry.strip_suffix('/') {
                        cook_fingerprint::reconcile_dir_output(
                            &work_node.working_dir,
                            root,
                            &kept,
                        );
                    }
                }
            }
            if let Some(updated_entry) = updated {
                cm.update_step(&meta.recipe_name, &meta.cache_key, updated_entry);
            }
            return CacheDecision::Hit;
        }
        // RebuildResult::Rebuild — a local miss (includes a cold entry == None).
        // COOK-276: name the changed determinant. NoCacheEntry normally means
        // cold (no attribution), unless the env-in-key probe above found the
        // unit's history parked under a different env suffix.
        let cause = match &result {
            RebuildResult::Rebuild(reason) => reason
                .cause_summary()
                .or_else(|| env_moved_key.then(|| "env changed".to_string())),
            RebuildResult::Skip => None,
        };
        // COOK-162 §3: `local` units never reach the backend, so a local miss is
        // a plain Miss → rebuild.
        if meta.sharing.is_local() {
            return CacheDecision::Miss(cause);
        }
        // Shared unit: attempt a cold fetch-by-key from the backend by
        // recomputing the one key from the declared inputs. A declared input
        // that is missing on disk means the unit cannot be a clean hit; treat it
        // as a backend miss.
        let input_hashes = match cook_fingerprint::hash_input_paths(&input_refs, &work_node.working_dir) {
            Some(h) => h,
            None => {
                return if meta.sharing.is_pinned() {
                    CacheDecision::PinnedColdMiss
                } else {
                    CacheDecision::Miss(cause)
                };
            }
        };
        if let Some(outcome) = cook_fingerprint::fetch_by_key(
            &restore_ctx,
            meta.command_hash,
            meta.env_contribution,
            seal_contrib,
            &input_hashes,
            &current_outputs,
            &work_node.working_dir,
            meta.discovered_inputs.as_ref(),
        ) {
            let restored: std::collections::BTreeSet<&str> =
                outcome.restored_outputs.iter().map(|s| s.as_str()).collect();
            // COOK-278: a fetch hit must be byte-identical to a fresh build.
            // On a warm revert the PREVIOUS build's concrete outputs are still
            // on disk (content-dependent filenames — stale Next.js chunks);
            // sweep every previously-recorded output the restore didn't
            // rewrite, then reconcile `dir/` subtrees to exactly the restored
            // set (CS-0119 parity with the plain-hit path).
            if let Some(prior) = entry {
                let stale = || {
                    prior
                        .outputs
                        .iter()
                        .filter(|rec| !restored.contains(&*rec.path))
                        .map(|rec| work_node.working_dir.join(&*rec.path))
                };
                // Files first, then empty dirs (`remove_dir`, not `_all`): a
                // stale dir record whose subtree gained restored files must
                // survive — non-empty removal fails and that is correct.
                cook_fingerprint::statmemo::disarm();
                for abs in stale().filter(|p| !p.is_dir()) {
                    let _ = std::fs::remove_file(&abs);
                }
                for abs in stale().filter(|p| p.is_dir()) {
                    let _ = std::fs::remove_dir(&abs);
                }
            }
            let kept: std::collections::BTreeSet<String> =
                outcome.restored_outputs.iter().cloned().collect();
            for out in &meta.output_paths {
                if let Some(root) = out.strip_suffix('/') {
                    cook_fingerprint::reconcile_dir_output(
                        &work_node.working_dir,
                        root,
                        &kept,
                    );
                }
            }
            // COOK-269: a cold fetch restores the outputs but previously
            // recorded nothing locally, so §17.7's prior-output snapshot came
            // up empty after a fresh clone / lost `.cook` — an output orphaned
            // by a later shrink was never swept. Record the StepEntry a fresh
            // execution would have written, hashed from the bytes on disk.
            // COOK-278: the entry is FAT — declared + discovered inputs, and
            // the RESTORED output list (not the stale pre-fetch one) — so the
            // next unchanged run is a plain local skip instead of a perpetual
            // InputSetChanged → refetch round-trip. Complete-or-skip: a path
            // that cannot be recorded skips recording entirely rather than
            // persisting a partial list whose artifact indices would misalign.
            let file_record = |p: &str| {
                let abs = work_node.working_dir.join(p);
                cook_fingerprint::hash_file(&abs).map(|h| cook_fingerprint::FileRecord {
                    path: p.into(),
                    mtime: cook_fingerprint::stat_mtime(&abs).unwrap_or(0),
                    hash: h,
                })
            };
            // Restored outputs can include empty-dir records (COOK-180);
            // record those with the same hash-0 convention publish uses.
            let output_record = |p: &str| {
                let abs = work_node.working_dir.join(p);
                if abs.is_dir() {
                    Some(cook_fingerprint::FileRecord {
                        path: p.into(),
                        mtime: cook_fingerprint::stat_mtime(&abs).unwrap_or(0),
                        hash: 0,
                    })
                } else {
                    file_record(p)
                }
            };
            let inputs: Option<Vec<_>> = meta
                .input_paths
                .iter()
                .map(|p| file_record(p))
                .chain(outcome.discovered_paths.iter().map(|p| file_record(p)))
                .collect();
            let outputs: Option<Vec<_>> = outcome
                .restored_outputs
                .iter()
                .map(|p| output_record(p))
                .collect();
            if let (Some(inputs), Some(outputs)) = (inputs, outputs) {
                cm.update_step(
                    &meta.recipe_name,
                    &meta.cache_key,
                    cook_fingerprint::StepEntry {
                        inputs,
                        outputs,
                        command_hash: meta.command_hash,
                        env_contribution: meta.env_contribution,
                        seal_contribution: seal_contrib,
                    },
                );
            }
            CacheDecision::Hit
        } else if meta.sharing.is_pinned() {
            // Fetch-only unit absent from BOTH the local index and the shared
            // store: a hard error. The caller MUST NOT dispatch it.
            CacheDecision::PinnedColdMiss
        } else {
            CacheDecision::Miss(cause)
        }
    }

    // ----- helper: process a newly-ready node -----
    // Returns how many work items were submitted to the pool.
    #[allow(clippy::too_many_arguments)]
    fn process_ready(
        dag: &Dag<WorkNode>,
        id: usize,
        pool: &WorkerPool,
        cancelled: &mut Vec<bool>,
        finished: &mut usize,
        interactive_queue: &mut Vec<usize>,
        event_tx: &Option<mpsc::Sender<EngineEvent>>,
        trackers: &mut BTreeMap<String, RecipeTracker>,
        cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
        cache_ctx: &CacheContext,
        failures: &mut Vec<(usize, String, String)>,
        test_cache: Option<&TestCache>,
        cached_test_results: &mut Vec<crate::TestResult>,
        rerun_patterns: &[String],
        blocked_results: &mut Vec<crate::TestResult>,
        // G4 (CS-0074): probe cache lookup state.
        probe_units_by_node: &BTreeMap<usize, cook_contracts::ProbeUnit>,
        upstream_probe_fingerprints: &mut BTreeMap<String, [u8; 32]>,
        probe_fingerprint_by_node: &mut BTreeMap<usize, [u8; 32]>,
        keyless_probes: &mut std::collections::BTreeSet<String>,
        published: &AtomicU64,
    ) -> usize {
        if cancelled[id] {
            *finished += 1;
            return 0;
        }

        let node = dag.node(id);
        let work_node = node.payload();

        match &work_node.payload {
            None => {
                // Pre-satisfied (cached): complete immediately and cascade.
                ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                emit(
                    event_tx,
                    EngineEvent::NodeCacheHit {
                        recipe: work_node.recipe_name.clone(),
                        unit: id,
                        node_name: work_node.recipe_name.clone(),
                        artifact: work_node.cache_meta.as_ref()
                            .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
                        // Pre-satisfied node: no payload to derive a kind from.
                        kind: NodeKind::Cooked,
                    },
                );
                finish_recipe_node(trackers, &work_node.recipe_name, true, false, event_tx);

                *finished += 1;
                let newly_ready = dag.complete(id);
                let mut submitted = 0;
                for nid in newly_ready {
                    submitted += process_ready(
                        dag,
                        nid,
                        pool,
                        cancelled,
                        finished,
                        interactive_queue,
                        event_tx,
                        trackers,
                        cache_managers,
                        cache_ctx,
                        failures,
                        test_cache,
                        cached_test_results,
                        rerun_patterns,
                        blocked_results,
                        probe_units_by_node,
                        upstream_probe_fingerprints,
                        probe_fingerprint_by_node,
                        keyless_probes,
                        published,
                    );
                }
                submitted
            }
            Some(WorkPayload::Interactive { .. }) => {
                // Queue for main-thread execution after pool drains.
                interactive_queue.push(id);
                0
            }
            Some(WorkPayload::LuaChunk { is_chore: true, .. }) => {
                // CS-0051: chore-body Lua bundles share the drain with the
                // body's shell steps. The chore-window loop submits each
                // such chunk to the worker pool individually and waits for
                // its single result, preserving the one-drain semantic
                // (no other recipe's work runs while the chore body owns
                // the controlling terminal).
                interactive_queue.push(id);
                0
            }
            Some(WorkPayload::Test { test_name, should_fail, line, iteration_item, .. }) => {
                // Phase 5: test-result cache lookup.
                // Check the content-addressed test cache before submitting to
                // the pool. On a hit, synthesize TestStarted + TestPassed
                // (cached=true) events and mark the node done without dispatch.
                if let Some(tc) = test_cache {
                    // COOK-211: compute the fingerprint here at ready time — all
                    // predecessors are complete, so their consumed outputs are
                    // materialised and can be content-hashed for early cutoff.
                    if let Some(fp) = crate::run::compute_ready_test_fingerprint(dag, id, cache_ctx, &pool.probe_value_store()) {
                        let fp = &fp;
                        // Force-rerun: if the test id matches any --rerun pattern,
                        // skip cache lookup. Cache write still occurs after the
                        // test runs (executor's success-path write site below),
                        // so a forced re-run refreshes the cached entry.
                        let test_id_str = match iteration_item {
                            Some(item) if !item.is_empty() => format!("{}:{}[{}]", work_node.recipe_name, test_name, item),
                            _ => format!("{}:{}", work_node.recipe_name, test_name),
                        };
                        let force_rerun = rerun_matches(&test_id_str, rerun_patterns);
                        let cached_entry = if force_rerun { None } else { tc.lookup(fp) };
                        if let Some(entry) = cached_entry {
                            // Cache hit — synthesize events and skip execution.
                            ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                            let test_id = crate::id::parse_test_id(&test_id_str);
                            let duration = std::time::Duration::from_secs_f64(entry.duration_secs);
                            emit(event_tx, EngineEvent::TestStarted {
                                id: test_id.clone(),
                                recipe: work_node.recipe_name.clone(),
                                name: test_name.clone(),
                                line: *line as u32,
                                iteration_item: iteration_item.clone(),
                            });
                            emit(event_tx, EngineEvent::TestPassed {
                                id: test_id.clone(),
                                duration,
                                cached: true,
                                should_fail: entry.should_fail_observed,
                                stdout: entry.stdout.clone(),
                                stderr: entry.stderr.clone(),
                                line: *line as u32,
                            });
                            // Register the node under its derived name (CS-0160)
                            // before completing it. `NodeCompleted` only updates a
                            // node that already exists and is Running, so without
                            // this a cache-hit test node was never created by any
                            // event carrying a name: renderers fell through to the
                            // command-token fallback and printed `?`, and the JSON
                            // stream emitted a bare `node#N`. Every other cached
                            // node kind already emits `NodeCacheHit`; this brings
                            // the test path in line. The progress model marks the
                            // node Completed on this event, so the `NodeCompleted`
                            // below finds it non-Running and does not double-count.
                            emit(event_tx, EngineEvent::NodeCacheHit {
                                recipe: work_node.recipe_name.clone(),
                                unit: id,
                                node_name: test_name.clone(),
                                artifact: None,
                                kind: NodeKind::Test,
                            });
                            // Emit NodeCompleted so the recipe tracker counts this node.
                            emit(event_tx, EngineEvent::NodeCompleted {
                                recipe: work_node.recipe_name.clone(),
                                unit: id,
                                node_name: test_name.clone(),
                                elapsed: duration,
                                kind: NodeKind::Test,
                                cache_key: node_cache_key(work_node),
                            });
                            finish_recipe_node(trackers, &work_node.recipe_name, true, false, event_tx);

                            let namespace = crate::id::id_namespace(&test_id);
                            let recipe = crate::id::id_recipe(&test_id);
                            cached_test_results.push(crate::TestResult {
                                id: test_id,
                                namespace,
                                recipe,
                                name: test_name.clone(),
                                suite: work_node.recipe_name.clone(),
                                iteration_item: iteration_item.clone(),
                                outcome: crate::TestOutcome::Passed,
                                duration,
                                from_cache: true,
                                stdout: entry.stdout,
                                stderr: entry.stderr,
                                fingerprint: Some(fp.clone()),
                                blocked_by: None,
                                should_fail: entry.should_fail_observed,
                                timed_out: false,
                                line: *line as u32,
                                exit_code: None,
                            });

                            *finished += 1;
                            let newly_ready = dag.complete(id);
                            let mut submitted = 0;
                            for nid in newly_ready {
                                submitted += process_ready(
                                    dag,
                                    nid,
                                    pool,
                                    cancelled,
                                    finished,
                                    interactive_queue,
                                    event_tx,
                                    trackers,
                                    cache_managers,
                                    cache_ctx,
                                    failures,
                                    test_cache,
                                    cached_test_results,
                                    rerun_patterns,
                                    blocked_results,
                                    probe_units_by_node,
                                    upstream_probe_fingerprints,
                                    probe_fingerprint_by_node,
                                    keyless_probes,
                                    published,
                                );
                            }
                            return submitted;
                        }
                    }
                }
                // Cache miss (or caching disabled) — fall through to normal dispatch.
                // Reuse the generic Some(payload) path below.
                let payload = match &work_node.payload {
                    Some(p) => p,
                    None => unreachable!(),
                };
                // Check artifact cache before executing (no-op for Test nodes since they
                // have no cache_meta, but kept for structural symmetry).
                // COOK-162: `pinned` cold-miss aborts the node like a failed step.
                let miss_cause = match check_node_cache(work_node, cache_managers, cache_ctx, &pool.probe_value_store()) {
                    CacheDecision::Hit => {
                        ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                        emit(
                            event_tx,
                            EngineEvent::NodeCacheHit {
                                recipe: work_node.recipe_name.clone(),
                                unit: id,
                                node_name: payload.display_name(),
                                artifact: work_node.cache_meta.as_ref()
                                    .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
                                kind: node_kind_for_payload(payload),
                            },
                        );
                        finish_recipe_node(trackers, &work_node.recipe_name, true, false, event_tx);

                        *finished += 1;
                        let newly_ready = dag.complete(id);
                        let mut submitted = 0;
                        for nid in newly_ready {
                            submitted += process_ready(
                                dag,
                                nid,
                                pool,
                                cancelled,
                                finished,
                                interactive_queue,
                                event_tx,
                                trackers,
                                cache_managers,
                                cache_ctx,
                                failures,
                                test_cache,
                                cached_test_results,
                                rerun_patterns,
                                blocked_results,
                                probe_units_by_node,
                                upstream_probe_fingerprints,
                                probe_fingerprint_by_node,
                                keyless_probes,
                                published,
                            );
                        }
                        return submitted;
                    }
                    CacheDecision::PinnedColdMiss => {
                        ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                        let msg = format!(
                            "pinned unit '{}' has no cached artifact for its key; pinned units are fetch-only and are never rebuilt",
                            payload.display_name()
                        );
                        emit(
                            event_tx,
                            EngineEvent::NodeFailed {
                                recipe: work_node.recipe_name.clone(),
                                unit: id,
                                node_name: payload.display_name(),
                                elapsed: std::time::Duration::ZERO,
                                error: msg.clone(),
                            },
                        );
                        failures.push((id, work_node.recipe_name.clone(), msg));
                        finish_recipe_node(trackers, &work_node.recipe_name, false, true, event_tx);
                        *finished += 1;
                        let dependents: Vec<usize> = dag.node(id).dependents().to_vec();
                        for dep_id in dependents {
                            cancel_subtree(dag, dep_id, cancelled, event_tx, trackers, &payload.display_name(), blocked_results);
                        }
                        return 0;
                    }
                    CacheDecision::Miss(cause) => cause,
                };

                ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                emit(
                    event_tx,
                    EngineEvent::NodeStarted {
                        recipe: work_node.recipe_name.clone(),
                        unit: id,
                        node_name: payload.display_name(),
                        artifact: work_node.cache_meta.as_ref()
                            .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
                        fallback_label: payload.display_name(),
                        kind: node_kind_for_payload(payload),
                        cause: miss_cause,
                        cache_key: node_cache_key(work_node),
                    },
                );
                // Emit TestStarted for this test-step node.
                let test_id_str = match iteration_item {
                    Some(item) if !item.is_empty() => format!("{}:{}[{}]", work_node.recipe_name, test_name, item),
                    _ => format!("{}:{}", work_node.recipe_name, test_name),
                };
                emit(
                    event_tx,
                    EngineEvent::TestStarted {
                        id: crate::id::parse_test_id(&test_id_str),
                        recipe: work_node.recipe_name.clone(),
                        name: test_name.clone(),
                        line: *line as u32,
                        iteration_item: iteration_item.clone(),
                    },
                );

                // CS-0050: ensure parent dirs for cook-step outputs.
                // Test nodes have no cache_meta, so this is a no-op.
                if let Err(err_msg) = ensure_output_parent_dirs(work_node) {
                    emit(
                        event_tx,
                        EngineEvent::NodeFailed {
                            recipe: work_node.recipe_name.clone(),
                            unit: id,
                            node_name: payload.display_name(),
                            elapsed: std::time::Duration::ZERO,
                            error: err_msg.clone(),
                        },
                    );
                    failures.push((id, work_node.recipe_name.clone(), err_msg));
                    finish_recipe_node(trackers, &work_node.recipe_name, false, true, event_tx);
                    *finished += 1;
                    let dependents: Vec<usize> = dag.node(id).dependents().to_vec();
                    for dep_id in dependents {
                        cancel_subtree(dag, dep_id, cancelled, event_tx, trackers, &payload.display_name(), blocked_results);
                    }
                    return 0;
                }

                let env_vars_hashmap: std::collections::HashMap<String, String> =
                    work_node.env_vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let _ = should_fail; // used in the TestPassed path via TestOutput
                pool.submit(WorkItem {
                    id,
                    payload: payload.clone(),
                    recipe_name: work_node.recipe_name.clone(),
                    working_dir: work_node.working_dir.clone(),
                    env_vars: env_vars_hashmap,
                    process_env_vars: work_node
                        .process_env_vars
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    project_root: cache_ctx.project_root.clone(),
                });
                1
            }
            Some(WorkPayload::Probe { key, .. }) => {
                // G4 (CS-0074): probe cache lookup before worker dispatch.
                //
                // If the probe has a `ProbeUnit` entry in `probe_units_by_node`
                // (populated by the call site from `RecipeUnits.probes`), we
                // compute its fingerprint and attempt a cache GET. On a hit we
                // insert the cached bytes directly into the ProbeValueStore
                // and complete the node without dispatching to a worker, unblocking
                // downstream consumers. On a miss (or when probe metadata is
                // absent), we fall through to normal worker dispatch.
                //
                // The fingerprint is also stored in `probe_fingerprint_by_node`
                // so that the completion handler (G5) can reuse it without
                // recomputation. Storing on dispatch — not on completion — means
                // the map is populated regardless of whether the result came from
                // cache or from the worker.
                let probe_key = key.clone();
                let node_name = format!("probe:{}", probe_key);

                if let Some(probe_unit) = probe_units_by_node.get(&id) {
                    // G4 (CS-0074): everything decided before a probe value
                    // exists — fingerprint, keylessness, tool locations, the
                    // cache lookup, and the producer kinds that need no VM —
                    // belongs to `cook_probe::eval` and is shared with the
                    // register pre-pass (COOK-359). What stays here is what is
                    // genuinely the scheduler's: dispatch, events, node
                    // completion, and fingerprint propagation.
                    //
                    // CS-0172: an `envs { }` probe records AMBIENT PROCESS
                    // environment values (§22.5.2) — the whole point of the
                    // probe is to make a host value a keyed determinant — so
                    // the lookup reads the process environment, not the
                    // declared-variable namespace. The two were the same table
                    // before CS-0172, which let a config block redefine what
                    // the probe recorded about the host.
                    let env_lookup = |name: &str| std::env::var(name).ok();
                    let eval_ctx = cook_probe::eval::EvalCtx {
                        working_dir: &work_node.working_dir,
                        cache: Some(cook_probe::eval::CacheAccess {
                            backend: cache_ctx.backend.as_ref(),
                            project_root: &cache_ctx.project_root,
                            publish_enabled: cache_ctx.publish_enabled,
                        }),
                    };
                    match cook_probe::eval::lookup(
                        probe_unit,
                        &eval_ctx,
                        &env_lookup,
                        upstream_probe_fingerprints,
                        keyless_probes,
                    ) {
                        Ok(found) => {
                            for w in &found.warnings {
                                tracing::warn!("{w}");
                            }
                            // Store the fingerprint now so G5 and downstream
                            // probes find it whether this node hits or misses.
                            probe_fingerprint_by_node.insert(id, found.fingerprint);
                            if found.keyless {
                                keyless_probes.insert(probe_key.clone());
                            }
                            // CS-0157: where each declared tool resolves RIGHT
                            // NOW, as a per-run read view. Set before the
                            // hit/miss fork so both paths carry it — a cached
                            // value must not be the source of a location.
                            if !found.tool_paths.is_empty() {
                                pool.probe_value_store()
                                    .set_tool_paths(&probe_key, found.tool_paths.clone());
                            }

                            // A value already in hand: the cache served it, or
                            // the producer kind is synthesised (CS-0148
                            // `files { }`). Either way no worker is involved,
                            // so the node completes here.
                            if let Some((bytes, source)) = found.resolved.as_ref() {
                                let started = std::time::Instant::now();
                                let recorded = cook_probe::eval::record(
                                    &probe_key,
                                    &eval_ctx,
                                    &found.fingerprint,
                                    found.keyless,
                                    bytes,
                                    *source,
                                );
                                for w in &recorded.warnings {
                                    tracing::warn!("{w}");
                                }
                                if recorded.published {
                                    published.fetch_add(1, Ordering::Relaxed);
                                }
                                pool.probe_value_store().insert(&probe_key, bytes.clone());
                                // Propagate the fingerprint so downstream probes
                                // can resolve their own upstream entries, exactly
                                // as the worker-result handler does on a miss.
                                upstream_probe_fingerprints
                                    .insert(probe_key.clone(), found.fingerprint);
                                ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                                match source {
                                    // Served by the cache: report a hit.
                                    cook_probe::eval::ValueSource::Cache => {
                                        tracing::debug!(
                                            "probe '{}': cache hit (fp={:x?})",
                                            probe_key,
                                            &found.fingerprint[..4],
                                        );
                                        emit(
                                            event_tx,
                                            EngineEvent::NodeCacheHit {
                                                recipe: work_node.recipe_name.clone(),
                                                unit: id,
                                                node_name: node_name.clone(),
                                                artifact: None,
                                                // No Probe variant; `display()`
                                                // special-cases the `probe:` name
                                                // prefix, so the default is correct.
                                                kind: NodeKind::Cooked,
                                            },
                                        );
                                    }
                                    // Synthesised without a VM: this is work that
                                    // ran, so it reports as a started-and-completed
                                    // node rather than a hit.
                                    cook_probe::eval::ValueSource::Produced => {
                                        emit(
                                            event_tx,
                                            EngineEvent::NodeStarted {
                                                recipe: work_node.recipe_name.clone(),
                                                unit: id,
                                                node_name: node_name.clone(),
                                                artifact: None,
                                                fallback_label: node_name.clone(),
                                                kind: NodeKind::Cooked,
                                                cause: None,
                                                cache_key: node_cache_key(work_node),
                                            },
                                        );
                                        emit(
                                            event_tx,
                                            EngineEvent::NodeCompleted {
                                                recipe: work_node.recipe_name.clone(),
                                                unit: id,
                                                node_name: node_name.clone(),
                                                elapsed: started.elapsed(),
                                                kind: NodeKind::Cooked,
                                                cache_key: node_cache_key(work_node),
                                            },
                                        );
                                    }
                                }
                                finish_recipe_node(
                                    trackers,
                                    &work_node.recipe_name,
                                    true,
                                    false,
                                    event_tx,
                                );
                                *finished += 1;
                                let newly_ready = dag.complete(id);
                                let mut submitted = 0;
                                for nid in newly_ready {
                                    submitted += process_ready(
                                        dag,
                                        nid,
                                        pool,
                                        cancelled,
                                        finished,
                                        interactive_queue,
                                        event_tx,
                                        trackers,
                                        cache_managers,
                                        cache_ctx,
                                        failures,
                                        test_cache,
                                        cached_test_results,
                                        rerun_patterns,
                                        blocked_results,
                                        probe_units_by_node,
                                        upstream_probe_fingerprints,
                                        probe_fingerprint_by_node,
                                        keyless_probes,
                                        published,
                                    );
                                }
                                return submitted;
                            }

                            tracing::debug!(
                                "probe '{}': no value in hand, dispatching to worker",
                                probe_key,
                            );
                        }
                        Err(e) => {
                            // Fingerprint resolution failed (e.g. missing upstream).
                            // This is a hard error — the probe cannot be fingerprinted
                            // so it cannot safely proceed.
                            let err_msg =
                                format!("probe '{}': fingerprint resolution failed: {}", probe_key, e.message());
                            ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                            emit(
                                event_tx,
                                EngineEvent::NodeFailed {
                                    recipe: work_node.recipe_name.clone(),
                                    unit: id,
                                    node_name: node_name.clone(),
                                    elapsed: Duration::ZERO,
                                    error: err_msg.clone(),
                                },
                            );
                            failures.push((id, work_node.recipe_name.clone(), err_msg.clone()));
                            finish_recipe_node(trackers, &work_node.recipe_name, false, true, event_tx);
                            *finished += 1;
                            let dependents: Vec<usize> = dag.node(id).dependents().to_vec();
                            for dep_id in dependents {
                                cancel_subtree(dag, dep_id, cancelled, event_tx, trackers, &node_name, blocked_results);
                            }
                            return 0;
                        }
                    }
                }

                // Cache miss (or no probe metadata) — dispatch to worker as G1.
                ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                emit(
                    event_tx,
                    EngineEvent::NodeStarted {
                        recipe: work_node.recipe_name.clone(),
                        unit: id,
                        node_name: node_name.clone(),
                        artifact: None,
                        fallback_label: node_name.clone(),
                        kind: NodeKind::Cooked,
                        cause: None,
                        cache_key: node_cache_key(work_node),
                    },
                );

                let env_vars_hashmap: std::collections::HashMap<String, String> =
                    work_node.env_vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let payload = work_node.payload.as_ref().expect("checked: Probe arm");
                pool.submit(WorkItem {
                    id,
                    payload: payload.clone(),
                    recipe_name: work_node.recipe_name.clone(),
                    working_dir: work_node.working_dir.clone(),
                    env_vars: env_vars_hashmap,
                    process_env_vars: work_node
                        .process_env_vars
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    project_root: cache_ctx.project_root.clone(),
                });
                1
            }
            Some(payload) => {
                // Check cache before executing.
                // COOK-162: `pinned` cold-miss aborts the node like a failed step.
                let miss_cause = match check_node_cache(work_node, cache_managers, cache_ctx, &pool.probe_value_store()) {
                    CacheDecision::Hit => {
                        ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                        emit(
                            event_tx,
                            EngineEvent::NodeCacheHit {
                                recipe: work_node.recipe_name.clone(),
                                unit: id,
                                node_name: payload.display_name(),
                                artifact: work_node.cache_meta.as_ref()
                                    .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
                                kind: node_kind_for_payload(payload),
                            },
                        );
                        finish_recipe_node(trackers, &work_node.recipe_name, true, false, event_tx);

                        *finished += 1;
                        let newly_ready = dag.complete(id);
                        let mut submitted = 0;
                        for nid in newly_ready {
                            submitted += process_ready(
                                dag,
                                nid,
                                pool,
                                cancelled,
                                finished,
                                interactive_queue,
                                event_tx,
                                trackers,
                                cache_managers,
                                cache_ctx,
                                failures,
                                test_cache,
                                cached_test_results,
                                rerun_patterns,
                                blocked_results,
                                probe_units_by_node,
                                upstream_probe_fingerprints,
                                probe_fingerprint_by_node,
                                keyless_probes,
                                published,
                            );
                        }
                        return submitted;
                    }
                    CacheDecision::PinnedColdMiss => {
                        ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                        let msg = format!(
                            "pinned unit '{}' has no cached artifact for its key; pinned units are fetch-only and are never rebuilt",
                            payload.display_name()
                        );
                        emit(
                            event_tx,
                            EngineEvent::NodeFailed {
                                recipe: work_node.recipe_name.clone(),
                                unit: id,
                                node_name: payload.display_name(),
                                elapsed: Duration::ZERO,
                                error: msg.clone(),
                            },
                        );
                        failures.push((id, work_node.recipe_name.clone(), msg));
                        finish_recipe_node(
                            trackers,
                            &work_node.recipe_name,
                            false,
                            true,
                            event_tx,
                        );
                        *finished += 1;
                        let dependents: Vec<usize> = dag.node(id).dependents().to_vec();
                        for dep_id in dependents {
                            cancel_subtree(dag, dep_id, cancelled, event_tx, trackers, &payload.display_name(), blocked_results);
                        }
                        return 0;
                    }
                    CacheDecision::Miss(cause) => cause,
                };

                ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                emit(
                    event_tx,
                    EngineEvent::NodeStarted {
                        recipe: work_node.recipe_name.clone(),
                        unit: id,
                        node_name: payload.display_name(),
                        artifact: work_node.cache_meta.as_ref()
                            .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
                        fallback_label: payload.display_name(),
                        kind: node_kind_for_payload(payload),
                        cause: miss_cause,
                        cache_key: node_cache_key(work_node),
                    },
                );
                // Emit TestStarted for test-step nodes so Phase 4 reporters can
                // track in-flight tests.
                if let WorkPayload::Test { test_name, line, iteration_item, .. } = payload {
                    let test_id_str = match iteration_item {
                        Some(item) if !item.is_empty() => format!("{}:{}[{}]", work_node.recipe_name, test_name, item),
                        _ => format!("{}:{}", work_node.recipe_name, test_name),
                    };
                    let test_id = crate::id::parse_test_id(&test_id_str);
                    emit(
                        event_tx,
                        EngineEvent::TestStarted {
                            id: test_id,
                            recipe: work_node.recipe_name.clone(),
                            name: test_name.clone(),
                            line: *line as u32,
                            iteration_item: iteration_item.clone(),
                        },
                    );
                }

                // CS-0050: ensure parent directories of declared cook-step
                // outputs exist before the shell text runs. No-op for
                // non-cook units (cache_meta == None) and for outputs whose
                // parent already exists. A non-directory at the parent path
                // is reported as a node failure rather than a panic; the
                // surrounding bookkeeping mirrors a worker-pool failure.
                if let Err(err_msg) = ensure_output_parent_dirs(work_node) {
                    emit(
                        event_tx,
                        EngineEvent::NodeFailed {
                            recipe: work_node.recipe_name.clone(),
                            unit: id,
                            node_name: payload.display_name(),
                            elapsed: Duration::ZERO,
                            error: err_msg.clone(),
                        },
                    );
                    failures.push((id, work_node.recipe_name.clone(), err_msg));
                    finish_recipe_node(
                        trackers,
                        &work_node.recipe_name,
                        false,
                        true,
                        event_tx,
                    );
                    *finished += 1;
                    let dependents: Vec<usize> = dag.node(id).dependents().to_vec();
                    for dep_id in dependents {
                        cancel_subtree(dag, dep_id, cancelled, event_tx, trackers, &payload.display_name(), blocked_results);
                    }
                    return 0;
                }

                // CS-0119: build-owned pre-clean — before the command runs,
                // empty any declared directory outputs so the post-execution
                // resolve_output_paths sees only what THIS invocation produced.
                // Without this, files from a previous build that the new command
                // no longer writes would survive as orphans.
                if let Some(meta) = &work_node.cache_meta {
                    for entry in &meta.output_paths {
                        if let Some(root) = entry.strip_suffix('/') {
                            let dir = work_node.working_dir.join(root);
                            if dir.is_dir() {
                                let empty: std::collections::BTreeSet<String> =
                                    std::collections::BTreeSet::new();
                                cook_fingerprint::reconcile_dir_output(
                                    &work_node.working_dir,
                                    root,
                                    &empty,
                                );
                            }
                        }
                    }
                }

                // Convert BTreeMap env_vars to HashMap for WorkItem
                let env_vars_hashmap: std::collections::HashMap<String, String> =
                    work_node.env_vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

                pool.submit(WorkItem {
                    id,
                    payload: payload.clone(),
                    recipe_name: work_node.recipe_name.clone(),
                    working_dir: work_node.working_dir.clone(),
                    env_vars: env_vars_hashmap,
                    process_env_vars: work_node
                        .process_env_vars
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    // CS-0045: project_root drives the worker's
                    // sandbox policy. CacheContext is the canonical
                    // source — it survives the cross-Cookfile-import
                    // case where work_node.working_dir is an
                    // imported subdir but the project root stays at
                    // the workspace root.
                    project_root: cache_ctx.project_root.clone(),
                });
                1
            }
        }
    }

    let mut interactive_queue: Vec<usize> = Vec::new();
    let mut finished: usize = 0;

    // ----- Seed: initial ready nodes -----
    let initial = dag.initial_ready();
    for id in initial {
        pending += process_ready(
            &dag,
            id,
            &pool,
            &mut cancelled,
            &mut finished,
            &mut interactive_queue,
            &event_tx,
            &mut recipe_trackers,
            &cache_managers,
            &cache_ctx,
            &mut failures,
            test_cache,
            &mut cached_test_results,
            rerun_patterns,
            &mut blocked_results,
            probe_units_by_node,
            &mut upstream_probe_fingerprints,
            &mut probe_fingerprint_by_node,
            &mut keyless_probes,
            published,
        );
    }

    // ----- Main loop: receive results until every node is accounted for -----
    loop {
        // If pool is drained and we have interactive nodes queued, run them.
        //
        // CS-0051: a chore body MUST execute as a single drain. We branch
        // on the head's `is_chore` flag. Chore steps for the same recipe
        // are drained as one window with one InteractiveStart/End pair;
        // legacy non-chore interactives keep their per-node pair.
        while pending == 0 && !interactive_queue.is_empty() {
            let head_id = interactive_queue[0];
            if cancelled[head_id] {
                interactive_queue.remove(0);
                finished += 1;
                continue;
            }

            // Peek at head's payload to decide chore vs legacy path.
            // A chore-window head may be either an interactive shell step
            // or a Lua-bundle step, both flagged via `is_chore = true`.
            let head_is_chore = is_chore_window_member(&dag.node(head_id).payload().payload);

            if head_is_chore {
                // -------- CHORE-WINDOW PATH (CS-0051) --------
                //
                // A chore body is emitted as a linear chain of `Interactive`
                // units bracketed by `_enter_chore`/`_exit_chore` (step1 →
                // step2 → … stepN). The dag_units emitter chains them with
                // dependency edges, so only the head is initially in the
                // interactive queue — later steps surface as we complete
                // each predecessor.
                //
                // To emit one InteractiveStart up front (so the renderer can
                // hide the progress bars BEFORE any chore output appears),
                // we statically discover the full window by walking
                // `dependents()` from the head while same-recipe / chore /
                // single-successor invariants hold. After the walk we have
                // the full step count, run the steps, then close with one
                // InteractiveEnd.
                let chore_recipe = dag.node(head_id).payload().recipe_name.clone();

                ensure_recipe_started(&mut recipe_trackers, &chore_recipe, &event_tx);
                if let Some(t) = recipe_trackers.get_mut(&chore_recipe) {
                    t.is_chore = true;
                }

                // Pop the head off the queue; gather any other chore-body
                // steps that are also already queued (rare — the chain is
                // usually linear so only the head is ready — but harmless).
                let mut window: Vec<usize> = vec![interactive_queue.remove(0)];
                while let Some(&peek_id) = interactive_queue.first() {
                    let same_recipe =
                        dag.node(peek_id).payload().recipe_name == chore_recipe;
                    let same_kind =
                        is_chore_window_member(&dag.node(peek_id).payload().payload);
                    if same_recipe && same_kind {
                        window.push(interactive_queue.remove(0));
                    } else {
                        break;
                    }
                }

                // Walk the linear chain of chore-body successors from the
                // tail of `window`. The chain ends at the first node that
                // (a) is not a chore body for the same recipe, or
                // (b) has multiple dependents/dependents-of-dependents
                //     beyond the simple chain.
                let mut tail = *window.last().unwrap();
                loop {
                    let dependents = dag.node(tail).dependents();
                    if dependents.len() != 1 {
                        break;
                    }
                    let next = dependents[0];
                    let same_recipe =
                        dag.node(next).payload().recipe_name == chore_recipe;
                    let same_kind =
                        is_chore_window_member(&dag.node(next).payload().payload);
                    if !(same_recipe && same_kind) {
                        break;
                    }
                    // The next node must not have other unmet predecessors —
                    // i.e. it's truly waiting only on `tail`. We're walking
                    // before any window step has run, so `remaining_deps`
                    // counts every predecessor still pending. A value of 1
                    // means `tail` is the sole gate; anything else means
                    // `next` has a fan-in beyond the chore chain.
                    if dag.node(next).remaining_deps() != 1 {
                        break;
                    }
                    window.push(next);
                    tail = next;
                }

                let n = window.len();
                let head_node_name = dag
                    .node(window[0])
                    .payload()
                    .payload
                    .as_ref()
                    .map(|p| p.display_name())
                    .unwrap_or_else(|| chore_recipe.clone());

                // Emit the bracketing InteractiveStart BEFORE any step runs
                // so the renderer freezes / hides progress before chore
                // output is interleaved.
                emit(
                    &event_tx,
                    EngineEvent::InteractiveStart {
                        recipe: chore_recipe.clone(),
                        node_name: head_node_name.clone(),
                        chore_step_count: n,
                    },
                );

                let chore_start = Instant::now();
                let mut failed_idx: Option<usize> = None; // 1-indexed step number
                let mut last_err: Option<String> = None;

                for (idx0, &id) in window.iter().enumerate() {
                    if cancelled[id] {
                        // Pre-cancelled; treat as a skipped attempt below.
                        continue;
                    }
                    let work_node = dag.node(id).payload();

                    // CS-0051: a chore-window member is either a shell-step
                    // interactive unit or a Lua-bundle unit. The shell case
                    // runs on this thread via `run_interactive_on_main`; the
                    // Lua case is submitted to the worker pool and we block
                    // on the single result. Because chore windows enter only
                    // when `pending == 0`, the submitted Lua chunk is the
                    // only in-flight item, so the next `rx.recv()` returns
                    // it — no lock-step protocol needed.
                    let result: Result<(), String> = match &work_node.payload {
                        Some(WorkPayload::Interactive { cmd, line, is_chore: _ }) => {
                            // CS-0050: parent-dir creation is a no-op for
                            // chore bodies (no cache_meta) but kept for
                            // uniformity.
                            match ensure_output_parent_dirs(work_node) {
                                // R1 (CS-0164): a chore/interactive step spawns
                                // with the process-env subset (its bound chore
                                // params), not config `var.*` values.
                                Ok(()) => run_interactive_on_main(
                                    cmd,
                                    *line,
                                    &work_node.working_dir,
                                    &work_node.process_env_vars,
                                ),
                                Err(e) => Err(e),
                            }
                        }
                        Some(WorkPayload::LuaChunk { .. }) => {
                            match ensure_output_parent_dirs(work_node) {
                                Ok(()) => {
                                    let env_vars_hashmap: std::collections::HashMap<
                                        String,
                                        String,
                                    > = work_node
                                        .env_vars
                                        .iter()
                                        .map(|(k, v)| (k.clone(), v.clone()))
                                        .collect();
                                    pool.submit(WorkItem {
                                        id,
                                        payload: work_node.payload.clone().expect(
                                            "chore-window LuaChunk node missing payload",
                                        ),
                                        recipe_name: work_node.recipe_name.clone(),
                                        working_dir: work_node.working_dir.clone(),
                                        env_vars: env_vars_hashmap,
                                        process_env_vars: work_node
                                            .process_env_vars
                                            .iter()
                                            .map(|(k, v)| (k.clone(), v.clone()))
                                            .collect(),
                                        project_root: cache_ctx.project_root.clone(),
                                    });
                                    match rx.recv() {
                                        Ok(work_result) => {
                                            // Forward any captured output.
                                            // Lua chunks normally inherit
                                            // stdout/stderr so this list is
                                            // empty, but forwarding any
                                            // captured lines preserves the
                                            // CS-0035 fd-of-origin contract
                                            // for downstream observers.
                                            for (stream, line) in &work_result.output_lines {
                                                emit(
                                                    &event_tx,
                                                    EngineEvent::OutputLine {
                                                        recipe: work_node
                                                            .recipe_name
                                                            .clone(),
                                                        unit: id,
                                                        node_name: work_node
                                                            .payload
                                                            .as_ref()
                                                            .map(|p| p.display_name())
                                                            .unwrap_or_default(),
                                                        line: line.clone(),
                                                        stream: *stream,
                                                    },
                                                );
                                            }
                                            if work_result.success {
                                                Ok(())
                                            } else {
                                                Err(work_result.error.unwrap_or_else(
                                                    || "lua chunk failed".into(),
                                                ))
                                            }
                                        }
                                        Err(e) => Err(format!(
                                            "chore-body lua: pool channel closed: {e}"
                                        )),
                                    }
                                }
                                Err(e) => Err(e),
                            }
                        }
                        // The pre-walk only admits Interactive and LuaChunk
                        // members; anything else here is a structural bug.
                        // Be defensive and surface a clear diagnostic
                        // rather than silently advancing the DAG.
                        other => Err(format!(
                            "BUG: unexpected payload in chore window at step {}: {:?}",
                            idx0 + 1,
                            other
                        )),
                    };

                    if let Err(e) = result {
                        failed_idx = Some(idx0 + 1);
                        last_err = Some(e);
                        break;
                    }

                    // Advance the DAG. Window steps form a linear chain, so
                    // each `dag.complete` releases at most the next window
                    // step (which we'll process on the next iteration). Any
                    // out-of-chain dependents (rare for chore bodies, but
                    // possible if someone declares deps on a chore step)
                    // route through `process_ready` as usual.
                    let newly_ready = dag.complete(id);
                    for nid in newly_ready {
                        let already_in_window = window.contains(&nid);
                        if already_in_window {
                            continue;
                        }
                        pending += process_ready(
                            &dag,
                            nid,
                            &pool,
                            &mut cancelled,
                            &mut finished,
                            &mut interactive_queue,
                            &event_tx,
                            &mut recipe_trackers,
                            &cache_managers,
                            &cache_ctx,
                            &mut failures,
                            test_cache,
                            &mut cached_test_results,
                            rerun_patterns,
                            &mut blocked_results,
                            probe_units_by_node,
                            &mut upstream_probe_fingerprints,
                            &mut probe_fingerprint_by_node,
                            &mut keyless_probes,
                            published,
                        );
                    }
                }

                let chore_elapsed = chore_start.elapsed();
                let attempted = failed_idx.unwrap_or(n);
                finished += attempted;

                // Compute terminality before mutating cancellation state.
                // Terminal = no more queued/in-flight work and every
                // window-node dependent is either cancelled or part of
                // the window itself (already run by this same drain).
                let window_set: std::collections::BTreeSet<usize> =
                    window.iter().copied().collect();
                let is_terminal = interactive_queue.is_empty()
                    && pending == 0
                    && window.iter().all(|&id| {
                        dag.node(id).dependents().iter().all(|&d| {
                            cancelled[d] || window_set.contains(&d)
                        })
                    });

                // InteractiveEnd MUST precede the recipe-tracker ticks so
                // a terminal-chore renderer can set its suppression flag
                // before RecipeCompleted (and the global Finished) arrive.
                // The cargo-run shape is: chore body output, then nothing.
                emit(
                    &event_tx,
                    EngineEvent::InteractiveEnd {
                        recipe: chore_recipe.clone(),
                        node_name: head_node_name.clone(),
                        elapsed: chore_elapsed,
                        success: failed_idx.is_none(),
                        is_terminal,
                        failed_step: failed_idx,
                    },
                );

                // Account for steps in the recipe tracker: successful steps
                // tick `completed_nodes` without failure; the failing step
                // (if any) ticks with failure=true; the untouched tail is
                // marked cancelled below.
                let success_count = match failed_idx {
                    Some(k) => k - 1,
                    None => n,
                };
                for _ in 0..success_count {
                    finish_recipe_node(
                        &mut recipe_trackers,
                        &chore_recipe,
                        false,
                        false,
                        &event_tx,
                    );
                }
                if failed_idx.is_some() {
                    finish_recipe_node(
                        &mut recipe_trackers,
                        &chore_recipe,
                        false,
                        true,
                        &event_tx,
                    );
                }

                if let Some(k) = failed_idx {
                    // Cancel the untouched tail of the window.
                    for &skipped_id in &window[k..] {
                        if !cancelled[skipped_id] {
                            cancelled[skipped_id] = true;
                            finished += 1;
                        }
                    }

                    let err_msg = last_err.unwrap_or_else(|| "unknown".into());
                    let summary = format!("step {}/{}: {}", k, n, progress_error(&err_msg));
                    emit(
                        &event_tx,
                        EngineEvent::NodeFailed {
                            recipe: chore_recipe.clone(),
                            unit: window[k - 1],
                            node_name: chore_recipe.clone(),
                            elapsed: chore_elapsed,
                            error: summary.clone(),
                        },
                    );
                    failures.push((
                        window[k - 1],
                        chore_recipe.clone(),
                        format!("step {}/{}: {}", k, n, err_msg),
                    ));

                    // Cascade cancellation through any dependents of the
                    // failing step and the skipped tail.
                    for &id in &window[(k - 1)..] {
                        let dependents: Vec<usize> = dag.node(id).dependents().to_vec();
                        for dep_id in dependents {
                            cancel_subtree(
                                &dag,
                                dep_id,
                                &mut cancelled,
                                &event_tx,
                                &mut recipe_trackers,
                                &chore_recipe,
                                &mut blocked_results,
                            );
                        }
                    }
                }
            } else {
                // -------- LEGACY PATH: per-node interactive (unchanged) --------
                let id = interactive_queue.remove(0);
                if cancelled[id] {
                    finished += 1;
                    continue;
                }
                let node = dag.node(id);
                let work_node = node.payload();
                if let Some(payload @ WorkPayload::Interactive { cmd, line, .. }) =
                    &work_node.payload
                {
                    let recipe_name = work_node.recipe_name.clone();
                    let node_name = payload.display_name();
                    ensure_recipe_started(&mut recipe_trackers, &recipe_name, &event_tx);

                    // InteractiveStart is emitted BEFORE NodeStarted so the renderer can
                    // freeze/clear the progress bars before any repaint triggered by the
                    // node's arrival into the build state.
                    emit(
                        &event_tx,
                        EngineEvent::InteractiveStart {
                            recipe: recipe_name.clone(),
                            node_name: node_name.clone(),
                            chore_step_count: 0, // 0 = legacy non-chore single-line path
                        },
                    );
                    emit(
                        &event_tx,
                        EngineEvent::NodeStarted {
                            recipe: recipe_name.clone(),
                            unit: id,
                            node_name: node_name.clone(),
                            artifact: work_node.cache_meta.as_ref()
                                .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
                            fallback_label: node_name.clone(),
                            // Interactive payloads (@-shell) are never test steps,
                            // so default to Cooked.
                            kind: NodeKind::Cooked,
                            cause: None,
                            cache_key: node_cache_key(work_node),
                        },
                    );
                    let interactive_start = Instant::now();

                    // CS-0050: ensure parent dirs of declared cook-step outputs
                    // exist before the shell text runs. Interactive units today
                    // have `cache_meta == None` (the `interactive = true` flag is
                    // only set by `@`-prefixed shell steps which never declare
                    // outputs), but the call is uniform across dispatch paths so
                    // any future cook-style interactive variant inherits the
                    // contract.
                    let result = match ensure_output_parent_dirs(work_node) {
                        Ok(()) => run_interactive_on_main(
                            cmd,
                            *line,
                            &work_node.working_dir,
                            &work_node.env_vars,
                        ),
                        Err(e) => Err(e),
                    };
                    let interactive_elapsed = interactive_start.elapsed();
                    finished += 1;

                    // Terminal = no more queued interactives and this node has no
                    // (live) dependents, so after it completes the build will end.
                    let is_terminal = interactive_queue.is_empty()
                        && dag.node(id).dependents().iter().all(|&d| cancelled[d]);

                    let success = result.is_ok();
                    emit(
                        &event_tx,
                        EngineEvent::InteractiveEnd {
                            recipe: recipe_name.clone(),
                            node_name: node_name.clone(),
                            elapsed: interactive_elapsed,
                            success,
                            is_terminal,
                            failed_step: None,
                        },
                    );

                    if success {
                        emit(
                            &event_tx,
                            EngineEvent::NodeCompleted {
                                recipe: recipe_name.clone(),
                                unit: id,
                                node_name: node_name.clone(),
                                elapsed: interactive_elapsed,
                                // Interactive nodes are never test steps.
                                kind: NodeKind::Cooked,
                                cache_key: node_cache_key(work_node),
                            },
                        );

                        // Update cache if needed (C1: single-source publish path).
                        if let Some(meta) = &dag.node(id).payload().cache_meta {
                            if let Some(cm) = cache_managers.get(&dag.node(id).payload().recipe_name) {
                                let working_dir = dag.node(id).payload().working_dir.clone();
                                publish_completion(
                                    cm,
                                    meta,
                                    &working_dir,
                                    &pool.probe_value_store(),
                                    &cache_ctx,
                                    published,
                                );
                            }
                        }

                        finish_recipe_node(
                            &mut recipe_trackers,
                            &recipe_name,
                            false,
                            false,
                            &event_tx,
                        );

                        let newly_ready = dag.complete(id);
                        for nid in newly_ready {
                            pending += process_ready(
                                &dag,
                                nid,
                                &pool,
                                &mut cancelled,
                                &mut finished,
                                &mut interactive_queue,
                                &event_tx,
                                &mut recipe_trackers,
                                &cache_managers,
                                &cache_ctx,
                                &mut failures,
                                test_cache,
                                &mut cached_test_results,
                                rerun_patterns,
                                &mut blocked_results,
                                probe_units_by_node,
                                &mut upstream_probe_fingerprints,
                                &mut probe_fingerprint_by_node,
                                &mut keyless_probes,
                                published,
                            );
                        }
                    } else {
                        let err_msg = result.unwrap_err();
                        emit(
                            &event_tx,
                            EngineEvent::NodeFailed {
                                recipe: recipe_name.clone(),
                                unit: id,
                                node_name: node_name.clone(),
                                elapsed: interactive_elapsed,
                                error: progress_error(&err_msg),
                            },
                        );
                        failures.push((id, recipe_name.clone(), err_msg));
                        finish_recipe_node(
                            &mut recipe_trackers,
                            &recipe_name,
                            false,
                            true,
                            &event_tx,
                        );
                        for &dep_id in dag.node(id).dependents() {
                            cancel_subtree(
                                &dag,
                                dep_id,
                                &mut cancelled,
                                &event_tx,
                                &mut recipe_trackers,
                                &node_name,
                                &mut blocked_results,
                            );
                        }
                    }
                }
            }
        }

        // If nothing left, break.
        if pending == 0 && interactive_queue.is_empty() {
            break;
        }

        // Wait for pool results.
        let result = rx.recv().expect("worker channel closed unexpectedly");
        pending -= 1;
        finished += 1;

        // G3/G5 (CS-0074): a result carrying probe output completes the probe.
        // Publishing and materialising are `cook_probe::eval::record` — the
        // same call the no-worker paths at dispatch make, so a probe's value
        // reaches the store and `.cook/probes/` by one route however it was
        // produced (COOK-359). What stays here is the scheduler's share:
        // propagating the fingerprint and populating the per-run store.
        if let Some(ref probe_out) = result.probe_output {
            if result.success {
                match probe_fingerprint_by_node.get(&result.id) {
                    Some(&fp) => {
                        // Populate upstream_fingerprints for downstream probes.
                        upstream_probe_fingerprints.insert(probe_out.key.clone(), fp);

                        let working_dir = dag.node(result.id).payload().working_dir.clone();
                        let eval_ctx = cook_probe::eval::EvalCtx {
                            working_dir: &working_dir,
                            cache: Some(cook_probe::eval::CacheAccess {
                                backend: cache_ctx.backend.as_ref(),
                                project_root: &cache_ctx.project_root,
                                publish_enabled: cache_ctx.publish_enabled,
                            }),
                        };
                        // CS-0178 keylessness and COOK-168 publish suppression
                        // are decided inside `record`; the counter follows what
                        // it reports rather than re-deriving the condition.
                        let recorded = cook_probe::eval::record(
                            &probe_out.key,
                            &eval_ctx,
                            &fp,
                            keyless_probes.contains(&probe_out.key),
                            &probe_out.bytes,
                            cook_probe::eval::ValueSource::Produced,
                        );
                        for w in &recorded.warnings {
                            tracing::warn!("{w}");
                        }
                        if recorded.published {
                            published.fetch_add(1, Ordering::Relaxed);
                            tracing::debug!(
                                "probe '{}': cached output (fp={:x?})",
                                probe_out.key, &fp[..4],
                            );
                        }
                    }
                    None => {
                        // No fingerprint was computed at dispatch time (probe_units_by_node
                        // had no entry for this node — caching disabled for this probe).
                        // Still populate upstream_fingerprints with a sentinel so
                        // downstream probes that `requires` this probe don't error.
                        // We use a zero fingerprint as "unfingerprinted" (not cacheable).
                        // Downstream probes that consume it will include this sentinel,
                        // which means they too will be un-cacheable if they rely on it.
                        // This is acceptable: the missing-metadata path is the "no probe
                        // data available" edge case (tests, non-run.rs callers).
                        upstream_probe_fingerprints.insert(probe_out.key.clone(), [0u8; 32]);
                    }
                }
            }
            // The per-run store is populated whether or not the unit succeeded,
            // matching the pre-existing G3 ordering.
            pool.probe_value_store()
                .insert(&probe_out.key, probe_out.bytes.clone());
        }

        let node = dag.node(result.id);
        let work_node = node.payload();
        let recipe_name = work_node.recipe_name.clone();

        if result.success {
            // Emit output lines.  Each captured line carries its fd-of-origin
            // (CS-0035) so the OutputLine event reflects stdout vs stderr
            // honestly instead of hardcoding stdout.
            for (stream, line) in &result.output_lines {
                emit(
                    &event_tx,
                    EngineEvent::OutputLine {
                        recipe: recipe_name.clone(),
                        unit: result.id,
                        node_name: result.node_name.clone(),
                        line: line.clone(),
                        stream: *stream,
                    },
                );
            }

            emit(
                &event_tx,
                EngineEvent::NodeCompleted {
                    recipe: recipe_name.clone(),
                    unit: result.id,
                    node_name: result.node_name.clone(),
                    // Real per-unit wall time measured by the worker around
                    // execution (queue wait excluded) — see
                    // `WorkResult::duration` in cook-luaotp/src/pool.rs.
                    elapsed: result.duration,
                    kind: work_node
                        .payload
                        .as_ref()
                        .map(node_kind_for_payload)
                        .unwrap_or(NodeKind::Cooked),
                    cache_key: node_cache_key(work_node),
                },
            );

            // Update cache entry if this node has cache metadata
            // (C1: single-source publish path).
            if let Some(meta) = &dag.node(result.id).payload().cache_meta {
                if let Some(cm) = cache_managers.get(&dag.node(result.id).payload().recipe_name) {
                    let working_dir = dag.node(result.id).payload().working_dir.clone();
                    publish_completion(
                        cm,
                        meta,
                        &working_dir,
                        &pool.probe_value_store(),
                        &cache_ctx,
                        published,
                    );
                }
            }

            finish_recipe_node(&mut recipe_trackers, &recipe_name, false, false, &event_tx);

            // Translate test output to a TestResult and emit TestPassed event.
            if let Some(to) = result.test_output {
                // Build the TestId in `<recipe>:<test_name>[<item>]` format so the
                // reporter can extract the recipe portion. `result.node_name`
                // is the raw display_name (= test_name alone); `recipe_name`
                // carries the fully-qualified recipe name.
                // COOK-211: recompute the same ready-time content fingerprint
                // used for the lookup, so the cache entry is written under the
                // key a future run will recompute. Predecessor outputs are still
                // materialised (the test's execution does not touch them).
                let fp_opt = crate::run::compute_ready_test_fingerprint(&dag, result.id, &cache_ctx, &pool.probe_value_store());
                let (line_no, iteration_item_opt) = match &dag.node(result.id).payload().payload {
                    Some(WorkPayload::Test { line, iteration_item, .. }) => (*line as u32, iteration_item.clone()),
                    _ => (0, None),
                };
                let id_str = match &iteration_item_opt {
                    Some(item) if !item.is_empty() => format!("{}:{}[{}]", recipe_name, to.test_name, item),
                    _ => format!("{}:{}", recipe_name, to.test_name),
                };
                let id = crate::id::parse_test_id(&id_str);
                let namespace = crate::id::id_namespace(&id);
                let recipe = crate::id::id_recipe(&id);
                let duration = Duration::from_secs_f64(to.duration);
                emit(
                    &event_tx,
                    EngineEvent::TestPassed {
                        id: id.clone(),
                        duration,
                        cached: false,
                        should_fail: to.should_fail,
                        stdout: to.stdout.clone(),
                        stderr: to.stderr.clone(),
                        line: line_no,
                    },
                );
                test_results.push(crate::TestResult {
                    id,
                    namespace,
                    recipe,
                    name: to.test_name.clone(),
                    suite: to.suite_name.clone(),
                    iteration_item: iteration_item_opt,
                    outcome: crate::TestOutcome::Passed,
                    duration,
                    from_cache: false,
                    stdout: to.stdout.clone(),
                    stderr: to.stderr.clone(),
                    fingerprint: fp_opt.clone(),
                    blocked_by: None,
                    should_fail: to.should_fail,
                    timed_out: false,
                    line: line_no,
                    exit_code: to.exit_code,
                });

                // Write passing test result to the content-addressed cache.
                if let (Some(tc), Some(fp)) = (test_cache, fp_opt) {
                    let entry = TestCacheEntry {
                        // COOK-360: one shared version, not this store's private 1.
                        schema_version: cook_fingerprint::CACHE_VERSION,
                        fingerprint: fp.clone(),
                        outcome: TestCacheOutcome::Passed,
                        stdout: to.stdout.clone(),
                        stderr: to.stderr.clone(),
                        duration_secs: to.duration,
                        should_fail_observed: to.should_fail,
                        recorded_at: iso8601_now(),
                    };
                    if let Err(e) = tc.store(&fp, &entry) {
                        tracing::warn!("test cache write failed for {fp}: {e}");
                    }
                }
            }

            let newly_ready = dag.complete(result.id);
            for id in newly_ready {
                pending += process_ready(
                    &dag,
                    id,
                    &pool,
                    &mut cancelled,
                    &mut finished,
                    &mut interactive_queue,
                    &event_tx,
                    &mut recipe_trackers,
                    &cache_managers,
                    &cache_ctx,
                    &mut failures,
                    test_cache,
                    &mut cached_test_results,
                    rerun_patterns,
                    &mut blocked_results,
                    probe_units_by_node,
                    &mut upstream_probe_fingerprints,
                    &mut probe_fingerprint_by_node,
                    &mut keyless_probes,
                    published,
                );
            }
        } else {
            // Emit output lines even on failure (CS-0035 — stream tagged).
            for (stream, line) in &result.output_lines {
                emit(
                    &event_tx,
                    EngineEvent::OutputLine {
                        recipe: recipe_name.clone(),
                        unit: result.id,
                        node_name: result.node_name.clone(),
                        line: line.clone(),
                        stream: *stream,
                    },
                );
            }

            // Translate test output to a TestResult and emit TestFailed/TestTimedOut event.
            if let Some(ref to) = result.test_output {
                // Build the TestId in `<recipe>:<test_name>[<item>]` format (same as TestStarted).
                let (line_no, iteration_item_opt) = match &dag.node(result.id).payload().payload {
                    Some(WorkPayload::Test { line, iteration_item, .. }) => (*line as u32, iteration_item.clone()),
                    _ => (0, None),
                };
                let id_str = match &iteration_item_opt {
                    Some(item) if !item.is_empty() => format!("{}:{}[{}]", recipe_name, to.test_name, item),
                    _ => format!("{}:{}", recipe_name, to.test_name),
                };
                let id = crate::id::parse_test_id(&id_str);
                let namespace = crate::id::id_namespace(&id);
                let recipe = crate::id::id_recipe(&id);
                let duration = Duration::from_secs_f64(to.duration);
                let outcome = if to.timed_out {
                    emit(
                        &event_tx,
                        EngineEvent::TestTimedOut {
                            id: id.clone(),
                            timeout: duration,
                            stdout: to.stdout.clone(),
                            stderr: to.stderr.clone(),
                            line: line_no,
                        },
                    );
                    crate::TestOutcome::TimedOut
                } else {
                    emit(
                        &event_tx,
                        EngineEvent::TestFailed {
                            id: id.clone(),
                            duration,
                            stdout: to.stdout.clone(),
                            stderr: to.stderr.clone(),
                            reason: crate::TestFailureReason::ExitStatusMismatch {
                                expected_success: !to.should_fail,
                                observed_success: to.exit_success,
                                exit_code: to.exit_code,
                            },
                            line: line_no,
                        },
                    );
                    crate::TestOutcome::Failed
                };
                test_results.push(crate::TestResult {
                    id,
                    namespace,
                    recipe,
                    name: to.test_name.clone(),
                    suite: to.suite_name.clone(),
                    iteration_item: iteration_item_opt,
                    outcome,
                    duration,
                    from_cache: false,
                    stdout: to.stdout.clone(),
                    stderr: to.stderr.clone(),
                    fingerprint: None,
                    blocked_by: None,
                    should_fail: to.should_fail,
                    timed_out: to.timed_out,
                    line: line_no,
                    exit_code: to.exit_code,
                });
            }

            let err_msg = result
                .error
                .unwrap_or_else(|| "unknown error".to_string());

            // Test semantic failures (result.test_output.is_some()) stay "soft" in
            // the one sense that matters for exit accounting: the outcome is already
            // recorded in test_results as TestOutcome::Failed or TestOutcome::TimedOut
            // and drives the exit code through the test reporter, so it does NOT join
            // the hard `failures` list, which is for infrastructure errors (spawn
            // failures and the like).
            //
            // COOK-341: it is NOT soft for scheduling. A failed test cancels its
            // dependents exactly as a failed cook step does. Every other step kind
            // already worked this way — a failed `cook` emits `skipped
            // (upstream-failed)` for everything downstream — and the test kind's
            // exemption meant a dependent ran its body over a tree its own gate had
            // just rejected. `chore ship: checks` printed SHIPPED with `checks` red
            // and only then exited 1, which for a release chore means the tag is
            // already pushed by the time the failure is reported.
            //
            // Siblings are untouched, because a sibling is not a dependent: `cook
            // test` still runs and reports the whole suite rather than stopping at
            // the first red. Cancelled test units are reported as TestOutcome::Blocked
            // with `blocked_by` naming the cause — the outcome §17.4 rule 2 already
            // defines and forbids caching, previously reachable only when a *build*
            // dependency failed.
            //
            // Hard failures (test_output is None for a Test payload, or any non-Test
            // payload failure) go into `failures` as before and cancel dependents.
            let is_test_semantic_failure = result.test_output.is_some();
            if is_test_semantic_failure {
                // Soft failure: emit NodeFailed for observability but don't escalate.
                emit(
                    &event_tx,
                    EngineEvent::NodeFailed {
                        recipe: recipe_name.clone(),
                        unit: result.id,
                        node_name: result.node_name.clone(),
                        // Real per-unit wall time measured by the worker
                        // around execution (queue wait excluded) — see
                        // `WorkResult::duration` in cook-luaotp/src/pool.rs.
                        elapsed: result.duration,
                        error: progress_error(&err_msg),
                    },
                );
                finish_recipe_node(&mut recipe_trackers, &recipe_name, false, false, &event_tx);
                // COOK-341: cancel the dependent subtree before completing the node.
                // `cancel_subtree` marks each dependent in `cancelled`, which
                // `process_ready` consults, so the ordering matters: complete first
                // and a dependent could be dispatched in the same drain below.
                for &dep_id in dag.node(result.id).dependents() {
                    cancel_subtree(
                        &dag,
                        dep_id,
                        &mut cancelled,
                        &event_tx,
                        &mut recipe_trackers,
                        &result.node_name,
                        &mut blocked_results,
                    );
                }
                // Complete the node in the DAG so the drain proceeds; dependents
                // just cancelled are skipped by `process_ready`.
                let newly_ready = dag.complete(result.id);
                for id in newly_ready {
                    pending += process_ready(
                        &dag,
                        id,
                        &pool,
                        &mut cancelled,
                        &mut finished,
                        &mut interactive_queue,
                        &event_tx,
                        &mut recipe_trackers,
                        &cache_managers,
                        &cache_ctx,
                        &mut failures,
                        test_cache,
                        &mut cached_test_results,
                        rerun_patterns,
                        &mut blocked_results,
                        probe_units_by_node,
                        &mut upstream_probe_fingerprints,
                        &mut probe_fingerprint_by_node,
                        &mut keyless_probes,
                        published,
                    );
                }
            } else {
                // Hard failure: infrastructure error.
                emit(
                    &event_tx,
                    EngineEvent::NodeFailed {
                        recipe: recipe_name.clone(),
                        unit: result.id,
                        node_name: result.node_name.clone(),
                        // Real per-unit wall time measured by the worker
                        // around execution (queue wait excluded) — see
                        // `WorkResult::duration` in cook-luaotp/src/pool.rs.
                        elapsed: result.duration,
                        error: progress_error(&err_msg),
                    },
                );

                failures.push((result.id, recipe_name.clone(), err_msg));
                finish_recipe_node(&mut recipe_trackers, &recipe_name, false, true, &event_tx);

                for &dep_id in dag.node(result.id).dependents() {
                    cancel_subtree(
                        &dag,
                        dep_id,
                        &mut cancelled,
                        &event_tx,
                        &mut recipe_trackers,
                        &result.node_name,
                        &mut blocked_results,
                    );
                }
            }
        }
    }

    pool.shutdown();

    // Flush cache updates to disk
    for cm in cache_managers.values() {
        if let Err(e) = cm.flush_all() {
            tracing::warn!("recipe cache not persisted: {e}; next run will re-execute");
        }
    }

    if failures.is_empty() {
        // Merge cached_test_results (from test cache hits synthesized during
        // process_ready) with test_results (from actual executions).
        let mut all = cached_test_results;
        all.extend(test_results);
        // COOK-341: Blocked rows now also arise on this path. A failed test
        // cancels its dependents without joining the hard `failures` list, so
        // the run still returns Ok and these rows would otherwise be dropped —
        // the suite would report a dependent as neither run nor blocked, just
        // absent. Before COOK-341 `cancel_subtree` fired only for hard
        // failures, which always take the Err arm, so this was vacuously empty.
        all.extend(blocked_results);
        Ok(all)
    } else {
        // Build partial_test_results: everything accumulated so far (including
        // Blocked rows from cancel_subtree) so that run_for_test_inner can
        // return Ok with these rows instead of propagating the error.
        let mut partial = cached_test_results;
        partial.extend(test_results);
        partial.extend(blocked_results);
        Err(EngineError::TaskFailures {
            count: failures.len(),
            failures,
            partial_test_results: partial,
        })
    }
}

/// Single-source completion → record → cloud_key → artifact/depfile upload →
/// determinant-manifest path, shared by both completion sites (the restored /
/// interactive path and the freshly-executed worker path). The two call sites
/// differ only in which node id sources `working_dir`; everything below — the
/// `publish_to_backend` derivation, the `seal_contribution` recompute, the
/// upload loops, and the manifest write — lives here ONCE so the publish/upload
/// contract is single-source.
///
/// `working_dir` is the unit's resolved working directory; `meta` is the unit's
/// `CacheMeta`; `cm` is its recipe's cache manager; `probe_store` is the pool's
/// `ProbeValueStore`. Behaviour-preserving extraction of the two ~210-line
/// blocks (COOK-91 review C1).
fn publish_completion(
    cm: &ThreadSafeCacheManager,
    meta: &cook_contracts::CacheMeta,
    working_dir: &std::path::Path,
    probe_store: &cook_luaotp::ProbeValueStore,
    cache_ctx: &CacheContext,
    published: &AtomicU64,
) {
    // CS-0085 §17.6: expand any glob patterns in output_paths against the
    // unit's working directory before recording.
    let resolved_output_paths = resolve_output_paths(&meta.output_paths, working_dir);
    // CS-0119: directory-output orphans are handled by the build-owned pre-clean
    // that empties each declared `dir/` subtree immediately before the command
    // runs (see `execute_dag`), so by this point `resolved_output_paths` already
    // describes exactly what this invocation produced — no post-execute sweep is
    // needed here. Cache-hit reconciliation lives on the `RebuildResult::Skip`
    // path, which never reaches this publish function.
    let mut meta_for_record = meta.clone();
    meta_for_record.output_paths = resolved_output_paths.clone();
    // COOK-161: fold the effective seal set's probe values into the persisted
    // key (the sealed probes have run by now — the unit depends on them).
    let seal_contrib = crate::seal::seal_contribution(&meta.seal_keys, probe_store);
    let step_entry = match cm.record_completion(
        &meta.recipe_name,
        &meta.cache_key,
        &meta_for_record,
        working_dir,
        seal_contrib,
    ) {
        Ok(step_entry) => step_entry,
        Err(e) => {
            tracing::warn!(
                "cache: skipping record for {}::{}: {e}",
                meta.recipe_name,
                meta.cache_key
            );
            return;
        }
    };

    // Post-execution augmentation: parse the just-written depfile and append
    // discovered FileRecords to step_entry.inputs, then persist the augmented
    // entry.
    let mut step_entry = step_entry;
    if let Some(di) = &meta.discovered_inputs {
        let abs_depfile = working_dir.join(&di.from);
        let source_for_skip = meta.input_paths.first().map(String::as_str).unwrap_or("");
        match cook_cache::parse_make_depfile(&abs_depfile, source_for_skip, working_dir) {
            Ok(discovered_paths) => {
                match cook_cache::collect_records_public(&discovered_paths, working_dir) {
                    Ok(records) => {
                        for rec in records {
                            step_entry.inputs.push(rec);
                        }
                        // clone: step_entry.inputs is borrowed below for cloud_key composition.
                        cm.update_step(&meta.recipe_name, &meta.cache_key, step_entry.clone());
                    }
                    Err(p) => {
                        tracing::warn!(
                            "discovered-inputs: failed to hash discovered path '{}'",
                            p
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "discovered-inputs: depfile parse failed for '{}': {e}",
                    di.from
                );
            }
        }
    }

    // Compute cloud_key for this unit (spec §5.3).
    let mut sorted_hashes: Vec<u64> = step_entry.inputs.iter().map(|fr| fr.hash).collect();
    sorted_hashes.sort();
    let recipe_namespace =
        recipe_namespace(&meta.project_id, &meta.cookfile_path, &meta.recipe_name);
    let cloud_k = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: &recipe_namespace,
        command_hash: meta.command_hash,
        env_contribution: meta.env_contribution,
        seal_contribution: seal_contrib,
        sorted_input_content_hashes: &sorted_hashes,
    });

    // COOK-162 §3: `local` units never publish to the shared store.
    // COOK-168: publish-off / read-only client mode suppresses ALL uploads
    // globally; fetch-by-key is unaffected.
    let publish_to_backend = !meta.sharing.is_local() && cache_ctx.publish_enabled;
    // Coarse zero / non-zero gate for the end-of-run CAS budget check: this
    // unit is about to write at least a determinant manifest to the store.
    if publish_to_backend {
        published.fetch_add(1, Ordering::Relaxed);
    }

    // Upload one artifact per declared output (2026-05-02 addendum spec §5.1).
    // Each artifact is keyed by artifact_key(cloud_key, idx, path) so a future
    // cache hit can restore them all independently.
    // CS-0085: iterate the resolved (glob-expanded) list.
    for (out_idx, output_path) in resolved_output_paths.iter().enumerate() {
        let abs_output = working_dir.join(output_path);
        // COOK-180: classify each output via symlink_metadata (does NOT follow
        // links) so per-file fidelity round-trips: a regular file stores its real
        // bytes + mode; a symlink stores no content, just kind+target; a dir
        // stores an empty marker. The restore side (restore_one) dispatches on
        // `kind`, so the empty body for symlink/dir kinds is intentional.
        let lstat = match std::fs::symlink_metadata(&abs_output) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let ft = lstat.file_type();
        #[cfg(unix)]
        let mode = std::os::unix::fs::PermissionsExt::mode(&lstat.permissions());
        #[cfg(not(unix))]
        let mode = 0o644u32;
        let (body, kind, target): (Vec<u8>, Option<String>, Option<String>) =
            if ft.is_symlink() {
                let t = std::fs::read_link(&abs_output)
                    .ok()
                    .and_then(|p| p.to_str().map(String::from));
                // A symlink whose target isn't valid UTF-8 can't be recorded — skip it.
                match t {
                    Some(t) => (Vec::new(), Some("symlink".to_string()), Some(t)),
                    None => continue,
                }
            } else if ft.is_dir() {
                (Vec::new(), Some("dir".to_string()), None)
            } else {
                match std::fs::read(&abs_output) {
                    Ok(b) => (b, None, None),
                    Err(_) => continue,
                }
            };
        let artifact_k = artifact_key(&cloud_k, out_idx as u32, output_path);
        let mut artifact_meta = ArtifactMeta {
            recipe_namespace: recipe_namespace.clone(),
            command_hash: meta.command_hash,
            env_contribution: meta.env_contribution,
            seal_contribution: seal_contrib,
            schema_version: CACHE_VERSION,
            size_bytes: body.len() as u64,
            tags: std::collections::BTreeSet::new(),
            consulted_env_keys: meta.consulted_env.keys().cloned().collect(),
            output_index: out_idx as u32,
            output_path: output_path.clone(),
            // CS-0054: stamped by the backend on put.
            content_hash: ArtifactMeta::zero_content_hash(),
            kind,
            mode,
            target,
        };
        if publish_to_backend {
            if let Err(e) = cook_cache::backend::put_bytes(
                cache_ctx.backend.as_ref(),
                &artifact_k,
                &body,
                &mut artifact_meta,
            ) {
                tracing::warn!("cache backend put failed for {}: {}", output_path, e);
            }
        }
    }

    // Upload the depfile as an implicit artifact at index outputs.len() so a
    // future restore can pull it back.
    // CS-0085: depfile_idx uses the resolved count to match the index
    // record_completion appended it at.
    if let Some(di) = &meta.discovered_inputs {
        let depfile_idx = resolved_output_paths.len() as u32;
        let abs_depfile = working_dir.join(&di.from);
        match std::fs::read(&abs_depfile) {
            Ok(bytes) => {
                let artifact_k = artifact_key(&cloud_k, depfile_idx, &di.from);
                let mut artifact_meta = ArtifactMeta {
                    recipe_namespace: recipe_namespace.clone(),
                    command_hash: meta.command_hash,
                    env_contribution: meta.env_contribution,
                    seal_contribution: seal_contrib,
                    schema_version: CACHE_VERSION,
                    size_bytes: bytes.len() as u64,
                    tags: std::collections::BTreeSet::new(),
                    consulted_env_keys: meta.consulted_env.keys().cloned().collect(),
                    output_index: depfile_idx,
                    output_path: di.from.clone(),
                    // CS-0054: stamped by the backend on put.
                    content_hash: ArtifactMeta::zero_content_hash(),
                    kind: None,
                    mode: ArtifactMeta::default_mode(),
                    target: None,
                };
                if publish_to_backend {
                    if let Err(e) = cook_cache::backend::put_bytes(
                        cache_ctx.backend.as_ref(),
                        &artifact_k,
                        &bytes,
                        &mut artifact_meta,
                    ) {
                        tracing::warn!(
                            "cache backend put failed for depfile {}: {e}",
                            di.from
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "discovered-inputs: depfile '{}' not found after execution: {e}",
                    di.from
                );
            }
        }

        // COOK-177: publish the discovered-input PATH LIST under a DECLARED-inputs-only
        // key so a cold consumer can recover the full key without having a depfile.
        // SOUND: every listed path's content is folded into the full key, so a stale
        // manifest can only cause a safe miss, never a wrong hit. This artifact is NOT
        // recorded in step_entry.outputs — it is fetched out-of-band on the cold path.
        if publish_to_backend {
            let declared_refs: Vec<&str> =
                meta.input_paths.iter().map(|s| s.as_str()).collect();
            if let Some(declared_hashes) =
                cook_fingerprint::hash_input_paths(&declared_refs, working_dir)
            {
                let declared_key = cloud_key(&CloudKeyInputs {
                    schema_version: CACHE_VERSION,
                    recipe_namespace: &recipe_namespace,
                    command_hash: meta.command_hash,
                    env_contribution: meta.env_contribution,
                    seal_contribution: seal_contrib,
                    sorted_input_content_hashes: &declared_hashes,
                });
                // Parse the discovered relative paths the SAME way the warm path does.
                let source_for_skip =
                    meta.input_paths.first().map(String::as_str).unwrap_or("");
                let discovered_paths: Vec<String> = cook_cache::parse_make_depfile(
                    &working_dir.join(&di.from),
                    source_for_skip,
                    working_dir,
                )
                .unwrap_or_default();
                let json = serde_json::to_vec(&discovered_paths).unwrap_or_default();
                let manifest_k = artifact_key(
                    &declared_key,
                    cook_fingerprint::DISCOVERED_INPUTS_MANIFEST_INDEX,
                    cook_fingerprint::DISCOVERED_INPUTS_MANIFEST_PATH,
                );
                let mut manifest_meta = ArtifactMeta {
                    recipe_namespace: recipe_namespace.clone(),
                    command_hash: meta.command_hash,
                    env_contribution: meta.env_contribution,
                    seal_contribution: seal_contrib,
                    schema_version: CACHE_VERSION,
                    size_bytes: json.len() as u64,
                    tags: std::collections::BTreeSet::new(),
                    consulted_env_keys: meta.consulted_env.keys().cloned().collect(),
                    output_index: cook_fingerprint::DISCOVERED_INPUTS_MANIFEST_INDEX,
                    output_path: cook_fingerprint::DISCOVERED_INPUTS_MANIFEST_PATH
                        .to_string(),
                    // CS-0054: stamped by the backend on put.
                    content_hash: ArtifactMeta::zero_content_hash(),
                    kind: Some("discovered_inputs".to_string()),
                    mode: 0o644,
                    target: None,
                };
                if let Err(e) = cook_cache::backend::put_bytes(
                    cache_ctx.backend.as_ref(),
                    &manifest_k,
                    &json,
                    &mut manifest_meta,
                ) {
                    tracing::warn!(
                        "cache backend put failed for discovered-inputs manifest: {}",
                        e
                    );
                }

                // COOK-278: additionally maintain the multi-set manifest under
                // its own reserved key. The single-set artifact above is
                // last-writer-wins, so an edit that changes the discovered SET
                // erases the older set and breaks revert-restore; this one
                // accumulates every distinct set seen for the declared key
                // (newest first, capped). The v1 artifact keeps being written
                // so pre-COOK-278 binaries sharing the store lose nothing.
                let mut sets = cook_fingerprint::read_discovered_input_sets(
                    cache_ctx.backend.as_ref(),
                    &declared_key,
                );
                sets.retain(|s| *s != discovered_paths);
                sets.insert(0, discovered_paths);
                sets.truncate(cook_fingerprint::DISCOVERED_INPUT_SETS_CAP);
                let sets_json = serde_json::to_vec(&sets).unwrap_or_default();
                let sets_k = artifact_key(
                    &declared_key,
                    cook_fingerprint::DISCOVERED_INPUT_SETS_INDEX,
                    cook_fingerprint::DISCOVERED_INPUT_SETS_PATH,
                );
                let mut sets_meta = ArtifactMeta {
                    recipe_namespace: recipe_namespace.clone(),
                    command_hash: meta.command_hash,
                    env_contribution: meta.env_contribution,
                    seal_contribution: seal_contrib,
                    schema_version: CACHE_VERSION,
                    size_bytes: sets_json.len() as u64,
                    tags: std::collections::BTreeSet::new(),
                    consulted_env_keys: meta.consulted_env.keys().cloned().collect(),
                    output_index: cook_fingerprint::DISCOVERED_INPUT_SETS_INDEX,
                    output_path: cook_fingerprint::DISCOVERED_INPUT_SETS_PATH.to_string(),
                    // CS-0054: stamped by the backend on put.
                    content_hash: ArtifactMeta::zero_content_hash(),
                    kind: Some("discovered_input_sets".to_string()),
                    mode: 0o644,
                    target: None,
                };
                if let Err(e) = cook_cache::backend::put_bytes(
                    cache_ctx.backend.as_ref(),
                    &sets_k,
                    &sets_json,
                    &mut sets_meta,
                ) {
                    tracing::warn!(
                        "cache backend put failed for discovered-input sets manifest: {}",
                        e
                    );
                }
            }
        }
    }

    // COOK-180: record empty directories declared by `dir/` outputs so a cache
    // hit is byte-identical to a miss. resolve_output_paths only yields FILES
    // (glob expansion drops dirs), so genuinely-empty subdirectories under a
    // directory output would otherwise be lost.
    //
    // INDEX BOOKKEEPING: these dir records CANNOT go through record_completion —
    // its collect_records hashes file bytes and errors (UnreadableFile) on a
    // directory, which would abort the whole unit's record/publish. So we append
    // the dir FileRecords to step_entry.outputs directly (AFTER the file outputs
    // and the implicit depfile output that record_completion already appended)
    // and publish their artifacts at the matching trailing indices. The depfile
    // index is therefore unchanged. Restore alignment holds because a `dir/`
    // output makes the unit a terminal-output unit, and the cache-hit path
    // derives current_outputs straight from the persisted StepEntry.outputs — so
    // these appended (index, path) pairs are exactly what try_restore fetches.
    let mut empty_dir_paths: Vec<String> = Vec::new();
    for entry in &meta.output_paths {
        if let Some(root) = entry.strip_suffix('/') {
            for ed in cook_fingerprint::empty_dirs_under(working_dir, root) {
                empty_dir_paths.push(ed);
            }
        }
    }
    empty_dir_paths.sort();
    empty_dir_paths.dedup();
    if !empty_dir_paths.is_empty() {
        let mut next_idx = step_entry.outputs.len() as u32;
        for ed in &empty_dir_paths {
            let abs_ed = working_dir.join(ed);
            #[cfg(unix)]
            let mode = std::fs::symlink_metadata(&abs_ed)
                .ok()
                .map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()))
                .unwrap_or(0o755);
            #[cfg(not(unix))]
            let mode = 0o755u32;
            // Persist the dir as an implicit output so the cache-hit path (which
            // derives current_outputs from StepEntry.outputs for terminal-output
            // units) fetches it at this exact index. The hash is irrelevant for a
            // dir: restore_one's "dir" branch ignores the body/hash, and the
            // cloud_key keys on INPUT hashes only.
            step_entry.outputs.push(cook_fingerprint::FileRecord {
                path: ed.as_str().into(),
                mtime: cook_fingerprint::stat_mtime(&abs_ed).unwrap_or(0),
                hash: 0,
            });
            let artifact_k = artifact_key(&cloud_k, next_idx, ed);
            let mut artifact_meta = ArtifactMeta {
                recipe_namespace: recipe_namespace.clone(),
                command_hash: meta.command_hash,
                env_contribution: meta.env_contribution,
                seal_contribution: seal_contrib,
                schema_version: CACHE_VERSION,
                size_bytes: 0,
                tags: std::collections::BTreeSet::new(),
                consulted_env_keys: meta.consulted_env.keys().cloned().collect(),
                output_index: next_idx,
                output_path: ed.clone(),
                // CS-0054: stamped by the backend on put.
                content_hash: ArtifactMeta::zero_content_hash(),
                kind: Some("dir".to_string()),
                mode,
                target: None,
            };
            if publish_to_backend {
                if let Err(e) = cook_cache::backend::put_bytes(
                    cache_ctx.backend.as_ref(),
                    &artifact_k,
                    b"",
                    &mut artifact_meta,
                ) {
                    tracing::warn!("cache backend put failed for empty dir {}: {}", ed, e);
                }
            }
            next_idx += 1;
        }
        // Persist the augmented outputs so the restore path knows to fetch the
        // empty dirs at these indices.
        cm.update_step(&meta.recipe_name, &meta.cache_key, step_entry.clone());
    }

    // COOK-166: persist the producer determinant manifest alongside the shared
    // artifacts, keyed by the unit's cloud_key K. `local` units skip this
    // (publish_to_backend is false).
    if publish_to_backend {
        let manifest = build_determinant_manifest(
            CACHE_VERSION,
            &recipe_namespace,
            &cloud_k,
            meta.command_hash,
            meta.env_contribution,
            seal_contrib,
            &step_entry.inputs,
            &resolved_output_paths,
            &empty_dir_paths,
            &meta.consulted_env,
            &meta.seal_keys,
            probe_store,
        );
        if let Err(e) = cache_ctx.backend.as_ref().put_manifest(&cloud_k, &manifest) {
            tracing::warn!("cache manifest put failed for {recipe_namespace}: {e}");
        }
    }
}

/// COOK-166: build the producer determinant manifest from the resolved values
/// the publish site already holds. `key` is the unit's `cloud_key` (K). The
/// `inputs` slice is `step_entry.inputs` (post depfile-discovery augmentation);
/// `sealed` probe values are read from the `ProbeValueStore` for each key in
/// the effective seal set, decoded as UTF-8 canonical JSON (lossy decode guards
/// the theoretically-impossible non-UTF-8 case — probe values are canonical JSON).
#[allow(clippy::too_many_arguments)]
fn build_determinant_manifest(
    schema_version: u32,
    recipe_namespace: &str,
    key: &[u8; 32],
    command_hash: u64,
    env_contribution: u64,
    seal_contribution: u64,
    inputs: &[cook_fingerprint::FileRecord],
    output_paths: &[String],
    empty_dir_outputs: &[String],
    consulted_env: &std::collections::BTreeMap<String, String>,
    seal_keys: &std::collections::BTreeSet<String>,
    probe_store: &cook_luaotp::ProbeValueStore,
) -> DeterminantManifest {
    let inputs_map: std::collections::BTreeMap<String, u64> =
        inputs.iter().map(|fr| (fr.path.to_string(), fr.hash)).collect();
    // C2: single-source the sealed-probe resolution (absent → empty string)
    // so producer and `cook why` consumer cannot drift.
    let sealed_probes = crate::seal::resolve_sealed_probes(seal_keys, probe_store);
    DeterminantManifest {
        schema_version,
        recipe_namespace: recipe_namespace.to_string(),
        key: hex::encode(key),
        command_hash,
        env_contribution,
        seal_contribution,
        inputs: inputs_map,
        output_paths: output_paths.to_vec(),
        empty_dir_outputs: empty_dir_outputs.to_vec(),
        consulted_env: consulted_env.clone(),
        sealed_probes,
    }
}

#[cfg(test)]
#[path = "tests/executor_tests.rs"]
mod tests;
