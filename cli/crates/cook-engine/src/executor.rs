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

use cook_cache::{CacheContext, ThreadSafeCacheManager};
use cook_contracts::WorkPayload;
use cook_fingerprint::{
    artifact_key, cloud_key, needs_rebuild_cook, ArtifactMeta, CloudKeyInputs, RebuildResult,
    RestoreCtx, CACHE_VERSION,
};
use cook_dag::Dag;
use cook_luaotp::{WorkItem, WorkerPool};

use crate::{EngineError, EngineEvent, NodeKind, RecipeKind, WorkNode};

// ---------------------------------------------------------------------------
// RecipeTracker
// ---------------------------------------------------------------------------

struct RecipeTracker {
    start: Instant,
    total_nodes: usize,
    completed_nodes: usize,
    cached_nodes: usize,
    has_failure: bool,
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
pub fn execute_dag(
    dag: Dag<WorkNode>,
    num_workers: usize,
    cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    event_tx: Option<mpsc::Sender<EngineEvent>>,
    cache_ctx: Arc<CacheContext>,
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
    let (pool, rx) = WorkerPool::spawn(num_workers);

    let mut cancelled = vec![false; total];
    let mut pending: usize = 0; // how many work results we're waiting for
    let mut failures: Vec<(usize, String, String)> = Vec::new();
    let mut test_results: Vec<crate::TestResult> = Vec::new();

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
        if let Some(tracker) = trackers.get_mut(recipe_name) {
            tracker.completed_nodes += 1;
            if is_cached {
                tracker.cached_nodes += 1;
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
    // TODO(Phase 4/5): also push a crate::TestResult { outcome: Blocked } for
    // each cancelled test node. Currently blocked by the fact that cancel_subtree
    // is a nested fn without direct access to the outer test_results Vec.
    // The TestBlocked event is emitted so Phase 4 reporters can track it.
    fn cancel_subtree(
        dag: &Dag<WorkNode>,
        node_id: usize,
        cancelled: &mut Vec<bool>,
        event_tx: &Option<mpsc::Sender<EngineEvent>>,
        trackers: &mut BTreeMap<String, RecipeTracker>,
        upstream_name: &str,
    ) {
        if cancelled[node_id] {
            return;
        }
        cancelled[node_id] = true;
        let node = dag.node(node_id);
        let node_name = node
            .payload()
            .payload
            .as_ref()
            .map(|p| p.display_name())
            .unwrap_or_else(|| node.payload().recipe_name.clone());
        emit(
            event_tx,
            EngineEvent::NodeSkipped {
                recipe: node.payload().recipe_name.clone(),
                node_name: node_name.clone(),
            },
        );
        // Emit TestBlocked for test-step nodes whose upstream cook step failed.
        if matches!(node.payload().payload, Some(WorkPayload::Test { .. })) {
            let test_id = crate::id::parse_test_id(&node_name);
            emit(
                event_tx,
                EngineEvent::TestBlocked {
                    id: test_id,
                    upstream: upstream_name.to_string(),
                },
            );
        }
        finish_recipe_node(
            trackers,
            &node.payload().recipe_name,
            false,
            false,
            event_tx,
        );
        for &dep_id in dag.node(node_id).dependents() {
            cancel_subtree(dag, dep_id, cancelled, event_tx, trackers, &node_name);
        }
    }

    // ----- helper: check cache for a work node -----
    // Returns true if the node can be skipped (cache hit). When `cache_ctx`
    // exposes a backend, a hit-but-drifted entry is restored from the
    // artifact store rather than rebuilt (2026-05-02 addendum spec §5.2).
    fn check_node_cache(
        work_node: &WorkNode,
        cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
        cache_ctx: &CacheContext,
    ) -> bool {
        let meta = match &work_node.cache_meta {
            Some(m) => m,
            None => return false,
        };
        if meta.output_paths.is_empty() {
            return false;
        }
        let cm = match cache_managers.get(&work_node.recipe_name) {
            Some(cm) => cm,
            None => return false,
        };
        let cache = cm.get_or_load(&meta.recipe_name);
        let entry = cache.steps.get(&meta.cache_key);
        let input_refs: Vec<&str> = meta.input_paths.iter().map(|s| s.as_str()).collect();
        let current_outputs: Vec<&str> = meta.output_paths.iter().map(|s| s.as_str()).collect();
        let recipe_namespace = format!(
            "{}/{}::{}",
            meta.project_id, meta.cookfile_path, meta.recipe_name
        );
        let restore_ctx = RestoreCtx {
            backend: cache_ctx.backend.as_ref(),
            recipe_namespace: &recipe_namespace,
        };
        let (result, updated) = needs_rebuild_cook(
            entry,
            &input_refs,
            &current_outputs,
            meta.command_hash,
            meta.context_hash,
            meta.env_contribution,
            &work_node.working_dir,
            Some(&restore_ctx),
            meta.discovered_inputs.as_ref(),
        );
        if matches!(result, RebuildResult::Skip) {
            if let Some(updated_entry) = updated {
                cm.update_step(&meta.recipe_name, &meta.cache_key, updated_entry);
            }
            true
        } else {
            false
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
            Some(payload) => {
                // Check cache before executing
                if check_node_cache(work_node, cache_managers, cache_ctx) {
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
                        );
                    }
                    return submitted;
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
                if let WorkPayload::Test { test_name, .. } = payload {
                    let test_id = crate::id::parse_test_id(&format!(
                        "{}:{}",
                        work_node.recipe_name,
                        test_name
                    ));
                    emit(
                        event_tx,
                        EngineEvent::TestStarted {
                            id: test_id,
                            recipe: work_node.recipe_name.clone(),
                            name: test_name.clone(),
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
                        cancel_subtree(dag, dep_id, cancelled, event_tx, trackers, &payload.display_name());
                    }
                    return 0;
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

                        // Update cache if needed
                        if let Some(meta) = &dag.node(id).payload().cache_meta {
                            if let Some(cm) = cache_managers.get(&dag.node(id).payload().recipe_name) {
                                match cm.record_completion(&meta.recipe_name, &meta.cache_key, meta, &dag.node(id).payload().working_dir) {
                                    Ok(step_entry) => {
                                        // Post-execution augmentation: parse the just-written
                                        // depfile and append discovered FileRecords to
                                        // step_entry.inputs, then persist the augmented entry.
                                        let mut step_entry = step_entry;
                                        if let Some(di) = &meta.discovered_inputs {
                                            let working_dir = &dag.node(id).payload().working_dir;
                                            let abs_depfile = working_dir.join(&di.from);
                                            let source_for_skip = meta
                                                .input_paths
                                                .first()
                                                .map(String::as_str)
                                                .unwrap_or("");
                                            match cook_cache::parse_make_depfile(
                                                &abs_depfile,
                                                source_for_skip,
                                                working_dir,
                                            ) {
                                                Ok(discovered_paths) => {
                                                    match cook_cache::collect_records_public(&discovered_paths, working_dir) {
                                                        Ok(records) => {
                                                            for rec in records {
                                                                step_entry.inputs.push(rec);
                                                            }
                                                            // clone: step_entry.inputs is borrowed below for cloud_key composition.
                                                            cm.update_step(
                                                                &meta.recipe_name,
                                                                &meta.cache_key,
                                                                step_entry.clone(),
                                                            );
                                                        }
                                                        Err(p) => {
                                                            tracing::warn!(
                                                                "discovered-inputs: failed to hash discovered path '{}'", p
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

                                        let recipe_namespace = format!(
                                            "{}/{}::{}",
                                            meta.project_id, meta.cookfile_path, meta.recipe_name
                                        );

                                        let key_inputs = CloudKeyInputs {
                                            schema_version: CACHE_VERSION,
                                            recipe_namespace: &recipe_namespace,
                                            command_hash: meta.command_hash,
                                            context_hash: meta.context_hash,
                                            env_contribution: meta.env_contribution,
                                            sorted_input_content_hashes: &sorted_hashes,
                                        };
                                        let cloud_k = cloud_key(&key_inputs);

                                        // Upload one artifact per declared output (2026-05-02 addendum
                                        // spec §5.1). Each artifact is keyed by
                                        // artifact_key(cloud_key, idx, path) so a future cache hit can
                                        // restore them all independently.
                                        for (out_idx, output_path) in meta.output_paths.iter().enumerate() {
                                            let abs_output = dag.node(id).payload().working_dir.join(output_path);
                                            let bytes = match std::fs::read(&abs_output) {
                                                Ok(b) => b,
                                                Err(_) => continue,
                                            };
                                            let artifact_k = artifact_key(
                                                &cloud_k,
                                                out_idx as u32,
                                                output_path,
                                            );
                                            let mut artifact_meta = ArtifactMeta {
                                                recipe_namespace: recipe_namespace.clone(),
                                                command_hash: meta.command_hash,
                                                context_hash: meta.context_hash,
                                                env_contribution: meta.env_contribution,
                                                schema_version: CACHE_VERSION,
                                                size_bytes: bytes.len() as u64,
                                                tags: std::collections::BTreeSet::new(),
                                                consulted_env_keys: meta.consulted_env.keys().cloned().collect(),
                                                output_index: out_idx as u32,
                                                output_path: output_path.clone(),
                                                // CS-0054: stamped by the backend on put.
                                                content_hash: ArtifactMeta::zero_content_hash(),
                                            };
                                            if let Err(e) = cook_cache::backend::put_bytes(
                                                cache_ctx.backend.as_ref(),
                                                &artifact_k,
                                                &bytes,
                                                &mut artifact_meta,
                                            ) {
                                                tracing::warn!("cache backend put failed for {}: {}", output_path, e);
                                            }
                                        }

                                        // Upload the depfile as an implicit artifact at index
                                        // outputs.len() so a future restore can pull it back.
                                        if let Some(di) = &meta.discovered_inputs {
                                            let depfile_idx = meta.output_paths.len() as u32;
                                            let working_dir = &dag.node(id).payload().working_dir;
                                            let abs_depfile = working_dir.join(&di.from);
                                            match std::fs::read(&abs_depfile) {
                                                Ok(bytes) => {
                                                    let artifact_k = artifact_key(
                                                        &cloud_k,
                                                        depfile_idx,
                                                        &di.from,
                                                    );
                                                    let mut artifact_meta = ArtifactMeta {
                                                        recipe_namespace: recipe_namespace.clone(),
                                                        command_hash: meta.command_hash,
                                                        context_hash: meta.context_hash,
                                                        env_contribution: meta.env_contribution,
                                                        schema_version: CACHE_VERSION,
                                                        size_bytes: bytes.len() as u64,
                                                        tags: std::collections::BTreeSet::new(),
                                                        consulted_env_keys: meta
                                                            .consulted_env
                                                            .keys()
                                                            .cloned()
                                                            .collect(),
                                                        output_index: depfile_idx,
                                                        output_path: di.from.clone(),
                                                        // CS-0054: stamped by the backend on put.
                                                        content_hash: ArtifactMeta::zero_content_hash(),
                                                    };
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
                                                Err(e) => {
                                                    tracing::warn!(
                                                        "discovered-inputs: depfile '{}' not found after execution: {e}",
                                                        di.from
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("cache: skipping record for {}::{}: {e}", meta.recipe_name, meta.cache_key);
                                    }
                                }
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
            if let Some(meta) = &dag.node(result.id).payload().cache_meta {
                if let Some(cm) = cache_managers.get(&dag.node(result.id).payload().recipe_name) {
                    match cm.record_completion(&meta.recipe_name, &meta.cache_key, meta, &dag.node(result.id).payload().working_dir) {
                        Ok(step_entry) => {
                            // Post-execution augmentation: parse the just-written
                            // depfile and append discovered FileRecords to
                            // step_entry.inputs, then persist the augmented entry.
                            let mut step_entry = step_entry;
                            if let Some(di) = &meta.discovered_inputs {
                                let working_dir = &dag.node(result.id).payload().working_dir;
                                let abs_depfile = working_dir.join(&di.from);
                                let source_for_skip = meta
                                    .input_paths
                                    .first()
                                    .map(String::as_str)
                                    .unwrap_or("");
                                match cook_cache::parse_make_depfile(
                                    &abs_depfile,
                                    source_for_skip,
                                    working_dir,
                                ) {
                                    Ok(discovered_paths) => {
                                        match cook_cache::collect_records_public(&discovered_paths, working_dir) {
                                            Ok(records) => {
                                                for rec in records {
                                                    step_entry.inputs.push(rec);
                                                }
                                                // clone: step_entry.inputs is borrowed below for cloud_key composition.
                                                cm.update_step(
                                                    &meta.recipe_name,
                                                    &meta.cache_key,
                                                    step_entry.clone(),
                                                );
                                            }
                                            Err(p) => {
                                                tracing::warn!(
                                                    "discovered-inputs: failed to hash discovered path '{}'", p
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

                            let recipe_namespace = format!(
                                "{}/{}::{}",
                                meta.project_id, meta.cookfile_path, meta.recipe_name
                            );

                            let key_inputs = CloudKeyInputs {
                                schema_version: CACHE_VERSION,
                                recipe_namespace: &recipe_namespace,
                                command_hash: meta.command_hash,
                                context_hash: meta.context_hash,
                                env_contribution: meta.env_contribution,
                                sorted_input_content_hashes: &sorted_hashes,
                            };
                            let cloud_k = cloud_key(&key_inputs);

                            // Upload one artifact per declared output (2026-05-02 addendum
                            // spec §5.1).
                            for (out_idx, output_path) in meta.output_paths.iter().enumerate() {
                                let abs_output = dag.node(result.id).payload().working_dir.join(output_path);
                                let bytes = match std::fs::read(&abs_output) {
                                    Ok(b) => b,
                                    Err(_) => continue,
                                };
                                let artifact_k = artifact_key(
                                    &cloud_k,
                                    out_idx as u32,
                                    output_path,
                                );
                                let mut artifact_meta = ArtifactMeta {
                                    recipe_namespace: recipe_namespace.clone(),
                                    command_hash: meta.command_hash,
                                    context_hash: meta.context_hash,
                                    env_contribution: meta.env_contribution,
                                    schema_version: CACHE_VERSION,
                                    size_bytes: bytes.len() as u64,
                                    tags: std::collections::BTreeSet::new(),
                                    consulted_env_keys: meta.consulted_env.keys().cloned().collect(),
                                    output_index: out_idx as u32,
                                    output_path: output_path.clone(),
                                    // CS-0054: stamped by the backend on put.
                                    content_hash: ArtifactMeta::zero_content_hash(),
                                };
                                if let Err(e) = cook_cache::backend::put_bytes(
                                    cache_ctx.backend.as_ref(),
                                    &artifact_k,
                                    &bytes,
                                    &mut artifact_meta,
                                ) {
                                    tracing::warn!("cache backend put failed for {}: {}", output_path, e);
                                }
                            }

                            // Upload the depfile as an implicit artifact at index
                            // outputs.len() so a future restore can pull it back.
                            if let Some(di) = &meta.discovered_inputs {
                                let depfile_idx = meta.output_paths.len() as u32;
                                let working_dir = &dag.node(result.id).payload().working_dir;
                                let abs_depfile = working_dir.join(&di.from);
                                match std::fs::read(&abs_depfile) {
                                    Ok(bytes) => {
                                        let artifact_k = artifact_key(
                                            &cloud_k,
                                            depfile_idx,
                                            &di.from,
                                        );
                                        let mut artifact_meta = ArtifactMeta {
                                            recipe_namespace: recipe_namespace.clone(),
                                            command_hash: meta.command_hash,
                                            context_hash: meta.context_hash,
                                            env_contribution: meta.env_contribution,
                                            schema_version: CACHE_VERSION,
                                            size_bytes: bytes.len() as u64,
                                            tags: std::collections::BTreeSet::new(),
                                            consulted_env_keys: meta
                                                .consulted_env
                                                .keys()
                                                .cloned()
                                                .collect(),
                                            output_index: depfile_idx,
                                            output_path: di.from.clone(),
                                            // CS-0054: stamped by the backend on put.
                                            content_hash: ArtifactMeta::zero_content_hash(),
                                        };
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
                                    Err(e) => {
                                        tracing::warn!(
                                            "discovered-inputs: depfile '{}' not found after execution: {e}",
                                            di.from
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("cache: skipping record for {}::{}: {e}", meta.recipe_name, meta.cache_key);
                        }
                    }
                }
            }

            finish_recipe_node(&mut recipe_trackers, &recipe_name, false, false, &event_tx);

            // Translate test output to a TestResult and emit TestPassed event.
            if let Some(to) = result.test_output {
                // Build the TestId in `<recipe>:<test_name>` format so the
                // reporter can extract the recipe portion. `result.node_name`
                // is the raw display_name (= test_name alone); `recipe_name`
                // carries the fully-qualified recipe name.
                let id = crate::id::parse_test_id(&format!(
                    "{}:{}",
                    recipe_name,
                    to.test_name,
                ));
                let namespace = crate::id::id_namespace(&id);
                let recipe = crate::id::id_recipe(&id);
                let duration = Duration::from_secs_f64(to.duration);
                emit(
                    &event_tx,
                    EngineEvent::TestPassed {
                        id: id.clone(),
                        duration,
                        cached: false,
                        stdout: to.stdout.clone(),
                        stderr: to.stderr.clone(),
                    },
                );
                test_results.push(crate::TestResult {
                    id,
                    namespace,
                    recipe,
                    name: to.test_name.clone(),
                    suite: to.suite_name.clone(),
                    iteration_item: None,
                    outcome: crate::TestOutcome::Passed,
                    duration,
                    from_cache: false,
                    stdout: to.stdout,
                    stderr: to.stderr,
                    fingerprint: None,
                    blocked_by: None,
                    should_fail: to.should_fail,
                    timed_out: false,
                });
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
                // Build the TestId in `<recipe>:<test_name>` format (same as TestStarted).
                let id = crate::id::parse_test_id(&format!(
                    "{}:{}",
                    recipe_name,
                    to.test_name,
                ));
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
                            },
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
                    iteration_item: None,
                    outcome,
                    duration,
                    from_cache: false,
                    stdout: to.stdout.clone(),
                    stderr: to.stderr.clone(),
                    fingerprint: None,
                    blocked_by: None,
                    should_fail: to.should_fail,
                    timed_out: to.timed_out,
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
        Ok(test_results)
    } else {
        Err(EngineError::TaskFailures {
            count: failures.len(),
            failures,
        })
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
        use cook_fingerprint::{EnvDenylist, ExecutionContext};
        Arc::new(CacheContext {
            exec_ctx: Arc::new(ExecutionContext::probe()),
            denylist: Arc::new(EnvDenylist::baseline()),
            backend: Arc::new(LocalBackend::new(tmp.path().join("cloud"))),
            cloud_config: Arc::new(CloudConfig::default()),
            project_root: tmp.path().to_path_buf(),
            project_id: "test".to_string(),
        })
    }

    // 1. Single node succeeds
    #[test]
    fn test_executor_runs_single_node() {
        let (wd, _tmp) = tmp_dir();
        let cache_ctx = make_cache_ctx(&_tmp);
        let mut dag = Dag::new();
        dag.add_node(work_node(shell("true"), "single", wd), &[]).unwrap();

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx);
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

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx);
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

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx);
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
        let result = execute_dag(dag, 4, BTreeMap::new(), None, cache_ctx);
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
        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx);
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

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx);
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

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx);
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

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx);
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
        );

        let (tx, rx) = mpsc::channel::<EngineEvent>();
        let result = execute_dag(dag, 1, BTreeMap::new(), Some(tx), cache_ctx);
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
            context_hash: 0,
            env_contribution: 0,
            consulted_env: BTreeMap::new(),
            discovered_inputs: None,
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

        let result = execute_dag(dag, 1, BTreeMap::new(), None, cache_ctx);
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

        let result = execute_dag(dag, 1, BTreeMap::new(), None, cache_ctx);
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

        let result = execute_dag(dag, 1, BTreeMap::new(), None, cache_ctx);
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
        let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx);
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
        let _result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx);

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
        let _result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx);

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
                },
                "shell1", wd.clone()),
            &[a]).unwrap();
        dag.add_node(
            work_node(
                WorkPayload::Interactive { cmd: "true".into(), line: 3, is_chore: true },
                "shell1", wd),
            &[b]).unwrap();

        let (tx, rx) = mpsc::channel();
        let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx);
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
                },
                "lua_chore", wd),
            &[a]).unwrap();

        let (tx, rx) = mpsc::channel();
        let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx);
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
                },
                "regular_lua", wd),
            &[]).unwrap();

        let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx);
        assert!(result.is_ok(), "got: {result:?}");
    }
}
