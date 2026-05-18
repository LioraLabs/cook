//! Unified engine entry point for both single-Cookfile and workspace builds.
//!
//! [`run`] takes a fully-built [`RegisteredWorkspace`] along with the
//! recipe-level dependency edges for the reachable target closure, then
//! executes the build by constructing a single unified work-unit DAG across
//! every reachable recipe and walking it with the shared executor pool.
//! Cross-recipe edges live directly on the unified DAG; there is no per-wave
//! register / DAG / execute loop (SHI-222 Phase 4).
//!
//! Callers build the [`RegisteredWorkspace`] via `pipeline::register_workspace`
//! (or `register_single_cookfile` for single-Cookfile inputs), which runs
//! `cook_register::register_cookfile` once per Cookfile and merges per-import
//! results.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;

use cook_cache::{
    backend::LocalBackend, cache_ctx::CacheContext, cloud_backend::CloudBackend,
    cloud_config::CloudConfig, TestCache, ThreadSafeCacheManager,
};
use cook_contracts::{RecipeUnits, WorkPayload};
use cook_fingerprint::{CacheBackend, EnvDenylist, ExecutionContext, FingerprintInputs};

use crate::{
    dag_builder, executor, EngineError, EngineEvent, RecipeKind, RegisteredWorkspace,
};

// ---------------------------------------------------------------------------
// TestScope — how to scope a `cook test` invocation
// ---------------------------------------------------------------------------

/// How to scope a `cook test` invocation.
///
/// Constructed by `cook-cli` and consumed by the test-mode engine path
/// built on top of `pipeline::register_workspace`.
#[derive(Debug, Clone)]
pub enum TestScope {
    /// `cook test <recipe>` — scope to a single recipe and its dep closure.
    Recipe(String),
    /// `cook test <namespace>` — scope to an import alias's tree.
    Namespace(String),
}

/// The result of a successful engine run.
#[derive(Debug)]
pub struct RunResult {
    pub test_results: Vec<crate::TestResult>,
}

/// Split a namespaced recipe name into (prefix, local_name).
///
/// `"backend.proto.generate"` -> `("backend.proto", "generate")`
/// `"build"` -> `("", "build")`
fn split_recipe_name(name: &str) -> (String, String) {
    if let Some(dot_pos) = name.rfind('.') {
        (name[..dot_pos].to_string(), name[dot_pos + 1..].to_string())
    } else {
        (String::new(), name.to_string())
    }
}

/// Unified engine entry point.
///
/// Walks the unified work-unit DAG across every reachable recipe in
/// `registered_workspace`, then dispatches to a single
/// [`executor::execute_dag`] walk. Cross-recipe edges (coarse `deps` and
/// fine-grained `dep_edges`) are wired directly on the work-unit DAG; there
/// is no per-wave loop and no per-wave registration (SHI-222 Phase 4).
///
/// # Arguments
///
/// * `project_root` - The project root directory. Used to load
///   `.cook/cloud.toml`, probe execution context (machine identity +
///   declared-tool hashing), and compute cookfile-relative paths.
/// * `registered_workspace` - The workspace-wide aggregation of per-Cookfile
///   registration results. See [`RegisteredWorkspace`]. Phase 5 Task 5.1
///   will land a `register_workspace` helper that builds this from
///   `register_cookfile` + per-import merging; until then, callers must
///   construct it manually (test helpers are fine).
/// * `edges` - Recipe-level dependency edge map (from
///   [`crate::analyzer::dependency_edges_multi`]) for the reachable target
///   closure.
/// * `reachable` - Set of all reachable recipe names. Must equal
///   `edges.keys().cloned().collect()`.
/// * `num_jobs` - Maximum number of parallel worker threads.
/// * `rerun_patterns` - Glob patterns gating per-test cache lookup so
///   matching tests force-rerun even if a cache entry exists. Pass `&[]`
///   for non-test invocations.
/// * `on_event` - Callback invoked for each engine event (progress, errors,
///   etc.). The terminating [`EngineEvent::Finished`] event is emitted here
///   automatically; callers do not need to send one themselves.
pub fn run<F>(
    project_root: &Path,
    registered_workspace: &RegisteredWorkspace,
    edges: &BTreeMap<String, Vec<String>>,
    reachable: &BTreeSet<String>,
    num_jobs: usize,
    rerun_patterns: &[String],
    on_event: F,
) -> Result<RunResult, EngineError>
where
    F: Fn(EngineEvent) + Send + Sync,
{
    let started = std::time::Instant::now();
    let cache_ctx = match build_cache_ctx(project_root) {
        Ok(c) => c,
        Err(e) => {
            on_event(EngineEvent::Finished {
                elapsed: started.elapsed(),
                success: false,
            });
            return Err(e);
        }
    };
    let result = run_inner(
        registered_workspace,
        edges,
        reachable,
        num_jobs,
        cache_ctx,
        &on_event,
        rerun_patterns,
    );
    on_event(EngineEvent::Finished {
        elapsed: started.elapsed(),
        success: result.is_ok(),
    });
    result
}

