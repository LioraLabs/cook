//! DAG execution loop.
//!
//! Executes all nodes in a `Dag<WorkNode>` respecting dependency order.
//! Pre-satisfied (cached) nodes are completed immediately. Real work nodes
//! are dispatched to the `cook_luaotp::WorkerPool`. Interactive nodes are
//! queued and run on the main thread after the pool drains.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cook_cache::{CacheContext, TestCache, TestCacheEntry, TestCacheOutcome, ThreadSafeCacheManager};
use cook_contracts::WorkPayload;
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

fn normalize_glob_pattern(pattern: &str) -> std::borrow::Cow<'_, str> {
    if pattern == "**" {
        std::borrow::Cow::Borrowed("**/*")
    } else if let Some(prefix) = pattern.strip_suffix("/**") {
        std::borrow::Cow::Owned(format!("{prefix}/**/*"))
    } else if let Some(prefix) = pattern.strip_suffix('/') {
        // CS-0119: a directory output `dir/` owns the whole subtree.
        std::borrow::Cow::Owned(format!("{prefix}/**/*"))
    } else {
        std::borrow::Cow::Borrowed(pattern)
    }
}

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
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(declared.len());
    for entry in declared {
        if cook_fingerprint::is_terminal_output(entry) {
            let normalized = normalize_glob_pattern(entry);
            for resolved in cook_fingerprint::resolve_glob(working_dir, normalized.as_ref()) {
                if seen.insert(resolved.clone()) {
                    out.push(resolved);
                }
            }
        } else if seen.insert(entry.clone()) {
            out.push(entry.clone());
        }
    }
    out
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

    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(working_dir)
        .envs(&child_env)
        .status()
        .map_err(|e| format!("failed to execute: {e}"))?;

    if !status.success() {
        let code = status.code().unwrap_or(1);
        return Err(format!("COOK_CMD_FAILED:{}:{}:{}", line, code, cmd));
    }

    Ok(())
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
/// `fingerprint_by_node` — maps dag node id → pre-computed test fingerprint.
/// Only node ids whose `WorkPayload` is `Test` need entries; other ids are
/// ignored. When empty or `None` for a given node, caching is skipped for
/// that node.
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
pub fn execute_dag(
    dag: Dag<WorkNode>,
    num_workers: usize,
    cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    event_tx: Option<mpsc::Sender<EngineEvent>>,
    cache_ctx: Arc<CacheContext>,
    test_cache: Option<&TestCache>,
    fingerprint_by_node: &BTreeMap<usize, String>,
    rerun_patterns: &[String],
    probe_units_by_node: &BTreeMap<usize, cook_contracts::ProbeUnit>,
    dep_outputs: cook_luaotp::WorkerDepOutputs,
) -> Result<Vec<crate::TestResult>, EngineError> {
    // Install the depfile parser pointer so cook-fingerprint's pre-check
    // augmentation can call back into cook-cache without a runtime dep cycle.
    {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            cook_fingerprint::install_depfile_parser(|p, src, wd, fmt| {
                if fmt != "make" { return Err(()); }
                cook_cache::parse_make_depfile(p, src, wd).map_err(|_| ())
            });
        });
    }

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
        /// Not in cache; dispatch the unit normally.
        Miss,
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
        let meta = match &work_node.cache_meta {
            Some(m) => m,
            None => return CacheDecision::Miss,
        };
        if meta.output_paths.is_empty() {
            return CacheDecision::Miss;
        }
        let cm = match cache_managers.get(&work_node.recipe_name) {
            Some(cm) => cm,
            None => return CacheDecision::Miss,
        };
        let cache = cm.get_or_load(&meta.recipe_name);
        let entry = cache.steps.get(&meta.cache_key);
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
                .map(|f| f.path.clone())
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
                    e.outputs.iter().map(|r| r.path.clone()).collect();
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
        // COOK-162 §3: `local` units never reach the backend, so a local miss is
        // a plain Miss → rebuild.
        if meta.sharing.is_local() {
            return CacheDecision::Miss;
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
                    CacheDecision::Miss
                };
            }
        };
        if cook_fingerprint::fetch_by_key(
            &restore_ctx,
            meta.command_hash,
            meta.env_contribution,
            seal_contrib,
            &input_hashes,
            &current_outputs,
            &work_node.working_dir,
            meta.discovered_inputs.as_ref(),
        ) {
            CacheDecision::Hit
        } else if meta.sharing.is_pinned() {
            // Fetch-only unit absent from BOTH the local index and the shared
            // store: a hard error. The caller MUST NOT dispatch it.
            CacheDecision::PinnedColdMiss
        } else {
            CacheDecision::Miss
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
        fingerprint_by_node: &BTreeMap<usize, String>,
        cached_test_results: &mut Vec<crate::TestResult>,
        rerun_patterns: &[String],
        blocked_results: &mut Vec<crate::TestResult>,
        // G4 (CS-0074): probe cache lookup state.
        probe_units_by_node: &BTreeMap<usize, cook_contracts::ProbeUnit>,
        upstream_probe_fingerprints: &mut BTreeMap<String, [u8; 32]>,
        probe_fingerprint_by_node: &mut BTreeMap<usize, [u8; 32]>,
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
                        node_name: work_node.recipe_name.clone(),
                        artifact: work_node.cache_meta.as_ref()
                            .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
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
                        fingerprint_by_node,
                        cached_test_results,
                        rerun_patterns,
                        blocked_results,
                        probe_units_by_node,
                        upstream_probe_fingerprints,
                        probe_fingerprint_by_node,
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
                    if let Some(fp) = fingerprint_by_node.get(&id) {
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
                            // Emit NodeCompleted so the recipe tracker counts this node.
                            emit(event_tx, EngineEvent::NodeCompleted {
                                recipe: work_node.recipe_name.clone(),
                                node_name: test_name.clone(),
                                elapsed: duration,
                                kind: NodeKind::Test,
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
                                    fingerprint_by_node,
                                    cached_test_results,
                                    rerun_patterns,
                                    blocked_results,
                                    probe_units_by_node,
                                    upstream_probe_fingerprints,
                                    probe_fingerprint_by_node,
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
                match check_node_cache(work_node, cache_managers, cache_ctx, &pool.probe_value_store()) {
                    CacheDecision::Hit => {
                        ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                        emit(
                            event_tx,
                            EngineEvent::NodeCacheHit {
                                recipe: work_node.recipe_name.clone(),
                                node_name: payload.display_name(),
                                artifact: work_node.cache_meta.as_ref()
                                    .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
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
                                fingerprint_by_node,
                                cached_test_results,
                                rerun_patterns,
                                blocked_results,
                                probe_units_by_node,
                                upstream_probe_fingerprints,
                                probe_fingerprint_by_node,
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
                    CacheDecision::Miss => {}
                }

                ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                emit(
                    event_tx,
                    EngineEvent::NodeStarted {
                        recipe: work_node.recipe_name.clone(),
                        node_name: payload.display_name(),
                        artifact: work_node.cache_meta.as_ref()
                            .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
                        fallback_label: payload.display_name(),
                        kind: node_kind_for_payload(payload),
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
                    // Resolve fingerprint inputs. The env_lookup reads from the
                    // node's env_vars map (populated by the register phase from
                    // the recipe's env_vars).
                    let env_lookup = |name: &str| work_node.env_vars.get(name).cloned();
                    match cook_fingerprint::probe::resolve_probe_inputs(
                        probe_unit,
                        &work_node.working_dir,
                        &env_lookup,
                        upstream_probe_fingerprints,
                    ) {
                        Ok(inputs) => {
                            let fp = cook_fingerprint::compute_probe_fingerprint(&inputs);
                            // Store fingerprint now so G5 and downstream probes
                            // can find it regardless of cache-hit vs. miss path.
                            probe_fingerprint_by_node.insert(id, fp);

                            // Attempt cache GET.
                            match cook_cache::backend::get_bytes(
                                cache_ctx.backend.as_ref(),
                                &fp,
                            ) {
                                // A hit is only accepted when the cached bytes
                                // parse as probe-value JSON (CS-0102 stale-artifact
                                // defence, second layer behind the V2 marker).
                                Ok(Some(bytes))
                                    if cook_contracts::probe_value::decode_json(&bytes)
                                        .is_ok() =>
                                {
                                    // Cache hit — populate store and complete without dispatch.
                                    tracing::debug!(
                                        "probe '{}': cache hit (fp={:x?})",
                                        probe_key,
                                        &fp[..4],
                                    );
                                    // CS-0102: materialise the canonical local copy at
                                    // .cook/probes/<key>.json. Non-fatal on failure.
                                    let probes_dir = cache_ctx
                                        .project_root
                                        .join(".cook")
                                        .join("probes");
                                    if let Err(e) = cook_contracts::probe_value::write_probe_file(
                                        &probes_dir,
                                        &probe_key,
                                        &bytes,
                                    ) {
                                        tracing::warn!(
                                            "probe '{}': failed to write {}: {e}",
                                            probe_key,
                                            probes_dir.display(),
                                        );
                                    }
                                    {
                                        pool.probe_value_store()
                                            .insert(&probe_key, bytes);
                                    }
                                    // Propagate fingerprint so downstream probes can resolve
                                    // their own upstream_probe_fingerprints entries (mirrors
                                    // what the worker-result handler does on a cache miss).
                                    upstream_probe_fingerprints.insert(probe_key.clone(), fp);
                                    ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                                    emit(
                                        event_tx,
                                        EngineEvent::NodeCacheHit {
                                            recipe: work_node.recipe_name.clone(),
                                            node_name: node_name.clone(),
                                            artifact: None,
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
                                            fingerprint_by_node,
                                            cached_test_results,
                                            rerun_patterns,
                                            blocked_results,
                                            probe_units_by_node,
                                            upstream_probe_fingerprints,
                                            probe_fingerprint_by_node,
                                        );
                                    }
                                    return submitted;
                                }
                                Ok(Some(_)) => {
                                    // Cached bytes are not probe-value JSON —
                                    // treat as a miss and re-run produce. Evict
                                    // the stale entry so the post-run put (G5)
                                    // can self-heal the key with canonical JSON
                                    // (CS-0055 conflict detection would reject
                                    // an overwrite of differing bytes).
                                    tracing::warn!(
                                        "probe '{}': cached bytes are not probe-value JSON \
                                         (pre-CS-0102 artifact?); treating as miss",
                                        probe_key,
                                    );
                                    if let Err(e) = cache_ctx.backend.delete(&fp) {
                                        tracing::warn!("probe '{}': failed to evict stale cache entry: {e}", probe_key);
                                    }
                                }
                                Ok(None) => {
                                    // Cache miss — fall through to worker dispatch.
                                    tracing::debug!(
                                        "probe '{}': cache miss, dispatching to worker",
                                        probe_key,
                                    );
                                }
                                Err(e) => {
                                    // Backend error — treat as miss, log, dispatch.
                                    tracing::warn!(
                                        "probe '{}': cache backend error on get ({e}); treating as miss",
                                        probe_key,
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            // Fingerprint resolution failed (e.g. missing upstream).
                            // This is a hard error — the probe cannot be fingerprinted
                            // so it cannot safely proceed.
                            let err_msg = format!("probe '{}': fingerprint resolution failed: {e}", probe_key);
                            ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                            emit(
                                event_tx,
                                EngineEvent::NodeFailed {
                                    recipe: work_node.recipe_name.clone(),
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
                        node_name: node_name.clone(),
                        artifact: None,
                        fallback_label: node_name.clone(),
                        kind: NodeKind::Cooked,
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
                    project_root: cache_ctx.project_root.clone(),
                });
                1
            }
            Some(payload) => {
                // Check cache before executing.
                // COOK-162: `pinned` cold-miss aborts the node like a failed step.
                match check_node_cache(work_node, cache_managers, cache_ctx, &pool.probe_value_store()) {
                    CacheDecision::Hit => {
                        ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                        emit(
                            event_tx,
                            EngineEvent::NodeCacheHit {
                                recipe: work_node.recipe_name.clone(),
                                node_name: payload.display_name(),
                                artifact: work_node.cache_meta.as_ref()
                                    .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
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
                                fingerprint_by_node,
                                cached_test_results,
                                rerun_patterns,
                                blocked_results,
                                probe_units_by_node,
                                upstream_probe_fingerprints,
                                probe_fingerprint_by_node,
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
                    CacheDecision::Miss => {}
                }

                ensure_recipe_started(trackers, &work_node.recipe_name, event_tx);
                emit(
                    event_tx,
                    EngineEvent::NodeStarted {
                        recipe: work_node.recipe_name.clone(),
                        node_name: payload.display_name(),
                        artifact: work_node.cache_meta.as_ref()
                            .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
                        fallback_label: payload.display_name(),
                        kind: node_kind_for_payload(payload),
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
            fingerprint_by_node,
            &mut cached_test_results,
            rerun_patterns,
            &mut blocked_results,
            probe_units_by_node,
            &mut upstream_probe_fingerprints,
            &mut probe_fingerprint_by_node,
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
                                Ok(()) => run_interactive_on_main(
                                    cmd,
                                    *line,
                                    &work_node.working_dir,
                                    &work_node.env_vars,
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
                            fingerprint_by_node,
                            &mut cached_test_results,
                            rerun_patterns,
                            &mut blocked_results,
                            probe_units_by_node,
                            &mut upstream_probe_fingerprints,
                            &mut probe_fingerprint_by_node,
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
                    let summary = format!("step {}/{}: {}", k, n, err_msg);
                    emit(
                        &event_tx,
                        EngineEvent::NodeFailed {
                            recipe: chore_recipe.clone(),
                            node_name: chore_recipe.clone(),
                            elapsed: chore_elapsed,
                            error: summary.clone(),
                        },
                    );
                    failures.push((window[k - 1], chore_recipe.clone(), summary));

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
                            node_name: node_name.clone(),
                            artifact: work_node.cache_meta.as_ref()
                                .and_then(|m| m.output_paths.first().map(std::path::PathBuf::from)),
                            fallback_label: node_name.clone(),
                            // Interactive payloads (@-shell) are never test steps,
                            // so default to Cooked.
                            kind: NodeKind::Cooked,
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
                                node_name: node_name.clone(),
                                elapsed: interactive_elapsed,
                                // Interactive nodes are never test steps.
                                kind: NodeKind::Cooked,
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
                                fingerprint_by_node,
                                &mut cached_test_results,
                                rerun_patterns,
                                &mut blocked_results,
                                probe_units_by_node,
                                &mut upstream_probe_fingerprints,
                                &mut probe_fingerprint_by_node,
                            );
                        }
                    } else {
                        let err_msg = result.unwrap_err();
                        emit(
                            &event_tx,
                            EngineEvent::NodeFailed {
                                recipe: recipe_name.clone(),
                                node_name: node_name.clone(),
                                elapsed: interactive_elapsed,
                                error: err_msg.clone(),
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

        // G3 (CS-0074): if this result carries a probe output, write it into
        // the ProbeValueStore immediately so that consumer units
        // dispatched after this point can read it via cook.cache.get (§22.5.7).
        // CS-0102: first materialise the canonical local copy at
        // .cook/probes/<key>.json with the worker's bytes verbatim, so the
        // file, the per-run store, and the CAS artifact (G5 below) hold
        // byte-identical content. Non-fatal on failure.
        if let Some(ref probe_out) = result.probe_output {
            if result.success {
                let probes_dir = cache_ctx.project_root.join(".cook").join("probes");
                if let Err(e) = cook_contracts::probe_value::write_probe_file(
                    &probes_dir,
                    &probe_out.key,
                    &probe_out.bytes,
                ) {
                    tracing::warn!(
                        "probe '{}': failed to write {}: {e}",
                        probe_out.key,
                        probes_dir.display(),
                    );
                }
            }
            pool.probe_value_store()
                .insert(&probe_out.key, probe_out.bytes.clone());
        }

        // G5 (CS-0074): persist probe output to CacheBackend after the worker
        // returns with bytes on a cache miss. Reuses the fingerprint computed at
        // dispatch time (G4) stored in `probe_fingerprint_by_node`, so we never
        // recompute for the same node. Also populates `upstream_probe_fingerprints`
        // so downstream probes can include this probe's fingerprint in their own.
        //
        // This block runs for *every* result (success or failure) so that the
        // fingerprint map is always populated before newly-ready nodes are
        // processed (the `dag.complete(result.id)` call below may unblock
        // downstream probes). On failure, we skip the backend put but still
        // record the fingerprint so the map is consistent.
        if let Some(ref probe_out) = result.probe_output {
            if result.success {
                if let Some(&fp) = probe_fingerprint_by_node.get(&result.id) {
                    // G5a: populate upstream_fingerprints for downstream probes.
                    upstream_probe_fingerprints.insert(probe_out.key.clone(), fp);

                    // G5b: persist to CacheBackend with kind=probe_value.
                    let mut artifact_meta = ArtifactMeta {
                        recipe_namespace: format!("probe:{}", probe_out.key),
                        command_hash: 0,
                        env_contribution: 0,
                        seal_contribution: 0,
                        schema_version: CACHE_VERSION,
                        size_bytes: probe_out.bytes.len() as u64,
                        tags: std::collections::BTreeSet::new(),
                        consulted_env_keys: std::collections::BTreeSet::new(),
                        output_index: 0,
                        output_path: format!("probe:{}", probe_out.key),
                        content_hash: ArtifactMeta::zero_content_hash(),
                        kind: None,
                        mode: ArtifactMeta::default_mode(),
                        target: None,
                    }
                    .as_probe_value();
                    // COOK-168: publish-off / read-only client mode suppresses
                    // ALL shared-store uploads, including probe values — fetch
                    // by key is unaffected. The canonical local copy
                    // (.cook/probes/<key>.json) and the per-run ProbeValueStore
                    // were already populated above (CS-0102 / G3), so same-build
                    // consumers and downstream probes still read the value; only
                    // the shared-backend put is skipped.
                    if cache_ctx.publish_enabled {
                        if let Err(e) = cook_cache::backend::put_bytes(
                            cache_ctx.backend.as_ref(),
                            &fp,
                            &probe_out.bytes,
                            &mut artifact_meta,
                        ) {
                            tracing::warn!(
                                "probe '{}': cache backend put failed ({}); continuing without caching",
                                probe_out.key, e,
                            );
                        } else {
                            tracing::debug!(
                                "probe '{}': cached output (fp={:x?})",
                                probe_out.key, &fp[..4],
                            );
                        }
                    }
                } else {
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
                        line: line.clone(),
                        stream: *stream,
                    },
                );
            }

            emit(
                &event_tx,
                EngineEvent::NodeCompleted {
                    recipe: recipe_name.clone(),
                    node_name: result.node_name.clone(),
                    elapsed: Duration::ZERO,
                    kind: work_node
                        .payload
                        .as_ref()
                        .map(node_kind_for_payload)
                        .unwrap_or(NodeKind::Cooked),
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
                let fp_opt = fingerprint_by_node.get(&result.id).cloned();
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
                        schema_version: 1,
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
                    fingerprint_by_node,
                    &mut cached_test_results,
                    rerun_patterns,
                    &mut blocked_results,
                    probe_units_by_node,
                    &mut upstream_probe_fingerprints,
                    &mut probe_fingerprint_by_node,
                );
            }
        } else {
            // Emit output lines even on failure (CS-0035 — stream tagged).
            for (stream, line) in &result.output_lines {
                emit(
                    &event_tx,
                    EngineEvent::OutputLine {
                        recipe: recipe_name.clone(),
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

            // Test semantic failures (result.test_output.is_some()) are "soft":
            // the outcome is already recorded in test_results as TestOutcome::Failed
            // or TestOutcome::TimedOut. We do NOT add them to the hard `failures`
            // list — that list is for infrastructure failures (spawn errors, etc.).
            // Dependents of failed tests are NOT cancelled: a test failing does not
            // block sibling or downstream tests.
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
                        node_name: result.node_name.clone(),
                        elapsed: Duration::ZERO,
                        error: err_msg.clone(),
                    },
                );
                finish_recipe_node(&mut recipe_trackers, &recipe_name, false, false, &event_tx);
                // Complete the node in the DAG so dependents can proceed.
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
                        fingerprint_by_node,
                        &mut cached_test_results,
                        rerun_patterns,
                        &mut blocked_results,
                        probe_units_by_node,
                        &mut upstream_probe_fingerprints,
                        &mut probe_fingerprint_by_node,
                    );
                }
            } else {
                // Hard failure: infrastructure error.
                emit(
                    &event_tx,
                    EngineEvent::NodeFailed {
                        recipe: recipe_name.clone(),
                        node_name: result.node_name.clone(),
                        elapsed: Duration::ZERO,
                        error: err_msg.clone(),
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
        let _ = cm.flush_all();
    }

    if failures.is_empty() {
        // Merge cached_test_results (from test cache hits synthesized during
        // process_ready) with test_results (from actual executions).
        let mut all = cached_test_results;
        all.extend(test_results);
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
                path: ed.clone(),
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
    consulted_env: &std::collections::BTreeMap<String, String>,
    seal_keys: &std::collections::BTreeSet<String>,
    probe_store: &cook_luaotp::ProbeValueStore,
) -> DeterminantManifest {
    let inputs_map: std::collections::BTreeMap<String, u64> =
        inputs.iter().map(|fr| (fr.path.clone(), fr.hash)).collect();
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
        consulted_env: consulted_env.clone(),
        sealed_probes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Stream;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn shell(cmd: &str) -> WorkPayload {
        WorkPayload::Shell {
            cmd: cmd.to_string(),
            line: 0,
        }
    }

    #[test]
    fn build_determinant_manifest_captures_resolved_determinants() {
        use cook_fingerprint::FileRecord;
        use std::collections::{BTreeMap, BTreeSet};
        let inputs = vec![
            FileRecord {
                path: "src/b.c".into(),
                mtime: 0,
                hash: 0x2222,
            },
            FileRecord {
                path: "src/a.c".into(),
                mtime: 0,
                hash: 0x1111,
            },
        ];
        let mut consulted = BTreeMap::new();
        consulted.insert("CC".to_string(), "clang".to_string());
        let mut seal_keys = BTreeSet::new();
        seal_keys.insert("host".to_string());
        let store = cook_luaotp::ProbeValueStore::new();
        store.insert("host", b"\"x86_64-linux\"".to_vec());

        let m = build_determinant_manifest(
            CACHE_VERSION,
            "cook/Cookfile::build",
            &[0xABu8; 32],
            0x1234,
            0x5678,
            0x9abc,
            &inputs,
            &["build/a.o".to_string()],
            &consulted,
            &seal_keys,
            &store,
        );
        assert_eq!(m.recipe_namespace, "cook/Cookfile::build");
        assert_eq!(m.key, "ab".repeat(32));
        assert_eq!(m.inputs["src/a.c"], 0x1111);
        assert_eq!(m.inputs["src/b.c"], 0x2222);
        assert_eq!(m.output_paths, vec!["build/a.o".to_string()]);
        assert_eq!(m.consulted_env["CC"], "clang");
        assert_eq!(m.sealed_probes["host"], "\"x86_64-linux\"");
    }

    fn tmp_dir() -> (PathBuf, TempDir) {
        let d = TempDir::new().unwrap();
        (d.path().to_path_buf(), d)
    }

    fn default_env() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn work_node(payload: WorkPayload, recipe: &str, wd: PathBuf) -> WorkNode {
        WorkNode {
            payload: Some(payload),
            recipe_name: recipe.to_string(),
            cache_meta: None,
            working_dir: wd,
            env_vars: default_env(),
        }
    }

    fn presatisfied_node(recipe: &str, wd: PathBuf) -> WorkNode {
        WorkNode {
            payload: None,
            recipe_name: recipe.to_string(),
            cache_meta: None,
            working_dir: wd,
            env_vars: default_env(),
        }
    }

    /// Build a minimal CacheContext backed by a temp-dir LocalBackend.
    /// Suitable for executor tests that don't exercise the cache path.
    fn make_cache_ctx(tmp: &TempDir) -> Arc<CacheContext> {
        use cook_cache::{
            backend::LocalBackend, cache_ctx::CacheContext, cloud_config::CloudConfig,
        };
        use cook_fingerprint::EnvDenylist;
        Arc::new(CacheContext {
            denylist: Arc::new(EnvDenylist::baseline()),
            backend: Arc::new(LocalBackend::new(tmp.path().join("cloud"))),
            cloud_config: Arc::new(CloudConfig::default()),
            project_root: tmp.path().to_path_buf(),
            project_id: "test".to_string(),
            publish_enabled: true,
        })
    }

    // 1. Single node succeeds
    #[test]
    fn test_executor_runs_single_node() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);
        let mut dag = Dag::new();
        dag.add_node(work_node(shell("true"), "single", wd), &[]).unwrap();

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    // 2. Dependencies respected: A writes file, B reads it
    #[test]
    fn test_executor_respects_dependencies() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        let a = dag.add_node(
            work_node(shell("echo hello > output.txt"), "writer", wd.clone()),
            &[],
        ).unwrap();
        dag.add_node(
            work_node(shell("cat output.txt"), "reader", wd),
            &[a],
        ).unwrap();

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    // 3. Failure cancels downstream
    #[test]
    fn test_executor_failure_cancels_downstream() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        let a = dag.add_node(work_node(shell("false"), "fail_a", wd.clone()), &[]).unwrap();
        // B depends on A — should never run.
        dag.add_node(
            work_node(
                shell("echo should_not_run > /tmp/cook_test_should_not_exist"),
                "downstream_b",
                wd,
            ),
            &[a],
        ).unwrap();

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::TaskFailures { failures, .. } => {
                assert_eq!(failures.len(), 1);
                assert_eq!(failures[0].1, "fail_a");
            }
            other => panic!("expected TaskFailures, got: {other:?}"),
        }
    }

    // 4. Parallel independent nodes (timing)
    #[test]
    fn test_executor_parallel_independent_nodes() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        for i in 0..4 {
            dag.add_node(
                work_node(shell("sleep 0.2"), &format!("sleep_{i}"), wd.clone()),
                &[],
            ).unwrap();
        }

        let start = std::time::Instant::now();
        let result = execute_dag(dag, 4, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        let elapsed = start.elapsed();

        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        // With 4 workers, 4 x sleep 0.2 should take ~0.2s, not ~0.8s.
        assert!(
            elapsed.as_secs_f64() < 0.6,
            "took too long ({:.2}s), likely not parallel",
            elapsed.as_secs_f64()
        );
    }

    // 5. Empty DAG
    #[test]
    fn test_executor_empty_dag() {
        let (_wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);
        let dag: Dag<WorkNode> = Dag::new();
        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok());
    }

    // 6. Presatisfied chain: presatisfied A -> presatisfied B -> work C
    #[test]
    fn test_executor_presatisfied_chain() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        let a = dag.add_node(presatisfied_node("cached_a", wd.clone()), &[]).unwrap();
        let b = dag.add_node(presatisfied_node("cached_b", wd.clone()), &[a]).unwrap();
        dag.add_node(work_node(shell("true"), "real_work", wd), &[b]).unwrap();

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    // 7. Failure does not cancel independent nodes
    #[test]
    fn test_executor_failure_does_not_cancel_independent() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        // A will fail
        dag.add_node(work_node(shell("false"), "fail_a", wd.clone()), &[]).unwrap();
        // B is independent, should succeed
        dag.add_node(work_node(shell("true"), "ok_b", wd), &[]).unwrap();

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::TaskFailures { failures, .. } => {
                // Only A should be in failures
                assert_eq!(failures.len(), 1);
                assert_eq!(failures[0].1, "fail_a");
            }
            other => panic!("expected TaskFailures, got: {other:?}"),
        }
    }

    // 8. Interactive node runs after pool drains
    #[test]
    fn test_executor_interactive_node() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        let a = dag.add_node(work_node(shell("echo setup"), "setup", wd.clone()), &[]).unwrap();
        dag.add_node(
            work_node(
                WorkPayload::Interactive {
                    cmd: "echo interactive".to_string(),
                    line: 5,
                    is_chore: false,
                },
                "run",
                wd,
            ),
            &[a],
        ).unwrap();

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    // 9. CS-0035: OutputLine events carry true fd-of-origin in the `stream`
    //    field instead of attributing every captured byte to stdout.  Pre-fix,
    //    both the success branch and the failure branch in execute_dag
    //    hardcoded `is_stderr: false`, so any `Stream::Stderr` value rendered
    //    in events.jsonl was unreachable end-to-end.
    #[test]
    fn test_executor_output_line_stream_reflects_fd_of_origin() {
        use std::sync::mpsc;

        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        // A shell command that emits one line to stdout and one to stderr.
        // The captured bytes' fds must round-trip through OutputLine events.
        let mut dag = Dag::new();
        dag.add_node(
            work_node(
                shell("echo to-stdout; echo to-stderr 1>&2"),
                "mixed",
                wd,
            ),
            &[],
        )
        .unwrap();

        let (tx, rx) = mpsc::channel::<EngineEvent>();
        let result = execute_dag(dag, 1, BTreeMap::new(), Some(tx), cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");

        let mut got_stdout = false;
        let mut got_stderr = false;
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::OutputLine { line, stream, .. } = event {
                match stream {
                    Stream::Stdout => {
                        assert_eq!(line, "to-stdout", "stdout line content");
                        got_stdout = true;
                    }
                    Stream::Stderr => {
                        assert_eq!(line, "to-stderr", "stderr line content");
                        got_stderr = true;
                    }
                    _ => panic!("unexpected non-exhaustive Stream variant"),
                }
            }
        }
        assert!(got_stdout, "expected an OutputLine with stream=Stdout");
        assert!(got_stderr, "expected an OutputLine with stream=Stderr");
    }

    // ---------------------------------------------------------------------
    // CS-0050: engine MUST mkdir -p the parent of every declared cook-step
    // output before the step runs.
    // ---------------------------------------------------------------------

    fn cook_meta(output_paths: Vec<&str>) -> cook_contracts::CacheMeta {
        cook_contracts::CacheMeta {
            recipe_name: "r".into(),
            project_id: "test".into(),
            cookfile_path: "Cookfile".into(),
            cache_key: "k".into(),
            input_paths: vec![],
            output_paths: output_paths.into_iter().map(String::from).collect(),
            command_hash: 0,
            env_contribution: 0,
            consulted_env: BTreeMap::new(),
            discovered_inputs: None,
            seal_keys: Default::default(),
            sharing: Default::default(),
            record: false,
        }
    }

    fn cook_node(payload: WorkPayload, recipe: &str, wd: PathBuf, outputs: Vec<&str>) -> WorkNode {
        WorkNode {
            payload: Some(payload),
            recipe_name: recipe.to_string(),
            cache_meta: Some(cook_meta(outputs)),
            working_dir: wd,
            env_vars: default_env(),
        }
    }

    /// Build a cook node carrying a COOK-162 disposition (`local`/`pinned`) on
    /// its CacheMeta. The CacheMeta's `recipe_name` is set to match the node's
    /// recipe so `check_node_cache`'s cache-manager lookup resolves.
    fn cook_node_disposition(
        payload: WorkPayload,
        recipe: &str,
        wd: PathBuf,
        outputs: Vec<&str>,
        sharing: cook_contracts::Sharing,
    ) -> WorkNode {
        let mut meta = cook_meta(outputs);
        meta.recipe_name = recipe.to_string();
        meta.cache_key = format!("k_{recipe}");
        meta.sharing = sharing;
        WorkNode {
            payload: Some(payload),
            recipe_name: recipe.to_string(),
            cache_meta: Some(meta),
            working_dir: wd,
            env_vars: default_env(),
        }
    }

    /// A `cache_managers` map carrying one fresh, empty manager for `recipe`,
    /// backed by a temp cache dir. Required so `check_node_cache` does not
    /// short-circuit to Miss on a missing manager.
    fn empty_cache_managers(recipe: &str, dir: &std::path::Path) -> BTreeMap<String, Arc<ThreadSafeCacheManager>> {
        let mut m = BTreeMap::new();
        m.insert(
            recipe.to_string(),
            Arc::new(ThreadSafeCacheManager::new(dir.to_path_buf())),
        );
        m
    }

    // COOK-162 §3 sharing — `local` unit with no local StepEntry and an EMPTY
    // shared backend must NOT consult the backend and must fall through to a
    // normal rebuild (Miss). The node runs, produces its output, and succeeds.
    #[test]
    fn test_executor_cook162_local_cold_miss_rebuilds() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);
        let managers = empty_cache_managers("loc", _tmp.path());

        let mut dag = Dag::new();
        dag.add_node(
            cook_node_disposition(
                shell("echo hi > out.txt"),
                "loc",
                wd.clone(),
                vec!["out.txt"],
                cook_contracts::Sharing::Local,
            ),
            &[],
        )
        .unwrap();

        let result = execute_dag(dag, 1, managers, None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "local cold-miss should rebuild, got: {result:?}");
        assert!(wd.join("out.txt").exists(), "local unit should have run");
    }

    // COOK-162 §3 sharing — `pinned` (fetch-only) unit absent from BOTH the
    // local index and the EMPTY shared backend is a HARD ERROR. The unit MUST
    // NOT be dispatched/rebuilt; execute_dag returns TaskFailures and the
    // declared output is never produced.
    #[test]
    fn test_executor_cook162_pinned_cold_miss_is_fatal() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);
        let managers = empty_cache_managers("pin", _tmp.path());

        let mut dag = Dag::new();
        dag.add_node(
            cook_node_disposition(
                // If this ran, it would create out.txt — it MUST NOT.
                shell("echo hi > out.txt"),
                "pin",
                wd.clone(),
                vec!["out.txt"],
                cook_contracts::Sharing::Pinned,
            ),
            &[],
        )
        .unwrap();

        let result = execute_dag(dag, 1, managers, None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        let err = result.expect_err("pinned cold-miss must be fatal");
        match err {
            EngineError::TaskFailures { failures, .. } => {
                assert_eq!(failures.len(), 1, "exactly one failure expected");
                assert_eq!(failures[0].1, "pin");
                assert!(
                    failures[0].2.contains("pinned") && failures[0].2.contains("fetch-only"),
                    "diagnostic should explain the fetch-only contract; got: {}",
                    failures[0].2
                );
            }
            other => panic!("expected TaskFailures, got: {other:?}"),
        }
        assert!(
            !wd.join("out.txt").exists(),
            "pinned cold-miss MUST NOT dispatch the unit"
        );
    }

    // 10. CS-0050: a cook step's missing output parent dir is created
    //     before the shell text runs, so authors can drop `mkdir -p`.
    #[test]
    fn test_executor_cs_0050_creates_missing_output_parent() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        // Output sits in `build/out/foo.txt` — neither `build` nor
        // `build/out` exists when the step starts. The shell text has NO
        // `mkdir -p` boilerplate.
        let mut dag = Dag::new();
        dag.add_node(
            cook_node(
                shell("echo hi > build/out/foo.txt"),
                "build",
                wd.clone(),
                vec!["build/out/foo.txt"],
            ),
            &[],
        )
        .unwrap();

        let result = execute_dag(dag, 1, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");

        let out = wd.join("build/out/foo.txt");
        assert!(out.exists(), "output {} not created", out.display());
        let body = std::fs::read_to_string(&out).unwrap();
        assert_eq!(body.trim_end(), "hi");
    }

    // 11. CS-0050: when the parent path resolves to a non-directory (a
    //     regular file), the engine MUST surface a clear diagnostic
    //     naming the output and the offending parent, NOT execute the
    //     shell text, and NOT attempt to overwrite the file.
    #[test]
    fn test_executor_cs_0050_parent_is_file_diagnostic() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        // Make `build` a regular file; then declare an output whose
        // parent is `build/`.
        std::fs::write(wd.join("build"), b"not a dir").unwrap();

        let mut dag = Dag::new();
        dag.add_node(
            cook_node(
                shell("echo hi > build/foo.txt"),
                "build",
                wd.clone(),
                vec!["build/foo.txt"],
            ),
            &[],
        )
        .unwrap();

        let result = execute_dag(dag, 1, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        let err = result.expect_err("expected failure when parent is a regular file");
        match err {
            EngineError::TaskFailures { failures, .. } => {
                assert_eq!(failures.len(), 1);
                let msg = &failures[0].2;
                assert!(
                    msg.contains("CS-0050"),
                    "diagnostic should be tagged CS-0050; got: {msg}"
                );
                assert!(
                    msg.contains("build/foo.txt"),
                    "diagnostic should name the declared output; got: {msg}"
                );
                assert!(
                    msg.contains("non-directory") || msg.contains("non-directory"),
                    "diagnostic should explain why mkdir failed; got: {msg}"
                );
            }
            other => panic!("expected TaskFailures, got: {other:?}"),
        }

        // The `build` regular file MUST NOT have been overwritten.
        let body = std::fs::read_to_string(wd.join("build")).unwrap();
        assert_eq!(body, "not a dir");
        // And the declared output MUST NOT exist.
        assert!(!wd.join("build/foo.txt").exists());
    }

    // 12. CS-0050: the call is a no-op when cache_meta is absent (plate /
    //     test units, presatisfied units) — those paths must not regress.
    //     Exercised by the existing `test_executor_runs_single_node`
    //     baseline; this test pins idempotence on a cook step whose
    //     parent already exists as a directory.
    #[test]
    fn test_executor_cs_0050_idempotent_when_parent_exists() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        std::fs::create_dir_all(wd.join("build")).unwrap();

        let mut dag = Dag::new();
        dag.add_node(
            cook_node(
                shell("echo hi > build/foo.txt"),
                "build",
                wd.clone(),
                vec!["build/foo.txt"],
            ),
            &[],
        )
        .unwrap();

        let result = execute_dag(dag, 1, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert!(wd.join("build/foo.txt").exists());
    }

    // 13. CS-0050 unit-level helper: an output with no parent component
    //     (root-level path) is a no-op.
    #[test]
    fn test_ensure_output_parent_dirs_no_parent_is_noop() {
        let (wd, _tmp) = tmp_dir();
        let node = cook_node(shell("true"), "r", wd.clone(), vec!["out.txt"]);
        // wd.join("out.txt").parent() == Some(wd) which exists.
        ensure_output_parent_dirs(&node).expect("no parent component should be a no-op");
    }

    // -----------------------------------------------------------------
    // CS-0051: chore-window grouping. A chore body MUST execute as a
    // single drain — one InteractiveStart/InteractiveEnd pair covers all
    // body steps, and the recipe completion event carries `kind: Chore`.
    // -----------------------------------------------------------------

    #[test]
    fn chore_window_groups_consecutive_chore_steps_into_one_pair() {
        use std::sync::mpsc;
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        // Three chore steps (is_chore=true) for one recipe — they must group.
        let a = dag.add_node(
            work_node(
                WorkPayload::Interactive { cmd: "true".into(), line: 1, is_chore: true },
                "chore", wd.clone()),
            &[]).unwrap();
        let b = dag.add_node(
            work_node(
                WorkPayload::Interactive { cmd: "true".into(), line: 2, is_chore: true },
                "chore", wd.clone()),
            &[a]).unwrap();
        dag.add_node(
            work_node(
                WorkPayload::Interactive { cmd: "true".into(), line: 3, is_chore: true },
                "chore", wd.clone()),
            &[b]).unwrap();

        let (tx, rx) = mpsc::channel();
        let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "got: {result:?}");

        let events: Vec<_> = rx.try_iter().collect();
        let starts = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveStart { .. })).count();
        let ends = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveEnd { .. })).count();
        assert_eq!(starts, 1, "exactly one InteractiveStart per chore window; got events:\n{events:#?}");
        assert_eq!(ends, 1, "exactly one InteractiveEnd per chore window; got events:\n{events:#?}");

        match events.iter().find(|e| matches!(e, EngineEvent::InteractiveStart { .. })).unwrap() {
            EngineEvent::InteractiveStart { chore_step_count, .. } => {
                assert_eq!(*chore_step_count, 3);
            }
            _ => unreachable!(),
        }

        // RecipeCompleted MUST carry kind: Chore for chore recipes.
        let recipe_completed = events
            .iter()
            .find(|e| matches!(e, EngineEvent::RecipeCompleted { .. }))
            .expect("expected RecipeCompleted event");
        match recipe_completed {
            EngineEvent::RecipeCompleted { kind, .. } => {
                assert_eq!(*kind, RecipeKind::Chore);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn chore_window_failure_mid_run_emits_one_node_failed_with_step_index() {
        use std::sync::mpsc;
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        let a = dag.add_node(
            work_node(
                WorkPayload::Interactive { cmd: "true".into(), line: 1, is_chore: true },
                "chore", wd.clone()),
            &[]).unwrap();
        let b = dag.add_node(
            work_node(
                WorkPayload::Interactive { cmd: "false".into(), line: 2, is_chore: true },
                "chore", wd.clone()),
            &[a]).unwrap();
        dag.add_node(
            work_node(
                WorkPayload::Interactive { cmd: "true".into(), line: 3, is_chore: true },
                "chore", wd),
            &[b]).unwrap();

        let (tx, rx) = mpsc::channel();
        let _result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));

        let events: Vec<_> = rx.try_iter().collect();
        let node_failed: Vec<_> = events.iter().filter(|e| matches!(e, EngineEvent::NodeFailed { .. })).collect();
        assert_eq!(node_failed.len(), 1, "exactly one NodeFailed per chore failure; got: {events:#?}");
        match node_failed[0] {
            EngineEvent::NodeFailed { error, .. } => {
                assert!(error.contains("step 2/3"), "expected 'step 2/3' in error, got: {error}");
            }
            _ => unreachable!(),
        }

        let end = events.iter().find(|e| matches!(e, EngineEvent::InteractiveEnd { .. })).unwrap();
        match end {
            EngineEvent::InteractiveEnd { failed_step, success, .. } => {
                assert_eq!(*failed_step, Some(2));
                assert!(!*success);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn non_chore_interactive_still_emits_per_node_pair() {
        use std::sync::mpsc;
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        dag.add_node(
            work_node(
                WorkPayload::Interactive {
                    cmd: "echo legacy".into(),
                    line: 1,
                    is_chore: false,
                },
                "step", wd),
            &[]).unwrap();

        let (tx, rx) = mpsc::channel();
        let _result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));

        let events: Vec<_> = rx.try_iter().collect();
        let starts = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveStart { .. })).count();
        let ends = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveEnd { .. })).count();
        assert_eq!(starts, 1);
        assert_eq!(ends, 1);
        // chore_step_count must be 0 to flag the legacy path.
        match events.iter().find(|e| matches!(e, EngineEvent::InteractiveStart { .. })).unwrap() {
            EngineEvent::InteractiveStart { chore_step_count, .. } => assert_eq!(*chore_step_count, 0),
            _ => unreachable!(),
        }
    }

    // -----------------------------------------------------------------
    // CS-0051 Lua-bundle integration: the chore-window drain admits Lua-
    // bundle steps alongside shell steps. A mixed shell+Lua chore body
    // produces a single InteractiveStart/End pair; a pure-Lua chore body
    // does likewise; non-chore LuaChunks still route through the worker
    // pool (regression guard).
    // -----------------------------------------------------------------

    #[test]
    fn chore_window_groups_shell_and_lua_into_one_pair() {
        use std::sync::mpsc;
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        let a = dag.add_node(
            work_node(
                WorkPayload::Interactive { cmd: "true".into(), line: 1, is_chore: true },
                "shell1", wd.clone()),
            &[]).unwrap();
        let b = dag.add_node(
            work_node(
                WorkPayload::LuaChunk {
                    code: "-- noop".into(),
                    inputs: vec![],
                    outputs: vec![],
                    ingredient_groups: vec![],
                    step_kind: cook_contracts::StepKind::Chore,
                    is_chore: true,
                    line: 0,
                },
                "shell1", wd.clone()),
            &[a]).unwrap();
        dag.add_node(
            work_node(
                WorkPayload::Interactive { cmd: "true".into(), line: 3, is_chore: true },
                "shell1", wd),
            &[b]).unwrap();

        let (tx, rx) = mpsc::channel();
        let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "got: {result:?}");

        let events: Vec<_> = rx.try_iter().collect();
        let starts = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveStart { .. })).count();
        let ends = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveEnd { .. })).count();
        assert_eq!(
            starts, 1,
            "mixed shell+lua chore body must produce ONE InteractiveStart; got events:\n{events:#?}"
        );
        assert_eq!(
            ends, 1,
            "mixed shell+lua chore body must produce ONE InteractiveEnd; got events:\n{events:#?}"
        );
        match events.iter().find(|e| matches!(e, EngineEvent::InteractiveStart { .. })).unwrap() {
            EngineEvent::InteractiveStart { chore_step_count, .. } => {
                assert_eq!(*chore_step_count, 3, "chore_step_count covers all three body steps");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn pure_lua_chore_body_produces_one_drain_window() {
        use std::sync::mpsc;
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        let a = dag.add_node(
            work_node(
                WorkPayload::LuaChunk {
                    code: "-- noop".into(),
                    inputs: vec![],
                    outputs: vec![],
                    ingredient_groups: vec![],
                    step_kind: cook_contracts::StepKind::Chore,
                    is_chore: true,
                    line: 0,
                },
                "lua_chore", wd.clone()),
            &[]).unwrap();
        dag.add_node(
            work_node(
                WorkPayload::LuaChunk {
                    code: "-- noop".into(),
                    inputs: vec![],
                    outputs: vec![],
                    ingredient_groups: vec![],
                    step_kind: cook_contracts::StepKind::Chore,
                    is_chore: true,
                    line: 0,
                },
                "lua_chore", wd),
            &[a]).unwrap();

        let (tx, rx) = mpsc::channel();
        let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "got: {result:?}");

        let events: Vec<_> = rx.try_iter().collect();
        let starts = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveStart { .. })).count();
        let ends = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveEnd { .. })).count();
        assert_eq!(starts, 1, "pure-lua chore body must produce ONE InteractiveStart; got: {events:#?}");
        assert_eq!(ends, 1, "pure-lua chore body must produce ONE InteractiveEnd; got: {events:#?}");
        match events.iter().find(|e| matches!(e, EngineEvent::InteractiveStart { .. })).unwrap() {
            EngineEvent::InteractiveStart { chore_step_count, .. } => {
                assert_eq!(*chore_step_count, 2);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn non_chore_lua_chunk_still_dispatches_to_worker_pool() {
        // Regression: a LuaChunk with `is_chore = false` MUST continue to
        // route through the worker pool (its is_chore = false means it is
        // a regular cook/test/plate body, not a chore-window member).
        // We pin this by exercising a one-node DAG; the engine must
        // complete without queuing the unit on the interactive_queue.
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        dag.add_node(
            work_node(
                WorkPayload::LuaChunk {
                    code: "-- noop".into(),
                    inputs: vec![],
                    outputs: vec![],
                    ingredient_groups: vec![],
                    step_kind: cook_contracts::StepKind::Cook,
                    is_chore: false,
                    line: 0,
                },
                "regular_lua", wd),
            &[]).unwrap();

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        assert!(result.is_ok(), "got: {result:?}");
    }

    // SHI-173: a failing cook step must produce Blocked TestResult rows for
    // downstream test nodes, not short-circuit to EngineError::TaskFailures
    // with no test results.
    //
    // In cook-mode callers (run()) this error still propagates unchanged.
    // In test-mode (run_for_test_inner()) the Blocked rows are extracted and
    // the error is swallowed. This test verifies the executor side: that
    // TaskFailures.partial_test_results contains the Blocked row.
    #[test]
    fn cook_failure_produces_blocked_test_result() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        // Cook node that will always fail.
        let cook = dag.add_node(
            work_node(shell("false"), "blocked_by_build", wd.clone()),
            &[],
        ).unwrap();
        // Test node downstream of the failing cook node.
        dag.add_node(
            work_node(
                WorkPayload::Test {
                    cmd: "true".to_string(),
                    line: 1,
                    timeout: 30,
                    should_fail: false,
                    suite_name: "blocked_by_build".to_string(),
                    test_name: "my_test".to_string(),
                    iteration_item: None,
                    lua_code: None,
                    input_paths: vec![],
                },
                "blocked_by_build",
                wd.clone(),
            ),
            &[cook],
        ).unwrap();

        let result = execute_dag(
            dag, 2, BTreeMap::new(), None, cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(),
            std::sync::Arc::new(BTreeMap::new()),
        );

        // The cook node failed → EngineError::TaskFailures
        let err = result.expect_err("expected TaskFailures due to failing cook node");
        match err {
            EngineError::TaskFailures { failures, partial_test_results, .. } => {
                // One cook failure.
                assert_eq!(failures.len(), 1, "expected 1 cook failure");
                assert_eq!(failures[0].1, "blocked_by_build");
                // Exactly one Blocked TestResult for the downstream test node.
                assert_eq!(
                    partial_test_results.len(), 1,
                    "expected 1 Blocked TestResult in partial_test_results"
                );
                let blocked = &partial_test_results[0];
                assert_eq!(blocked.outcome, crate::TestOutcome::Blocked);
                assert_eq!(blocked.name, "my_test");
                assert!(
                    blocked.blocked_by.is_some(),
                    "blocked_by should be populated"
                );
            }
            other => panic!("expected TaskFailures, got: {other:?}"),
        }
    }

    // SHI-line: WorkPayload::Test { line } must propagate into TestStarted and
    // TestResult.line rather than remaining 0.
    #[test]
    fn test_line_number_propagates_from_payload_to_events() {
        use std::sync::mpsc;
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        dag.add_node(
            work_node(
                WorkPayload::Test {
                    cmd: "true".to_string(),
                    line: 17,
                    timeout: 30,
                    should_fail: false,
                    suite_name: "my_recipe".to_string(),
                    test_name: "my_test".to_string(),
                    iteration_item: None,
                    lua_code: None,
                    input_paths: vec![],
                },
                "my_recipe",
                wd,
            ),
            &[],
        ).unwrap();

        let (tx, rx) = mpsc::channel();
        let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        let test_results = result.expect("test node should pass");

        // TestResult.line must carry 17.
        assert_eq!(test_results.len(), 1, "expected exactly one TestResult");
        assert_eq!(
            test_results[0].line, 17,
            "TestResult.line should be 17 (from WorkPayload::Test {{ line: 17 }})"
        );

        // The TestStarted event must also carry line 17.
        let events: Vec<_> = rx.try_iter().collect();
        let started = events.iter().find(|e| matches!(e, EngineEvent::TestStarted { .. }))
            .expect("expected a TestStarted event");
        match started {
            EngineEvent::TestStarted { line, .. } => {
                assert_eq!(*line, 17, "TestStarted.line should be 17");
            }
            _ => unreachable!(),
        }

        // The TestPassed event must also carry line 17.
        let passed = events.iter().find(|e| matches!(e, EngineEvent::TestPassed { .. }))
            .expect("expected a TestPassed event");
        match passed {
            EngineEvent::TestPassed { line, .. } => {
                assert_eq!(*line, 17, "TestPassed.line should be 17");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_iteration_item_propagates() {
        use std::sync::mpsc;
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);

        let mut dag = Dag::new();
        dag.add_node(
            work_node(
                WorkPayload::Test {
                    cmd: "true".to_string(),
                    line: 17,
                    timeout: 30,
                    should_fail: false,
                    suite_name: "my_recipe".to_string(),
                    test_name: "my_test".to_string(),
                    iteration_item: Some("a.cpp".into()),
                    lua_code: None,
                    input_paths: vec![],
                },
                "my_recipe",
                wd,
            ),
            &[],
        ).unwrap();

        let (tx, rx) = mpsc::channel();
        let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &BTreeMap::new(), &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
        let test_results = result.expect("test node should pass");

        // TestResult.iteration_item must carry "a.cpp".
        assert_eq!(test_results.len(), 1, "expected exactly one TestResult");
        assert_eq!(
            test_results[0].iteration_item,
            Some("a.cpp".into()),
            "TestResult.iteration_item should be Some(\"a.cpp\")"
        );
        // TestResult.id must end with "[a.cpp]".
        assert!(
            test_results[0].id.0.ends_with("[a.cpp]"),
            "TestResult.id should end with [a.cpp], got: {}",
            test_results[0].id.0
        );

        // The TestStarted event must carry iteration_item = Some("a.cpp").
        let events: Vec<_> = rx.try_iter().collect();
        let started = events.iter().find(|e| matches!(e, EngineEvent::TestStarted { .. }))
            .expect("expected a TestStarted event");
        match started {
            EngineEvent::TestStarted { iteration_item, .. } => {
                assert_eq!(
                    *iteration_item,
                    Some("a.cpp".into()),
                    "TestStarted.iteration_item should be Some(\"a.cpp\")"
                );
            }
            _ => unreachable!(),
        }
    }

    // -----------------------------------------------------------------
    // CS-0074 G4/G5: probe cache hit/miss/invalidation.
    //
    // These tests exercise the G4 (cache lookup before dispatch) and G5
    // (persist probe output to backend after worker returns) paths in
    // execute_dag. They require that:
    //   - A cache hit populates the ProbeValueStore and skips the
    //     worker (NodeCacheHit event, no NodeStarted).
    //   - A cache miss dispatches to the worker, which runs the produce
    //     source, and the result is persisted to the backend with
    //     kind=probe_value.
    //   - Changing a declared env var forces a different fingerprint and
    //     a cache miss even when a prior entry exists.
    // -----------------------------------------------------------------

    fn probe_unit(key: &str, produce: &str) -> cook_contracts::ProbeUnit {
        cook_contracts::ProbeUnit {
            key: key.to_string(),
            produce_source: produce.to_string(),
            produce_line: 1,
            inputs: cook_contracts::ProbeInputs::default(),
        }
    }

    fn probe_unit_with_env(key: &str, produce: &str, env_var: &str) -> cook_contracts::ProbeUnit {
        cook_contracts::ProbeUnit {
            key: key.to_string(),
            produce_source: produce.to_string(),
            produce_line: 1,
            inputs: cook_contracts::ProbeInputs {
                env: vec![env_var.to_string()],
                tools: vec![],
                files: vec![],
                requires: vec![],
            },
        }
    }

    fn probe_work_node(key: &str, produce: &str, wd: PathBuf) -> WorkNode {
        WorkNode {
            payload: Some(WorkPayload::Probe {
                key: key.to_string(),
                produce: produce.to_string(),
                line: 1,
            }),
            recipe_name: format!("probe:{}", key),
            cache_meta: None,
            working_dir: wd,
            env_vars: BTreeMap::new(),
        }
    }

    fn probe_work_node_with_env(
        key: &str,
        produce: &str,
        env_var: &str,
        env_val: &str,
        wd: PathBuf,
    ) -> WorkNode {
        let mut env_vars = BTreeMap::new();
        env_vars.insert(env_var.to_string(), env_val.to_string());
        WorkNode {
            payload: Some(WorkPayload::Probe {
                key: key.to_string(),
                produce: produce.to_string(),
                line: 1,
            }),
            recipe_name: format!("probe:{}", key),
            cache_meta: None,
            working_dir: wd,
            env_vars,
        }
    }

    /// Compute the fingerprint for a ProbeUnit with no env/tool/file/upstream
    /// inputs, suitable for pre-seeding the backend in cache-hit tests.
    fn fingerprint_for(pu: &cook_contracts::ProbeUnit, wd: &std::path::Path) -> [u8; 32] {
        let inputs = cook_fingerprint::resolve_probe_inputs(
            pu,
            wd,
            &|_| None,
            &BTreeMap::new(),
        )
        .expect("fingerprint resolution should succeed for simple probe");
        cook_fingerprint::compute_probe_fingerprint(&inputs)
    }

    /// Pre-populate the cache backend with known bytes under the given
    /// fingerprint key. Used to set up the "cache hit" scenario for G4 tests.
    fn seed_probe_cache(backend: &dyn cook_cache::backend::CacheBackend, fp: &[u8; 32], bytes: &[u8]) {
        let mut meta = cook_fingerprint::ArtifactMeta {
            recipe_namespace: "probe:test".into(),
            command_hash: 0,
            env_contribution: 0,
            schema_version: cook_fingerprint::CACHE_VERSION,
            size_bytes: bytes.len() as u64,
            tags: std::collections::BTreeSet::new(),
            consulted_env_keys: std::collections::BTreeSet::new(),
            output_index: 0,
            output_path: "probe:test".into(),
            content_hash: cook_fingerprint::ArtifactMeta::zero_content_hash(),
            kind: None,
            seal_contribution: 0,
            mode: cook_fingerprint::ArtifactMeta::default_mode(),
            target: None,
        }
        .as_probe_value();
        cook_cache::backend::put_bytes(backend, fp, bytes, &mut meta)
            .expect("seed_probe_cache: backend put failed");
    }

    // G4 test: pre-populate the cache with canned bytes; the probe's produce
    // source calls `error()` so execution would fail — but on a hit we MUST
    // skip dispatch and deliver the cached bytes without ever invoking produce.
    #[test]
    fn probe_cache_hit_skips_produce_execution() {
        use std::sync::mpsc;

        let (_wd, _tmp) = tmp_dir();
        let wd = _wd.clone();
        let cache_ctx = make_cache_ctx(&_tmp);

        // Build the ProbeUnit and compute its fingerprint.
        let pu = probe_unit("test:hit", "error('should not run')");
        let fp = fingerprint_for(&pu, &wd);

        // Seed the backend with the known bytes we expect to see in the store.
        let expected_bytes =
            cook_contracts::probe_value::encode_canonical_json(&serde_json::json!([true]));
        seed_probe_cache(cache_ctx.backend.as_ref(), &fp, &expected_bytes);

        // Build a DAG with the probe node.
        let mut dag = Dag::new();
        let node_id = dag
            .add_node(probe_work_node("test:hit", "error('should not run')", wd), &[])
            .unwrap();

        // Build probe_units_by_node: maps node 0 → our ProbeUnit.
        let mut probe_units_by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = BTreeMap::new();
        probe_units_by_node.insert(node_id, pu);

        // Listen for events to verify NodeCacheHit (not NodeStarted).
        let (tx, rx) = mpsc::channel();
        let result = execute_dag(
            dag,
            2,
            BTreeMap::new(),
            Some(tx),
            cache_ctx.clone(),
            None,
            &BTreeMap::new(),
            &[],
            &probe_units_by_node,
            std::sync::Arc::new(BTreeMap::new()),
        );
        assert!(result.is_ok(), "expected Ok, got: {result:?}");

        // Verify NodeCacheHit was emitted (not NodeStarted).
        let events: Vec<_> = rx.try_iter().collect();
        let cache_hits: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, EngineEvent::NodeCacheHit { .. }))
            .collect();
        let node_started: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, EngineEvent::NodeStarted { .. }))
            .collect();
        assert_eq!(cache_hits.len(), 1, "expected exactly one NodeCacheHit; events: {events:#?}");
        assert_eq!(node_started.len(), 0, "expected no NodeStarted on cache hit; events: {events:#?}");

        // Also verify the cached bytes are still retrievable from the backend
        // (the put_bytes call in the test harness must not corrupt the entry).
        let post = cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp)
            .expect("post-hit get");
        let stored = post.expect("cache entry must still exist after a hit read");
        assert_eq!(stored, expected_bytes, "cached bytes must survive a hit read");

        // CS-0102: the hit must also materialise the canonical local copy at
        // .cook/probes/<key>.json with exactly the cached bytes.
        let probe_file = _tmp.path().join(".cook").join("probes").join("test:hit.json");
        assert!(
            probe_file.exists(),
            "cache hit must write {}",
            probe_file.display()
        );
        assert_eq!(
            std::fs::read(&probe_file).unwrap(),
            expected_bytes,
            ".cook/probes file must hold the exact cached bytes"
        );
    }

    // G5 test: on a cache miss the worker executes the produce source, the
    // output is persisted to the backend with kind=probe_value, and the result
    // is available in the probe-value store.
    #[test]
    fn probe_cache_miss_persists_output() {
        let (_wd, _tmp) = tmp_dir();
        let wd = _wd.clone();
        let cache_ctx = make_cache_ctx(&_tmp);

        // Produce source: return the integer 42.
        let produce = "return 42";
        let pu = probe_unit("test:miss", produce);
        let fp = fingerprint_for(&pu, &wd);

        // Backend starts empty — cache miss guaranteed.
        let pre = cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp)
            .expect("pre-check get");
        assert!(pre.is_none(), "backend must be empty before the run");

        let mut dag = Dag::new();
        let node_id = dag
            .add_node(probe_work_node("test:miss", produce, wd), &[])
            .unwrap();

        let mut probe_units_by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = BTreeMap::new();
        probe_units_by_node.insert(node_id, pu);

        let result = execute_dag(
            dag,
            2,
            BTreeMap::new(),
            None,
            cache_ctx.clone(),
            None,
            &BTreeMap::new(),
            &[],
            &probe_units_by_node,
            std::sync::Arc::new(BTreeMap::new()),
        );
        assert!(result.is_ok(), "expected Ok, got: {result:?}");

        // G5: verify the artifact was persisted to the backend.
        let post = cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp)
            .expect("post-run get");
        assert!(
            post.is_some(),
            "probe artifact must be persisted to cache backend after execution (G5)"
        );

        // CS-0102: the persisted value must be the canonical JSON rendering of 42.
        let bytes = post.unwrap();
        let expected = cook_contracts::probe_value::encode_canonical_json(&serde_json::json!(42));
        assert_eq!(
            bytes, expected,
            "persisted probe bytes must be the canonical JSON rendering"
        );

        // G5: verify the persisted bytes are retrievable from the backend.
        // (G3 — probe-value store — is internal to execute_dag and not
        // accessible after the function returns, but the G5 backend entry
        // serves as equivalent evidence that the produce path ran to completion.)
        let post2 = cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp)
            .expect("second get");
        let persisted = post2.expect("artifact must still be in backend on second read");
        assert_eq!(persisted, bytes, "persisted bytes must round-trip through backend");

        // CS-0102: the miss path must also materialise .cook/probes/<key>.json
        // with the same bytes (file == store == CAS).
        let probe_file = _tmp.path().join(".cook").join("probes").join("test:miss.json");
        assert!(
            probe_file.exists(),
            "cache miss must write {}",
            probe_file.display()
        );
        assert_eq!(
            std::fs::read(&probe_file).unwrap(),
            expected,
            ".cook/probes file must hold the exact persisted bytes"
        );
    }

    // CS-0102 stale-artifact defence: a cache entry whose bytes are not
    // probe-value JSON (e.g. a pre-CS-0102 artifact) MUST be treated as a
    // miss — produce runs, no NodeCacheHit is emitted, and the entry is
    // overwritten with canonical JSON.
    #[test]
    fn probe_cache_hit_with_non_json_bytes_falls_through_to_miss() {
        use std::sync::mpsc;

        let (_wd, _tmp) = tmp_dir();
        let wd = _wd.clone();
        let cache_ctx = make_cache_ctx(&_tmp);

        let produce = "return 7";
        let pu = probe_unit("test:stale", produce);
        let fp = fingerprint_for(&pu, &wd);

        // Seed the backend with bytes that are NOT JSON (0x91 0xc3 is the old
        // encoding of [true]).
        seed_probe_cache(cache_ctx.backend.as_ref(), &fp, &[0x91, 0xc3]);

        let mut dag = Dag::new();
        let node_id = dag
            .add_node(probe_work_node("test:stale", produce, wd), &[])
            .unwrap();

        let mut probe_units_by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = BTreeMap::new();
        probe_units_by_node.insert(node_id, pu);

        let (tx, rx) = mpsc::channel();
        let result = execute_dag(
            dag,
            2,
            BTreeMap::new(),
            Some(tx),
            cache_ctx.clone(),
            None,
            &BTreeMap::new(),
            &[],
            &probe_units_by_node,
            std::sync::Arc::new(BTreeMap::new()),
        );
        assert!(result.is_ok(), "expected Ok, got: {result:?}");

        // The stale entry must NOT register as a cache hit.
        let events: Vec<_> = rx.try_iter().collect();
        let cache_hits: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, EngineEvent::NodeCacheHit { .. }))
            .collect();
        assert_eq!(
            cache_hits.len(),
            0,
            "non-JSON cached bytes must fall through to miss; events: {events:#?}"
        );

        // The backend entry must now hold the canonical JSON rendering of 7.
        let post = cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp)
            .expect("post-run get")
            .expect("entry must exist after the re-run");
        assert_eq!(
            post,
            cook_contracts::probe_value::encode_canonical_json(&serde_json::json!(7)),
            "stale entry must be overwritten with canonical JSON"
        );
    }

    // G4/G5 invalidation test: changing a declared env var changes the probe's
    // fingerprint, causing a cache miss even when a prior entry exists under
    // the old fingerprint.
    #[test]
    fn probe_fingerprint_changes_invalidate_cache() {
        let (_wd, _tmp) = tmp_dir();
        let wd = _wd.clone();
        let cache_ctx = make_cache_ctx(&_tmp);
        let produce = "return 'result'";
        let env_var = "PROBE_TEST_VAR";

        // Build ProbeUnit for env_val="first".
        let pu_v1 = probe_unit_with_env("test:inv", produce, env_var);
        let fp_v1 = {
            let inputs = cook_fingerprint::resolve_probe_inputs(
                &pu_v1,
                &wd,
                &|name| if name == env_var { Some("first".into()) } else { None },
                &BTreeMap::new(),
            )
            .unwrap();
            cook_fingerprint::compute_probe_fingerprint(&inputs)
        };

        // Build ProbeUnit for env_val="second".
        let pu_v2 = probe_unit_with_env("test:inv", produce, env_var);
        let fp_v2 = {
            let inputs = cook_fingerprint::resolve_probe_inputs(
                &pu_v2,
                &wd,
                &|name| if name == env_var { Some("second".into()) } else { None },
                &BTreeMap::new(),
            )
            .unwrap();
            cook_fingerprint::compute_probe_fingerprint(&inputs)
        };
        assert_ne!(fp_v1, fp_v2, "fingerprints must differ when env var changes");

        // --- Run 1: env_val="first" → cache miss → populate backend under fp_v1 ---
        {
            let mut dag = Dag::new();
            let node_id = dag
                .add_node(
                    probe_work_node_with_env("test:inv", produce, env_var, "first", wd.clone()),
                    &[],
                )
                .unwrap();
            let mut by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = BTreeMap::new();
            by_node.insert(node_id, pu_v1);

            let result = execute_dag(
                dag,
                2,
                BTreeMap::new(),
                None,
                cache_ctx.clone(),
                None,
                &BTreeMap::new(),
                &[],
                &by_node,
                std::sync::Arc::new(BTreeMap::new()),
            );
            assert!(result.is_ok(), "run1 expected Ok, got: {result:?}");
        }

        // Verify fp_v1 is now in the backend.
        assert!(
            cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp_v1)
                .unwrap()
                .is_some(),
            "fp_v1 must be in backend after run1"
        );
        // fp_v2 must NOT be in the backend yet.
        assert!(
            cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp_v2)
                .unwrap()
                .is_none(),
            "fp_v2 must not be in backend before run2"
        );

        // --- Run 2: env_val="second" → different fingerprint → cache miss ---
        {
            use std::sync::mpsc;
            let mut dag = Dag::new();
            let node_id = dag
                .add_node(
                    probe_work_node_with_env("test:inv", produce, env_var, "second", wd.clone()),
                    &[],
                )
                .unwrap();
            let mut by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = BTreeMap::new();
            by_node.insert(node_id, pu_v2);

            let (tx, rx) = mpsc::channel();
            let result = execute_dag(
                dag,
                2,
                BTreeMap::new(),
                Some(tx),
                cache_ctx.clone(),
                None,
                &BTreeMap::new(),
                &[],
                &by_node,
                std::sync::Arc::new(BTreeMap::new()),
            );
            assert!(result.is_ok(), "run2 expected Ok, got: {result:?}");

            // run2 must NOT have emitted a NodeCacheHit — the env change must
            // force a miss even though fp_v1 is in the backend.
            let events: Vec<_> = rx.try_iter().collect();
            let cache_hits: Vec<_> = events
                .iter()
                .filter(|e| matches!(e, EngineEvent::NodeCacheHit { .. }))
                .collect();
            assert_eq!(
                cache_hits.len(),
                0,
                "run2 must not cache-hit because env var changed; events: {events:#?}"
            );
        }

        // After run2, fp_v2 must now exist in the backend as well (G5 persisted it).
        assert!(
            cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp_v2)
                .unwrap()
                .is_some(),
            "fp_v2 must be in backend after run2"
        );
    }

    // ---------------------------------------------------------------------------
    // normalize_glob_pattern tests — CS-0085 trailing-** normalisation
    // ---------------------------------------------------------------------------

    #[test]
    fn normalize_glob_pattern_appends_star_after_trailing_star_star() {
        assert_eq!(super::normalize_glob_pattern("build/**").as_ref(), "build/**/*");
        assert_eq!(super::normalize_glob_pattern(".next/**").as_ref(), ".next/**/*");
        assert_eq!(super::normalize_glob_pattern("apps/web/.next/**").as_ref(), "apps/web/.next/**/*");
    }

    #[test]
    fn normalize_glob_pattern_handles_bare_double_star() {
        assert_eq!(super::normalize_glob_pattern("**").as_ref(), "**/*");
    }

    #[test]
    fn normalize_glob_pattern_passes_through_non_trailing_double_star() {
        assert_eq!(super::normalize_glob_pattern("**/lib/*.so").as_ref(), "**/lib/*.so");
        assert_eq!(super::normalize_glob_pattern("src/**/*.c").as_ref(), "src/**/*.c");
    }

    #[test]
    fn normalize_glob_pattern_passes_through_non_glob_patterns() {
        assert_eq!(super::normalize_glob_pattern("*.c").as_ref(), "*.c");
        assert_eq!(super::normalize_glob_pattern("file?.txt").as_ref(), "file?.txt");
        assert_eq!(super::normalize_glob_pattern("build/main.o").as_ref(), "build/main.o");
    }

    #[test]
    fn resolve_output_paths_handles_trailing_double_star() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wd = tmp.path();
        std::fs::create_dir_all(wd.join("build/sub")).unwrap();
        std::fs::write(wd.join("build/a.o"), b"a").unwrap();
        std::fs::write(wd.join("build/sub/b.o"), b"b").unwrap();

        let resolved = super::resolve_output_paths(
            &["build/**".to_string()],
            wd,
        );
        let mut paths = resolved.clone();
        paths.sort();
        assert_eq!(paths, vec!["build/a.o".to_string(), "build/sub/b.o".to_string()],
            "trailing-** normalization should match files at any depth");
    }

    #[test]
    fn resolve_output_paths_deduplicates_overlap() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wd = tmp.path();
        std::fs::create_dir_all(wd.join("build")).unwrap();
        std::fs::write(wd.join("build/a.o"), b"a").unwrap();
        std::fs::write(wd.join("build/b.o"), b"b").unwrap();

        let resolved = super::resolve_output_paths(
            &["build/**".to_string(), "build/a.o".to_string()],
            wd,
        );
        let mut paths = resolved.clone();
        paths.sort();
        assert_eq!(paths.len(), 2, "overlapping literal+glob should dedupe");
        assert_eq!(paths, vec!["build/a.o".to_string(), "build/b.o".to_string()]);
    }

    #[test]
    fn resolve_output_paths_empty_glob_match_is_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wd = tmp.path();
        let resolved = super::resolve_output_paths(
            &["build/**".to_string()],
            wd,
        );
        assert!(resolved.is_empty(),
            "glob matching nothing returns empty Vec; §17.6 item 3 says this MUST NOT be an error");
    }

    #[test]
    fn resolve_output_paths_deduplicates_duplicate_literals() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wd = tmp.path();
        std::fs::write(wd.join("main.o"), b"obj").unwrap();
        let resolved = super::resolve_output_paths(
            &["main.o".to_string(), "main.o".to_string()],
            wd,
        );
        assert_eq!(resolved, vec!["main.o".to_string()],
            "duplicate literal entries must dedupe to a single entry per §17.6 item 1");
    }

    #[test]
    fn resolve_output_paths_expands_directory_output() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pkg/sub")).unwrap();
        std::fs::write(root.join("pkg/a.js"), b"a").unwrap();
        std::fs::write(root.join("pkg/sub/b.wasm"), b"b").unwrap();

        let resolved = super::resolve_output_paths(&["pkg/".to_string()], root);
        let set: std::collections::BTreeSet<&str> = resolved.iter().map(|s| s.as_str()).collect();
        assert!(set.contains("pkg/a.js"));
        assert!(set.contains("pkg/sub/b.wasm"));
        assert_eq!(set.len(), 2); // files only (CS-0064), directory entries dropped
    }
}
