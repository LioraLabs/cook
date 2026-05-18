//! Unified engine entry point for both single-Cookfile and workspace builds.
//!
//! `run()` takes the full recipe graph, resolves dependencies, and executes
//! recipes by building a single unified work-unit DAG and walking it with the
//! shared executor pool. Cross-recipe edges live directly on the unified DAG;
//! there is no per-wave register / DAG / execute loop (SHI-222 Phase 4).

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

use crate::analyzer::{self, GraphError};
use crate::{
    dag_builder, executor, EngineError, EngineEvent, RecipeKind, RegisteredWorkspace,
    RegistryEntry,
};

// ---------------------------------------------------------------------------
// TestScope — how to scope a `cook test` invocation
// ---------------------------------------------------------------------------

/// How to scope a `cook test` invocation.
#[derive(Debug, Clone)]
pub enum TestScope {
    /// `cook test <recipe>` — scope to a single recipe and its dep closure.
    Recipe(String),
    /// `cook test <namespace>` — scope to an import alias's tree.
    Namespace(String),
}

// ---------------------------------------------------------------------------
// run_for_test — test-mode engine entry point
// ---------------------------------------------------------------------------

/// Test-mode engine entry point.
///
/// 5-phase pipeline:
///   1. Discover: workspace-wide recipe registration (or target-driven if scope is Some).
///   2. Filter: filter_patterns + rerun_failed_set.
///   3. Reverse-closure: build the cook-unit slice from test units.
///   4. (Phase 5) fingerprint + cache lookup — stub here, every test runs.
///   5. Execute & report — emit test events + populate RunResult.
pub fn run_for_test(
    project_root: &Path,
    scope: Option<TestScope>,
    filter_patterns: &[String],
    rerun_failed_set: Option<&BTreeSet<crate::TestId>>,
    rerun_patterns: &[String],
    fail_fast: bool,
    num_jobs: usize,
    on_event: impl Fn(EngineEvent) + Send + Sync,
) -> Result<RunResult, EngineError> {
    let started = std::time::Instant::now();
    let result = run_for_test_inner(
        project_root,
        scope,
        filter_patterns,
        rerun_failed_set,
        rerun_patterns,
        fail_fast,
        num_jobs,
        &on_event,
    );
    on_event(EngineEvent::Finished {
        elapsed: started.elapsed(),
        success: result.is_ok(),
    });
    result
}