/// Inner DAG walker. Separated from [`run`] so that the public entry point
/// owns the `Finished` event emission and the `CacheContext` construction,
/// while this function stays focused on the DAG-build → executor-dispatch
/// pipeline.
fn run_inner<F>(
    registered_workspace: &RegisteredWorkspace,
    edges: &BTreeMap<String, Vec<String>>,
    reachable: &BTreeSet<String>,
    num_jobs: usize,
    cache_ctx: Arc<CacheContext>,
    on_event: &F,
    rerun_patterns: &[String],
) -> Result<RunResult, EngineError>
where
    F: Fn(EngineEvent) + Send + Sync,
{
    // 1. Collect RecipeUnits for every reachable recipe and stamp the
    //    cross-recipe deps from the recipe-level edge map. The DAG builder
    //    wires both coarse `deps` and fine-grained `dep_edges` from this
    //    slice in a single pass.
    //
    //    Recipes are passed in topological order (derived from `edges` via
    //    a Kahn walk) so that `build_dag`'s intra-call `recipe_leaves`
    //    accumulator has every dep present when wiring cross-recipe edges.
    let topo_order = toposort_reachable(edges, reachable)?;
    let mut all_units: Vec<RecipeUnits> = Vec::with_capacity(topo_order.len());
    for name in &topo_order {
        let units = registered_workspace
            .units_by_recipe
            .get(name)
            .ok_or_else(|| EngineError::UnknownRecipe(name.clone()))?;
        let mut u = units.clone();
        if let Some(deps) = edges.get(name) {
            u.deps = deps.clone();
        }
        all_units.push(u);
    }

    // 2. Build the unified work-unit DAG.
    let dag = dag_builder::build_dag(all_units)?;

    // 3. Emit BuildStarted in topological order, then RecipeQueued for each
    //    reachable recipe. `expected_nodes` is the count of DAG nodes owned
    //    by each recipe (matches what the executor tracks).
    let recipe_node_counts: BTreeMap<String, usize> = {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for i in 0..dag.len() {
            let name = dag.node(i).payload().recipe_name.clone();
            *counts.entry(name).or_insert(0) += 1;
        }
        counts
    };

    let topos: Vec<crate::RecipeTopology> = topo_order
        .iter()
        .map(|name| crate::RecipeTopology {
            name: name.clone(),
            deps: edges.get(name).cloned().unwrap_or_default(),
            expected_nodes: recipe_node_counts.get(name).copied().unwrap_or(0),
        })
        .collect();
    let total_nodes = topos.iter().map(|t| t.expected_nodes).sum();
    on_event(EngineEvent::BuildStarted {
        recipes: topos,
        total_nodes,
    });
    for name in &topo_order {
        on_event(EngineEvent::RecipeQueued {
            name: name.clone(),
        });
    }

    // 4. Synthetic lifecycle events for zero-unit recipes (meta-targets that
    //    have no cook steps of their own, only deps). The executor never
    //    sees these recipes, so without the synthetic pair they stay stuck
    //    in `Waiting` in the progress renderer.
    {
        let recipes_in_dag: BTreeSet<String> = (0..dag.len())
            .map(|i| dag.node(i).payload().recipe_name.clone())
            .collect();
        // Map qualified-recipe-name → cook_engine::RecipeKind (Recipe/Chore).
        // The kind on `RegisteredRecipePub` is `cook_register::RecipeKind`,
        // which has the same variants but is a distinct type (Task 4.1).
        let kind_by_name: BTreeMap<&str, RecipeKind> = registered_workspace
            .names
            .iter()
            .map(|r| {
                let kind = match r.kind {
                    cook_register::RecipeKind::Recipe => RecipeKind::Recipe,
                    cook_register::RecipeKind::Chore => RecipeKind::Chore,
                };
                (r.name.as_str(), kind)
            })
            .collect();
        for name in &topo_order {
            if !recipes_in_dag.contains(name) {
                let kind = kind_by_name
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(RecipeKind::Recipe);
                on_event(EngineEvent::RecipeStarted {
                    name: name.clone(),
                    total_nodes: 0,
                });
                on_event(EngineEvent::RecipeCompleted {
                    name: name.clone(),
                    elapsed: std::time::Duration::ZERO,
                    cached_nodes: 0,
                    total_nodes: 0,
                    kind,
                });
            }
        }
    }

    // Empty DAG: every reachable recipe was zero-unit (synthetic events
    // already emitted above). Nothing else to do.
    if dag.is_empty() {
        return Ok(RunResult { test_results: vec![] });
    }

    // 5. Per-recipe cache managers. One per reachable recipe, anchored at
    //    that recipe's prefix's working_dir.
    let cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>> = reachable
        .iter()
        .map(|name| {
            let prefix = split_recipe_name(name).0;
            let wd = registered_workspace
                .working_dir_by_prefix
                .get(&prefix)
                .cloned()
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let cache_dir = wd.join(".cook").join("cache");
            (name.clone(), Arc::new(ThreadSafeCacheManager::new(cache_dir)))
        })
        .collect();

    // 6. Build per-node test fingerprints (Phase 5 fingerprint v1) and the
    //    probe_units_by_node lookup. Both are derived from the unified DAG.
    let test_cache = TestCache::new(cache_ctx.project_root.join(".cook"));
    let probe_units_by_key: BTreeMap<String, cook_contracts::ProbeUnit> = registered_workspace
        .probes
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let fingerprint_by_node: BTreeMap<usize, String> = {
        let mut fp_map = BTreeMap::new();
        for node_idx in 0..dag.len() {
            let work_node = dag.node(node_idx).payload();
            if let Some(WorkPayload::Test { .. }) = &work_node.payload {
                let env_keys: Vec<(String, String)> = work_node
                    .env_vars
                    .iter()
                    .filter(|(k, _)| !cache_ctx.denylist.is_ignored(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let tool_hashes: Vec<(String, String)> = cache_ctx
                    .exec_ctx
                    .declared_tools
                    .iter()
                    .map(|(name, path)| {
                        let hash = cook_fingerprint::hash_file(path)
                            .map(|h| format!("{h:x}"))
                            .unwrap_or_else(|| "0".to_string());
                        (name.clone(), hash)
                    })
                    .collect();
                let inputs = FingerprintInputs {
                    cook_outputs: vec![],  // Phase 5 v1 stub
                    dep_outputs: vec![],   // Phase 5 v1 stub
                    env_keys,
                    tool_hashes,
                };
                let fp = cook_fingerprint::compute_test_fingerprint(
                    work_node.payload.as_ref().expect("checked above"),
                    &inputs,
                );
                fp_map.insert(node_idx, fp);
            }
        }
        fp_map
    };

    let probe_units_by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = (0..dag.len())
        .filter_map(|node_idx| {
            let work_node = dag.node(node_idx).payload();
            if let Some(WorkPayload::Probe { key, .. }) = &work_node.payload {
                probe_units_by_key.get(key).map(|pu| (node_idx, pu.clone()))
            } else {
                None
            }
        })
        .collect();

    // 7. Execute the unified DAG. Lifecycle events (RecipeStarted /
    //    RecipeCompleted) fire from the executor's recipe-tracker bookkeeping
    //    at unit-state transitions: RecipeStarted on the first unit leaving
    //    Waiting, RecipeCompleted when the last unit finishes (success or
    //    cached) or RecipeFailed on the last completion when any unit failed.
    //    Wave-aligned firing is gone — events now reflect actual unit motion.
    //    Zero-unit (meta-target) recipes are emitted synthetically above.
    //
    //    Bridge on_event through an mpsc channel so executor can use its
    //    existing Option<Sender<EngineEvent>> interface.
    let (event_tx, event_rx) = mpsc::channel::<EngineEvent>();
    let exec_result = std::thread::scope(|s| {
        let on_event_ref = on_event;
        let handle = s.spawn(move || {
            while let Ok(event) = event_rx.recv() {
                on_event_ref(event);
            }
        });

        let exec_result = executor::execute_dag(
            dag,
            num_jobs,
            cache_managers,
            Some(event_tx),
            cache_ctx.clone(),
            Some(&test_cache),
            &fingerprint_by_node,
            rerun_patterns,
            &probe_units_by_node,
        );

        // execute_dag drops the sender end on return, so the bridge thread's
        // recv() loop exits and join() completes promptly.
        let _ = handle.join();

        exec_result
    });

    let test_results = exec_result?;
    Ok(RunResult { test_results })
}

/// Topologically sort `reachable` against the recipe-level `edges` map.
///
/// Returns recipe names in dependency-first order so that
/// [`dag_builder::build_dag`]'s intra-call `recipe_leaves` accumulator has
/// every dep present before the dependent recipe is processed. Recipes
/// missing from `edges` are placed first (no deps known).
fn toposort_reachable(
    edges: &BTreeMap<String, Vec<String>>,
    reachable: &BTreeSet<String>,
) -> Result<Vec<String>, EngineError> {
    use std::collections::VecDeque;

    // Restrict the dep set per node to deps that are in `reachable`.
    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
    let mut forward: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for name in reachable {
        deps.entry(name.clone()).or_default();
        in_degree.entry(name.clone()).or_insert(0);
    }
    for name in reachable {
        if let Some(dep_list) = edges.get(name) {
            for d in dep_list {
                if reachable.contains(d) && d != name {
                    if deps.get_mut(name).unwrap().insert(d.clone()) {
                        *in_degree.get_mut(name).unwrap() += 1;
                        forward.entry(d.clone()).or_default().insert(name.clone());
                    }
                }
            }
        }
    }

    let mut queue: VecDeque<String> = in_degree
        .iter()
        .filter_map(|(n, &deg)| if deg == 0 { Some(n.clone()) } else { None })
        .collect();
    let mut order: Vec<String> = Vec::with_capacity(reachable.len());
    while let Some(n) = queue.pop_front() {
        order.push(n.clone());
        if let Some(deps_of) = forward.get(&n) {
            for next in deps_of {
                let deg = in_degree.get_mut(next).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(next.clone());
                }
            }
        }
    }
    if order.len() != reachable.len() {
        // Only nodes with unresolved in-degree are part of (or downstream of)
        // the cycle. Naming them is far more useful than dumping the entire
        // reachable set, which can include dozens of unrelated recipes.
        let unresolved: Vec<&String> = in_degree
            .iter()
            .filter(|(_, &d)| d > 0)
            .map(|(name, _)| name)
            .collect();
        return Err(EngineError::CycleDetected(format!(
            "cycle among recipes: {:?}",
            unresolved
        )));
    }
    Ok(order)
}

/// Build a [`CacheContext`] for this build invocation.
///
/// Loads `.cook/cloud.toml`, builds the env denylist, probes the execution
/// context (machine identity + declared-tool hashing), selects either the
/// local or cloud backend, and assembles the shared `CacheContext` carried
/// by every register pass and worker.
fn build_cache_ctx(project_root: &Path) -> Result<Arc<CacheContext>, EngineError> {
    let cloud_config = CloudConfig::load_or_default(project_root)
        .map_err(|e| EngineError::CacheError(format!("invalid .cook/cloud.toml: {e}")))?;
    let mut denylist = EnvDenylist::baseline();
    denylist.extend_with(cloud_config.cache_ignore_env());
    let denylist = Arc::new(denylist);
    let exec_ctx = Arc::new(
        ExecutionContext::probe_with_declared_tools(cloud_config.cache_tools())
            .map_err(|e| EngineError::CacheError(e.to_string()))?,
    );
    if !exec_ctx.declared_tools.is_empty() {
        tracing::debug!(
            "declared cache tools: {:?}",
            exec_ctx
                .declared_tools
                .iter()
                .map(|(n, p)| format!("{n} -> {}", p.display()))
                .collect::<Vec<_>>()
        );
    }
    let cache_dir = cloud_config
        .cache_dir()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("cook")
                .join("cloud")
        });
    let backend: Arc<dyn CacheBackend> = if cloud_config.cloud.enabled {
        let endpoint = cloud_config
            .cloud
            .endpoint
            .clone()
            .expect("validated by load_or_default when cloud.enabled");
        let api_key = cloud_config
            .resolved_api_key()
            .expect("validated by load_or_default when cloud.enabled");
        tracing::debug!(
            "cache backend: cloud (endpoint={}, project={:?})",
            endpoint,
            cloud_config.cloud.project,
        );
        Arc::new(CloudBackend::new(
            endpoint,
            api_key,
            cloud_config.backend_config(),
        ))
    } else {
        tracing::debug!("cache backend: local ({})", cache_dir.display());
        Arc::new(LocalBackend::with_config(
            cache_dir,
            cloud_config.backend_config(),
        ))
    };
    if let Err(e) = backend.health() {
        tracing::warn!("cache backend unavailable: {e}; continuing with backend disabled");
    }
    let project_id = cloud_config.project_id_or_fallback(project_root);
    Ok(Arc::new(CacheContext {
        exec_ctx,
        denylist,
        backend,
        cloud_config: Arc::new(cloud_config),
        project_root: project_root.to_path_buf(),
        project_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_recipe_name_with_prefix() {
        let (prefix, local) = split_recipe_name("backend.proto.generate");
        assert_eq!(prefix, "backend.proto");
        assert_eq!(local, "generate");
    }

    #[test]
    fn test_split_recipe_name_no_prefix() {
        let (prefix, local) = split_recipe_name("build");
        assert_eq!(prefix, "");
        assert_eq!(local, "build");
    }

    #[test]
    fn test_split_recipe_name_single_dot() {
        let (prefix, local) = split_recipe_name("backend.build");
        assert_eq!(prefix, "backend");
        assert_eq!(local, "build");
    }

    fn dummy_project_root() -> std::path::PathBuf {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        std::mem::forget(dir);
        path
    }

    /// Build an empty `RegisteredWorkspace` for tests that exercise the
    /// pre-DAG-build entry paths (empty targets, finished-event emission).
    fn empty_registered_workspace() -> RegisteredWorkspace {
        RegisteredWorkspace {
            names: Vec::new(),
            units_by_recipe: BTreeMap::new(),
            probes: BTreeMap::new(),
            final_env_by_cookfile: BTreeMap::new(),
            working_dir_by_prefix: BTreeMap::new(),
            alias_dirs_by_prefix: BTreeMap::new(),
        }
    }

    #[test]
    fn test_run_empty_reachable_returns_ok_with_no_results() {
        // Empty reachable set: no DAG to walk, no synthetic lifecycle events.
        // run() should short-circuit cleanly and emit Finished{success:true}.
        let ws = empty_registered_workspace();
        let edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let reachable: BTreeSet<String> = BTreeSet::new();
        let result = run(
            &dummy_project_root(),
            &ws,
            &edges,
            &reachable,
            1,
            &[],
            |_| {},
        );
        assert!(result.is_ok());
        assert!(result.unwrap().test_results.is_empty());
    }

    #[test]
    fn test_run_unknown_recipe_in_reachable() {
        // A name present in `reachable` but absent from
        // `registered_workspace.units_by_recipe` must surface as
        // `UnknownRecipe(name)`.
        let ws = empty_registered_workspace();
        let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edges.insert("missing".into(), vec![]);
        let reachable: BTreeSet<String> = ["missing"].iter().map(|s| s.to_string()).collect();
        let result = run(
            &dummy_project_root(),
            &ws,
            &edges,
            &reachable,
            1,
            &[],
            |_| {},
        );
        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::UnknownRecipe(name) => assert_eq!(name, "missing"),
            other => panic!("expected UnknownRecipe, got: {other:?}"),
        }
    }

    #[test]
    fn test_run_emits_finished_success_on_empty_reachable() {
        let ws = empty_registered_workspace();
        let edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let reachable: BTreeSet<String> = BTreeSet::new();

        let events = std::sync::Mutex::new(Vec::new());
        let result = run(
            &dummy_project_root(),
            &ws,
            &edges,
            &reachable,
            1,
            &[],
            |event| events.lock().unwrap().push(event),
        );
        assert!(result.is_ok());

        let events = events.lock().unwrap();
        let finished = events.iter().find_map(|e| match e {
            EngineEvent::Finished { success, .. } => Some(*success),
            _ => None,
        });
        assert_eq!(
            finished,
            Some(true),
            "expected Finished{{success:true}} event"
        );
    }

    #[test]
    fn test_run_emits_finished_failure_on_unknown_recipe() {
        let ws = empty_registered_workspace();
        let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edges.insert("missing".into(), vec![]);
        let reachable: BTreeSet<String> = ["missing"].iter().map(|s| s.to_string()).collect();

        let events = std::sync::Mutex::new(Vec::new());
        let result = run(
            &dummy_project_root(),
            &ws,
            &edges,
            &reachable,
            1,
            &[],
            |event| events.lock().unwrap().push(event),
        );
        assert!(result.is_err());

        let events = events.lock().unwrap();
        let finished = events.iter().find_map(|e| match e {
            EngineEvent::Finished { success, .. } => Some(*success),
            _ => None,
        });
        assert_eq!(
            finished,
            Some(false),
            "expected Finished{{success:false}} event"
        );
    }

    #[test]
    fn test_toposort_reachable_diamond() {
        // a -> b, a -> c, b -> d, c -> d
        let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edges.insert("a".into(), vec![]);
        edges.insert("b".into(), vec!["a".into()]);
        edges.insert("c".into(), vec!["a".into()]);
        edges.insert("d".into(), vec!["b".into(), "c".into()]);
        let reachable: BTreeSet<String> =
            ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        let order = toposort_reachable(&edges, &reachable).expect("toposort");
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
    }

    #[test]
    fn test_toposort_reachable_detects_cycle() {
        let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edges.insert("a".into(), vec!["b".into()]);
        edges.insert("b".into(), vec!["a".into()]);
        let reachable: BTreeSet<String> =
            ["a", "b"].iter().map(|s| s.to_string()).collect();
        let result = toposort_reachable(&edges, &reachable);
        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::CycleDetected(msg) => {
                assert!(
                    msg.contains("\"a\""),
                    "error should name cycle node 'a', got: {msg}"
                );
                assert!(
                    msg.contains("\"b\""),
                    "error should name cycle node 'b', got: {msg}"
                );
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
    }

    #[test]
    fn test_toposort_reachable_cycle_names_only_cycle_nodes() {
        // Build a graph with a long unrelated chain (x -> y -> z, all
        // resolvable) plus a 2-node cycle (a <-> b). The error should
        // name only the cycle nodes, not the resolvable ones.
        let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edges.insert("x".into(), vec![]);
        edges.insert("y".into(), vec!["x".into()]);
        edges.insert("z".into(), vec!["y".into()]);
        edges.insert("a".into(), vec!["b".into()]);
        edges.insert("b".into(), vec!["a".into()]);
        let reachable: BTreeSet<String> = ["x", "y", "z", "a", "b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let result = toposort_reachable(&edges, &reachable);
        match result.unwrap_err() {
            EngineError::CycleDetected(msg) => {
                assert!(msg.contains("\"a\""), "missing cycle node 'a': {msg}");
                assert!(msg.contains("\"b\""), "missing cycle node 'b': {msg}");
                // The resolvable nodes must NOT appear in the cycle list.
                assert!(
                    !msg.contains("\"x\""),
                    "resolvable node 'x' should not be in cycle error: {msg}"
                );
                assert!(
                    !msg.contains("\"y\""),
                    "resolvable node 'y' should not be in cycle error: {msg}"
                );
                assert!(
                    !msg.contains("\"z\""),
                    "resolvable node 'z' should not be in cycle error: {msg}"
                );
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
    }
}
