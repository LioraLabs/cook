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

use crate::{EngineError, EngineEvent, WorkNode};

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
/// Returns `Ok(())` if every node completed successfully (or was pre-satisfied),
/// or `Err(EngineError)` listing each failed node.
pub fn execute_dag(
    dag: Dag<WorkNode>,
    num_workers: usize,
    cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    event_tx: Option<mpsc::Sender<EngineEvent>>,
    cache_ctx: Arc<CacheContext>,
) -> Result<(), EngineError> {
    // Empty DAG — nothing to do.
    if dag.is_empty() {
        return Ok(());
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
                    emit(
                        event_tx,
                        EngineEvent::RecipeCompleted {
                            name: recipe_name.to_string(),
                            elapsed,
                            cached_nodes: tracker.cached_nodes,
                            total_nodes: tracker.total_nodes,
                        },
                    );
                }
            }
        }
    }

    // ----- helper: cancel a node and all its transitive dependents -----
    fn cancel_subtree(
        dag: &Dag<WorkNode>,
        node_id: usize,
        cancelled: &mut Vec<bool>,
        event_tx: &Option<mpsc::Sender<EngineEvent>>,
        trackers: &mut BTreeMap<String, RecipeTracker>,
    ) {
        if cancelled[node_id] {
            return;
        }
        cancelled[node_id] = true;
        let node = dag.node(node_id);
        emit(
            event_tx,
            EngineEvent::NodeSkipped {
                recipe: node.payload().recipe_name.clone(),
                node_name: node
                    .payload()
                    .payload
                    .as_ref()
                    .map(|p| p.display_name())
                    .unwrap_or_else(|| node.payload().recipe_name.clone()),
            },
        );
        finish_recipe_node(
            trackers,
            &node.payload().recipe_name,
            false,
            false,
            event_tx,
        );
        for &dep_id in dag.node(node_id).dependents() {
            cancel_subtree(dag, dep_id, cancelled, event_tx, trackers);
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
                    },
                );

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
                        cancel_subtree(dag, dep_id, cancelled, event_tx, trackers);
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
        while pending == 0 && !interactive_queue.is_empty() {
            let id = interactive_queue.remove(0);
            if cancelled[id] {
                finished += 1;
                continue;
            }

            let node = dag.node(id);
            let work_node = node.payload();
            if let Some(payload @ WorkPayload::Interactive { cmd, line }) = &work_node.payload {
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
                    },
                );

                if success {
                    emit(
                        &event_tx,
                        EngineEvent::NodeCompleted {
                            recipe: recipe_name.clone(),
                            node_name: node_name.clone(),
                            elapsed: interactive_elapsed,
                        },
                    );

                    // Update cache if needed
                    if let Some(meta) = &dag.node(id).payload().cache_meta {
                        if let Some(cm) = cache_managers.get(&dag.node(id).payload().recipe_name) {
                            match cm.record_completion(&meta.recipe_name, &meta.cache_key, meta, &dag.node(id).payload().working_dir) {
                                Ok(step_entry) => {
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
                                        let artifact_meta = ArtifactMeta {
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
                                        };
                                        if let Err(e) = cache_ctx.backend.put(&artifact_k, &bytes, &artifact_meta) {
                                            tracing::warn!("cache backend put failed for {}: {}", output_path, e);
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
                        );
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
                },
            );

            // Update cache entry if this node has cache metadata
            if let Some(meta) = &dag.node(result.id).payload().cache_meta {
                if let Some(cm) = cache_managers.get(&dag.node(result.id).payload().recipe_name) {
                    match cm.record_completion(&meta.recipe_name, &meta.cache_key, meta, &dag.node(result.id).payload().working_dir) {
                        Ok(step_entry) => {
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
                                let artifact_meta = ArtifactMeta {
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
                                };
                                if let Err(e) = cache_ctx.backend.put(&artifact_k, &bytes, &artifact_meta) {
                                    tracing::warn!("cache backend put failed for {}: {}", output_path, e);
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

            let err_msg = result
                .error
                .unwrap_or_else(|| "unknown error".to_string());

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
                );
            }
        }
    }

    pool.shutdown();

    // Flush cache updates to disk
    for cm in cache_managers.values() {
        let _ = cm.flush_all();
    }

    if failures.is_empty() {
        Ok(())
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
}