fn run_for_test_inner<F>(
    project_root: &Path,
    scope: Option<TestScope>,
    filter_patterns: &[String],
    rerun_failed_set: Option<&BTreeSet<crate::TestId>>,
    rerun_patterns: &[String],
    _fail_fast: bool,
    num_jobs: usize,
    on_event: &F,
) -> Result<RunResult, EngineError>
where
    F: Fn(EngineEvent) + Send + Sync,
{
    // rerun_patterns is plumbed to execute_dag below; it gates per-test cache
    // lookup so matching tests force-rerun even if a cache entry exists.

    // ── Phase 1: Discover ────────────────────────────────────────────────────
    // Load the workspace so we have both the recipe_infos and the registries.
    let cookfile_path = project_root.join("Cookfile");
    let workspace = crate::pipeline::workspace::Workspace::load(
        &cookfile_path,
        project_root,
        &[],
    )
    .map_err(|e| EngineError::RegistrationFailed {
        recipe: "<workspace>".to_string(),
        message: e.to_string(),
    })?;

    // Build recipe_infos from the workspace.
    let recipe_infos: BTreeMap<String, analyzer::RecipeInfo> = {
        use crate::pipeline::recipe_info::build_workspace_recipe_info;
        if workspace.imports.is_empty() {
            crate::pipeline::recipe_info::build_single_recipe_infos(&workspace.root.cookfile)
        } else {
            build_workspace_recipe_info(&workspace)
        }
    };

    // Build registries from the workspace.
    let registries: BTreeMap<String, RegistryEntry> =
        crate::pipeline::registries::build_workspace_registries(&workspace, None, &[])
            .map_err(|e| EngineError::RegistrationFailed {
                recipe: "<workspace>".to_string(),
                message: e.to_string(),
            })?;

    // Build the set of chore names so they can be excluded from test candidates.
    // Chores are excluded from `cook test` because they are destructive by
    // design (e.g., `cook clean` deletes build artefacts and caches) and have
    // no test steps. Including them would cause unintentional side-effects.
    let chore_names: BTreeSet<String> = {
        let mut names = BTreeSet::new();
        for chore in &workspace.root.cookfile.chores {
            names.insert(chore.name.clone());
        }
        for (_, loaded) in &workspace.imports {
            for chore in &loaded.cookfile.chores {
                names.insert(chore.name.clone());
            }
        }
        names
    };

    // Determine the set of candidate recipe names to build based on scope.
    let candidate_recipe_names: Vec<String> = match &scope {
        None => recipe_infos.keys().filter(|n| !chore_names.contains(*n)).cloned().collect(),
        Some(TestScope::Recipe(name)) => {
            // Include just this recipe and its transitive deps (chores excluded).
            analyzer::dependency_edges(&recipe_infos, name)
                .map_err(|e| match e {
                    GraphError::CycleDetected(s) => EngineError::CycleDetected(s),
                    GraphError::UnknownRecipe(s) => EngineError::UnknownRecipe(s),
                    e => EngineError::CycleDetected(e.to_string()),
                })?
                .keys()
                .filter(|n| !chore_names.contains(*n))
                .cloned()
                .collect()
        }
        Some(TestScope::Namespace(ns)) => {
            // Include all recipes whose name starts with "<ns>." (chores excluded).
            let prefix = format!("{ns}.");
            recipe_infos
                .keys()
                .filter(|n| !chore_names.contains(*n))
                .filter(|n| n.starts_with(&prefix) || *n == ns)
                .cloned()
                .collect()
        }
    };

    // ── Phase 2: Filter ──────────────────────────────────────────────────────
    // Pre-filter: when filter_patterns are present, limit the target recipe set
    // to those whose recipe name could plausibly match the glob (recipe-level
    // matching). The glob pattern uses `<recipe>:<test_name>` format; we match
    // the recipe portion by checking if any pattern matches `<recipe>:*`.
    // This avoids running unrelated recipes whose build steps may fail hard
    // (e.g., blocked_by_build running `false`) when we only care about specific tests.
    //
    // Post-execution, we still apply the full per-TestId filter to handle the
    // test_name portion of the glob.
    let candidate_recipe_names: Vec<String> = if !filter_patterns.is_empty() {
        candidate_recipe_names
            .into_iter()
            .filter(|recipe_name| {
                filter_patterns.iter().any(|pat| {
                    // Strip the `:test_name` suffix from the pattern to get the
                    // recipe-level glob (e.g., "pass_basic:*" → "pass_basic").
                    let recipe_pat = if let Some(colon_pos) = pat.find(':') {
                        pat[..colon_pos].to_string()
                    } else {
                        // Pattern has no colon — treat as recipe-level glob.
                        pat.clone()
                    };
                    // Match full `<recipe>:` against the full pattern.
                    let wildcard_id_for_recipe = format!("{}:", recipe_name);
                    let full_match = globset::Glob::new(pat)
                        .map(|g| g.compile_matcher().is_match(&wildcard_id_for_recipe))
                        .unwrap_or(false);
                    // Also match just the recipe name against the recipe portion of pattern.
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

    // Build dependency edges for the candidate set.
    let targets: Vec<String> = candidate_recipe_names;
    if targets.is_empty() {
        return Ok(RunResult { test_results: vec![] });
    }

    // ── Phase 3-5: Execute ───────────────────────────────────────────────────
    // In test mode, a cook-step failure should not short-circuit the run with
    // EngineError::TaskFailures. Instead, execute_dag will have already pushed
    // Blocked TestResult rows via cancel_subtree for every downstream test node.
    // Those rows are carried in TaskFailures.partial_test_results so we can
    // return Ok with the Blocked results rather than propagating the error.
    let raw_result = run_with_registries_inner(
        project_root,
        &recipe_infos,
        &targets,
        &registries,
        num_jobs,
        on_event,
        rerun_patterns,
    );
    let result = match raw_result {
        Ok(r) => r,
        Err(EngineError::TaskFailures { partial_test_results, .. }) => {
            // Cook steps failed but downstream test nodes were captured as
            // Blocked. Treat as a successful engine run — the reporter will
            // detect Blocked rows and exit non-zero via its existing logic.
            RunResult { test_results: partial_test_results }
        }
        Err(other) => return Err(other),
    };

    // Post-execution: filter test_results by filter_patterns and rerun_failed_set.
    let test_results = result
        .test_results
        .into_iter()
        .filter(|r| {
            let id_matches = if filter_patterns.is_empty() {
                true
            } else {
                matches_any(&r.id, filter_patterns)
            };
            let rerun_matches = if let Some(failed_set) = rerun_failed_set {
                failed_set.contains(&r.id)
            } else {
                true
            };
            id_matches && rerun_matches
        })
        .collect();

    Ok(RunResult { test_results })
}

/// Returns true if `id` matches any of the glob `patterns`.
fn matches_any(id: &crate::TestId, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| {
        match globset::Glob::new(pat) {
            Ok(g) => g.compile_matcher().is_match(&id.0),
            Err(_) => false,
        }
    })
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
/// Resolves the dependency graph for all `targets`, then executes the build
/// by constructing a single work-unit DAG across every reachable recipe and
/// walking it with the shared executor pool.
///
/// # Arguments
///
/// * `project_root` - The project root directory. Used to probe execution context,
///   load `.cook/cloud.toml`, and compute cookfile-relative paths.
/// * `recipe_infos` - All known recipes and their dependency metadata.
/// * `targets` - The target recipes to build.
/// * `registries` - One `RegistryEntry` per namespace prefix. Root recipes use `""` as the key.
/// * `num_jobs` - Maximum number of parallel worker threads.
/// * `inferred_deps` - `{dep}` references: recipe -> recipes it references.
///   Retained in the signature for compatibility with the wave-grouper-era
///   call sites; the unified-DAG path no longer treats them as wave-merging
///   hints (cross-recipe edges live directly on the work-unit DAG).
/// * `on_event` - Callback invoked for each engine event (progress, errors, etc.).
///
/// **SHI-222 transitional shape.** Until Phase 5 ports `cook-cli` to drive
/// registration with `register_workspace`, this entry point continues to
/// accept the per-prefix `RegistryEntry` map and internally aggregates it
/// into a [`RegisteredWorkspace`] before dispatching to [`run_inner`]. The
/// aggregation walks every reachable recipe and invokes the legacy
/// `RegisterSessionBuilder::register_recipe` for each; Phase 5 will replace
/// the aggregation helper with the proper `register_cookfile`-driven path.
pub fn run(
    project_root: &Path,
    recipe_infos: &BTreeMap<String, analyzer::RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, RegistryEntry>,
    num_jobs: usize,
    _inferred_deps: &BTreeMap<String, Vec<String>>,
    on_event: impl Fn(EngineEvent) + Send + Sync,
) -> Result<RunResult, EngineError> {
    let started = std::time::Instant::now();
    let result = run_with_registries_inner(
        project_root,
        recipe_infos,
        targets,
        registries,
        num_jobs,
        &on_event,
        &[],
    );
    on_event(EngineEvent::Finished {
        elapsed: started.elapsed(),
        success: result.is_ok(),
    });
    result
}

/// Transitional inner driver: aggregates the legacy per-prefix `RegistryEntry`
/// map into a [`RegisteredWorkspace`] via [`register_workspace_from_registries`],
/// then dispatches to [`run_inner`] which executes against the unified DAG.
///
/// Phase 5 will retire this shim in favour of a CLI that hands a fully built
/// [`RegisteredWorkspace`] directly to [`run_inner`].
fn run_with_registries_inner<F>(
    project_root: &Path,
    recipe_infos: &BTreeMap<String, analyzer::RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, RegistryEntry>,
    num_jobs: usize,
    on_event: &F,
    rerun_patterns: &[String],
) -> Result<RunResult, EngineError>
where
    F: Fn(EngineEvent) + Send + Sync,
{
    // Build CacheContext once for the whole build invocation.
    let cache_ctx = build_cache_ctx(project_root)?;

    // Resolve the reachable recipe set BEFORE registration so the legacy
    // aggregation only registers what's needed for this target invocation.
    let edges = analyzer::dependency_edges_multi(recipe_infos, targets).map_err(|e| match e {
        GraphError::CycleDetected(s) => EngineError::CycleDetected(s),
        GraphError::UnknownRecipe(s) => EngineError::UnknownRecipe(s),
        // Io/Parse cannot be produced by dependency_edges_multi (pure graph op).
        e => EngineError::CycleDetected(e.to_string()),
    })?;
    let reachable: BTreeSet<String> = edges.keys().cloned().collect();

    // Aggregate registries → RegisteredWorkspace by invoking the legacy
    // per-recipe registration API for every reachable name.
    let registered_workspace = register_workspace_from_registries(
        registries,
        &reachable,
        Some(cache_ctx.clone()),
    )?;

    run_inner(
        &registered_workspace,
        &edges,
        &reachable,
        num_jobs,
        cache_ctx,
        on_event,
        rerun_patterns,
    )
}

/// Walk the unified work-unit DAG across every reachable recipe.
///
/// `edges` is the recipe-level dependency edge map (from
/// [`analyzer::dependency_edges_multi`]) for the reachable target closure.
/// `reachable` is the set of all reachable recipe names — must equal
/// `edges.keys().cloned().collect()`.
///
/// **Unified-DAG contract (SHI-222 Phase 4).** This function builds the
/// work-unit DAG in a single [`dag_builder::build_dag`] invocation across
/// every reachable recipe, then dispatches it to a single
/// [`executor::execute_dag`] walk. Cross-recipe edges (coarse `deps` and
/// fine-grained `dep_edges`) are wired directly on the work-unit DAG; there
/// is no per-wave loop and no per-wave registration.
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
    //    RecipeCompleted) fire from the executor's recipe-tracker bookkeeping.
    //    Task 4.5 will refine that to unit-driven transitions; for Phase 4
    //    the per-recipe semantics already match the wave-loop behaviour.
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

/// Phase 4 transitional shim: aggregate per-prefix [`RegistryEntry`] map into
/// a [`RegisteredWorkspace`] by invoking the legacy
/// `RegisterSessionBuilder::register_recipe` for every reachable recipe name.
///
/// Each reachable recipe is split into `(prefix, local_name)`; the registry
/// for that prefix is used to register the local name, producing a
/// `RecipeUnits` whose `recipe_name` is rewritten to the fully-qualified
/// name. The aggregation also captures the per-prefix `working_dir` so the
/// executor can construct per-recipe cache managers.
///
/// Probes are aggregated from each `RecipeUnits.probes` into the
/// workspace-level `probes` map keyed by probe key; collisions across
/// recipes are resolved first-wins. Per CS-0074 probe keys are global, so
/// collisions shouldn't occur in well-formed input; Phase 5 will source
/// probes directly from `RegisteredCookfile.probes` instead, where conflict
/// detection happens at registration time.
///
/// Phase 5 Task 5.1 replaces this helper with a proper
/// `register_cookfile`-driven workspace registration that captures probes
/// and `final_env` at the Cookfile granularity.
fn register_workspace_from_registries(
    registries: &BTreeMap<String, RegistryEntry>,
    reachable: &BTreeSet<String>,
    cache_ctx: Option<Arc<CacheContext>>,
) -> Result<RegisteredWorkspace, EngineError> {
    use cook_contracts::ProbeUnit;
    use cook_register::{RecipeKind as RegKind, RegisteredRecipePub, RegistrationSource};

    let mut units_by_recipe: BTreeMap<String, RecipeUnits> = BTreeMap::new();
    let mut working_dir_by_prefix: BTreeMap<String, std::path::PathBuf> = BTreeMap::new();
    let mut alias_dirs_by_prefix: BTreeMap<String, BTreeMap<String, std::path::PathBuf>> =
        BTreeMap::new();
    let mut names: Vec<RegisteredRecipePub> = Vec::new();
    let mut probes: BTreeMap<String, ProbeUnit> = BTreeMap::new();

    // Record working_dir for every present prefix once, before per-recipe work,
    // so the engine can still build cache managers for namespaces whose
    // recipes are all zero-unit or absent from `reachable`.
    for (prefix, entry) in registries {
        working_dir_by_prefix
            .entry(prefix.clone())
            .or_insert_with(|| entry.registry.working_dir().clone());
        alias_dirs_by_prefix
            .entry(prefix.clone())
            .or_insert_with(|| entry.alias_dirs.clone());
    }

    for name in reachable {
        let (prefix, local_name) = split_recipe_name(name);
        let entry = registries
            .get(&prefix)
            .ok_or_else(|| EngineError::RegistrationFailed {
                recipe: name.clone(),
                message: format!("no registry for prefix '{prefix}'"),
            })?;
        let registry = &entry.registry;
        let lua_source = &entry.lua_source;

        let mut units = registry
            .register_recipe(lua_source, &local_name, cache_ctx.clone())
            .map_err(|e| EngineError::RegistrationFailed {
                recipe: name.clone(),
                message: e.to_string(),
            })?;

        // Rewrite the recipe name to the fully-qualified workspace form so
        // the DAG builder and executor see consistent identifiers across
        // imports.
        units.recipe_name = name.clone();

        // Aggregate this recipe's probes into the workspace-level map. This
        // is what `run_inner` consults to build `probe_units_by_node` for
        // the executor's probe-value cache fast path (CS-0074). Without this
        // step every probe re-executes on every run because the executor
        // can't look up the unit by key.
        for pu in &units.probes {
            probes.entry(pu.key.clone()).or_insert_with(|| pu.clone());
        }

        units_by_recipe.insert(name.clone(), units);

        // Surface a `RegisteredRecipePub` stub so callers (and synthetic
        // lifecycle-event emission) can branch on `kind`. The legacy
        // per-recipe `register_recipe` API does not surface kind/source
        // metadata, so we default to `Recipe` here; Phase 5 will replace
        // this with the metadata captured by `register_cookfile`.
        names.push(RegisteredRecipePub {
            name: name.clone(),
            source: RegistrationSource::Static { line: 0 },
            kind: RegKind::Recipe,
            requires: vec![],
        });
    }

    Ok(RegisteredWorkspace {
        names,
        units_by_recipe,
        probes,
        final_env_by_cookfile: BTreeMap::new(),
        working_dir_by_prefix,
        alias_dirs_by_prefix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::RecipeInfo;

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

    #[test]
    fn test_run_unknown_target() {
        let recipes = BTreeMap::new();
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();
        let result = run(
            &dummy_project_root(),
            &recipes,
            &["missing".to_string()],
            &registries,
            1,
            &inferred,
            |_| {},
        );
        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::UnknownRecipe(name) => assert_eq!(name, "missing"),
            other => panic!("expected UnknownRecipe, got: {other:?}"),
        }
    }

    #[test]
    fn test_run_empty_targets() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: vec![],
            },
        );
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();
        // Empty targets means empty reachable set, so run_inner returns
        // immediately with no test results and no DAG to execute.
        let result = run(&dummy_project_root(), &recipes, &[], &registries, 1, &inferred, |_| {});
        assert!(result.is_ok());
        assert!(result.unwrap().test_results.is_empty());
    }

    #[test]
    fn test_run_cycle_detected() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "a".to_string(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: vec!["b".to_string()],
            },
        );
        recipes.insert(
            "b".to_string(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: vec!["a".to_string()],
            },
        );
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();
        let result = run(&dummy_project_root(), &recipes, &["a".to_string()], &registries, 1, &inferred, |_| {});
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EngineError::CycleDetected(_)
        ));
    }

    #[test]
    fn test_run_emits_finished_success_on_empty_targets() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: vec![],
            },
        );
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();

        let events = std::sync::Mutex::new(Vec::new());
        let result = run(&dummy_project_root(), &recipes, &[], &registries, 1, &inferred, |event| {
            events.lock().unwrap().push(event)
        });
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
    fn test_run_emits_finished_failure_on_unknown_target() {
        let recipes = BTreeMap::new();
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();

        let events = std::sync::Mutex::new(Vec::new());
        let result = run(
            &dummy_project_root(),
            &recipes,
            &["missing".to_string()],
            &registries,
            1,
            &inferred,
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
    fn test_run_accepts_inferred_deps() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: vec![],
            },
        );
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();
        // This will fail because no registry for "" prefix, but it shouldn't panic
        let result = run(&dummy_project_root(), &recipes, &["build".to_string()], &registries, 1, &inferred, |_| {});
        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::RegistrationFailed { recipe, .. } => assert_eq!(recipe, "build"),
            other => panic!("expected RegistrationFailed, got: {other:?}"),
        }
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
