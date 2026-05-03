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
        let restore_ctx = cook_cache::RestoreCtx {
            backend: cache_ctx.backend.as_ref(),
            recipe_namespace: &recipe_namespace,
        };
        let (result, updated) = cook_cache::needs_rebuild_cook(
            entry,
            &input_refs,
            &current_outputs,
            meta.command_hash,
            meta.context_hash,
            meta.env_contribution,
            &work_node.working_dir,
            Some(&restore_ctx),
        );
        if matches!(result, cook_cache::RebuildResult::Skip) {
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

                // Convert BTreeMap env_vars to HashMap for WorkItem
                let env_vars_hashmap: std::collections::HashMap<String, String> =
                    work_node.env_vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

                pool.submit(WorkItem {
                    id,
                    payload: payload.clone(),
                    recipe_name: work_node.recipe_name.clone(),
                    working_dir: work_node.working_dir.clone(),
                    env_vars: env_vars_hashmap,
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

                let result =
                    run_interactive_on_main(cmd, *line, &work_node.working_dir, &work_node.env_vars);
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

                                    let key_inputs = cook_cache::backend::CloudKeyInputs {
                                        schema_version: cook_cache::store::CACHE_VERSION,
                                        recipe_namespace: &recipe_namespace,
                                        command_hash: meta.command_hash,
                                        context_hash: meta.context_hash,
                                        env_contribution: meta.env_contribution,
                                        sorted_input_content_hashes: &sorted_hashes,
                                    };
                                    let cloud_k = cook_cache::backend::cloud_key(&key_inputs);

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
                                        let artifact_k = cook_cache::backend::artifact_key(
                                            &cloud_k,
                                            out_idx as u32,
                                            output_path,
                                        );
                                        let artifact_meta = cook_cache::backend::ArtifactMeta {
                                            recipe_namespace: recipe_namespace.clone(),
                                            command_hash: meta.command_hash,
                                            context_hash: meta.context_hash,
                                            env_contribution: meta.env_contribution,
                                            schema_version: cook_cache::store::CACHE_VERSION,
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
            // Emit output lines
            for line in &result.output_lines {
                emit(
                    &event_tx,
                    EngineEvent::OutputLine {
                        recipe: recipe_name.clone(),
                        line: line.clone(),
                        is_stderr: false,
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

                            let key_inputs = cook_cache::backend::CloudKeyInputs {
                                schema_version: cook_cache::store::CACHE_VERSION,
                                recipe_namespace: &recipe_namespace,
                                command_hash: meta.command_hash,
                                context_hash: meta.context_hash,
                                env_contribution: meta.env_contribution,
                                sorted_input_content_hashes: &sorted_hashes,
                            };
                            let cloud_k = cook_cache::backend::cloud_key(&key_inputs);

                            // Upload one artifact per declared output (2026-05-02 addendum
                            // spec §5.1).
                            for (out_idx, output_path) in meta.output_paths.iter().enumerate() {
                                let abs_output = dag.node(result.id).payload().working_dir.join(output_path);
                                let bytes = match std::fs::read(&abs_output) {
                                    Ok(b) => b,
                                    Err(_) => continue,
                                };
                                let artifact_k = cook_cache::backend::artifact_key(
                                    &cloud_k,
                                    out_idx as u32,
                                    output_path,
                                );
                                let artifact_meta = cook_cache::backend::ArtifactMeta {
                                    recipe_namespace: recipe_namespace.clone(),
                                    command_hash: meta.command_hash,
                                    context_hash: meta.context_hash,
                                    env_contribution: meta.env_contribution,
                                    schema_version: cook_cache::store::CACHE_VERSION,
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
                );
            }
        } else {
            // Emit output lines even on failure
            for line in &result.output_lines {
                emit(
                    &event_tx,
                    EngineEvent::OutputLine {
                        recipe: recipe_name.clone(),
                        line: line.clone(),
                        is_stderr: false,
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
            context::ExecutionContext, envkey::EnvDenylist,
        };
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
}
