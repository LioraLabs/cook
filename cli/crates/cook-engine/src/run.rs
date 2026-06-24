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
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;

use cook_cache::{
    backend::LocalBackend, cache_ctx::CacheContext, cloud_backend::CloudBackend,
    cloud_config::CloudConfig, TestCache, ThreadSafeCacheManager,
};
use cook_contracts::{RecipeUnits, WorkPayload};
use cook_fingerprint::{CacheBackend, EnvDenylist, FingerprintInputs};

use crate::{
    dag_builder, executor, EngineError, EngineEvent, RecipeKind, RegisteredWorkspace, WorkNode,
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
    /// Stale-output reconciliation summary (§17.7). Empty under `--no-prune`
    /// or when nothing was orphaned. `swept` are the orphaned outputs Cook
    /// removed; `kept_modified` are orphans kept because they changed since
    /// Cook wrote them. Surfaced here (rather than printed by the engine) so
    /// the CLI can report them after the progress renderer has finished.
    pub swept: Vec<std::path::PathBuf>,
    pub kept_modified: Vec<std::path::PathBuf>,
}

/// Split a namespaced recipe name into (prefix, local_name).
///
/// `"backend.proto.generate"` -> `("backend.proto", "generate")`
/// `"build"` -> `("", "build")`
pub(crate) fn split_recipe_name(name: &str) -> (String, String) {
    if let Some(dot_pos) = name.rfind('.') {
        (name[..dot_pos].to_string(), name[dot_pos + 1..].to_string())
    } else {
        (String::new(), name.to_string())
    }
}

// ---------------------------------------------------------------------------
// Test-fingerprint file inputs — COOK-84 transitive-closure hashing
// ---------------------------------------------------------------------------

/// Lexically normalize a path: drop `.` components and resolve `..` by
/// popping the previous component. No filesystem access — purely textual,
/// so declared paths compare equal regardless of how `working_dir.join`
/// composed them.
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Project-root-relative, forward-slashed pair key (§17.4.4). Falls back to
/// the absolute path string when `abs` is outside the project root.
fn root_rel_key(project_root: &Path, abs: &Path) -> String {
    let rel = abs.strip_prefix(project_root).unwrap_or(abs);
    rel.to_string_lossy().replace('\\', "/")
}

/// Content-hash `abs`, memoized across all test nodes in a run. Missing
/// (or unreadable) files hash to the literal `"missing"` so an absent file
/// still contributes deterministically to the fingerprint.
fn memo_hash(memo: &mut BTreeMap<PathBuf, String>, abs: &Path) -> String {
    if let Some(h) = memo.get(abs) {
        return h.clone();
    }
    let h = cook_fingerprint::hash_file(abs)
        .map(|h| format!("{h:x}"))
        .unwrap_or_else(|| "missing".to_string());
    memo.insert(abs.to_path_buf(), h.clone());
    h
}

/// Collect the file-system contributions to a test node's upfront
/// fingerprint (COOK-84, §17.5.3).
///
/// Returns `(cook_outputs, dep_outputs)` for [`FingerprintInputs`]:
///
/// * **`.0` — OWN inputs**: the Test payload's `input_paths` resolved
///   against the test node's working dir, EXCLUDING any path declared as
///   an output by a node in the test's predecessor closure. Those
///   artifacts are stale at upfront-fingerprint time; their *sources* are
///   hashed via the closure contribution instead. The exclusion prevents
///   once-per-edit fingerprint oscillation as artifacts catch up.
/// * **`.1` — CLOSURE contribution**: for every predecessor node with
///   `cache_meta`, (a) each declared `input_paths` file resolved against
///   THAT node's working dir and content-hashed — also EXCLUDING any path
///   declared as an output by a node in the closure (i.e. intermediate
///   artifacts, e.g. `build/lib.txt` appearing as an input to an `app`
///   node in a lib → app → test chain). This exclusion is required for the
///   same reason as the own-input exclusion: in a ≥2-level chain those
///   intermediate artifacts are stale at upfront time and would introduce
///   once-per-edit fingerprint oscillation. Their *sources* are already
///   captured by the producing unit's own closure contribution. (b) one
///   identity pair `("unit:{cookfile_path}:{recipe_name}:{cache_key}",
///   "{command_hash:x}:{env_contribution:x}")` so editing a dep's command
///   busts downstream tests.
///
/// Pair keys are project-root-relative and forward-slashed (§17.4.4);
/// missing files hash to `"missing"`; file hashes are memoized in
/// `hash_memo` across all test nodes per run.
fn collect_test_file_inputs(
    dag: &cook_dag::Dag<WorkNode>,
    test_idx: usize,
    project_root: &Path,
    hash_memo: &mut BTreeMap<PathBuf, String>,
) -> (Vec<(String, String)>, Vec<(String, String)>) {
    use std::collections::VecDeque;

    // (1) BFS predecessor closure (excludes the test node itself).
    let mut closure: BTreeSet<usize> = BTreeSet::new();
    let mut queue: VecDeque<usize> = dag.deps(test_idx).iter().copied().collect();
    while let Some(idx) = queue.pop_front() {
        if closure.insert(idx) {
            queue.extend(dag.deps(idx).iter().copied());
        }
    }

    // (2) Outputs declared by the closure: literal paths as a normalized
    //     set, glob patterns as (working_dir, matcher) pairs.
    let mut literal_outputs: BTreeSet<PathBuf> = BTreeSet::new();
    let mut glob_outputs: Vec<(PathBuf, globset::GlobMatcher)> = Vec::new();
    for &idx in &closure {
        let node = dag.node(idx).payload();
        let Some(meta) = &node.cache_meta else {
            continue;
        };
        for out in &meta.output_paths {
            if cook_fingerprint::has_glob_meta(out) {
                if let Ok(g) = globset::Glob::new(out) {
                    glob_outputs.push((node.working_dir.clone(), g.compile_matcher()));
                }
            } else {
                literal_outputs.insert(lexical_normalize(&node.working_dir.join(out)));
            }
        }
    }
    let produced_upstream = |abs: &Path| -> bool {
        if literal_outputs.contains(abs) {
            return true;
        }
        glob_outputs.iter().any(|(wd, matcher)| {
            abs.strip_prefix(wd)
                .map(|rel| matcher.is_match(rel))
                .unwrap_or(false)
        })
    };

    // (3) OWN inputs: the Test payload's input_paths, resolved against the
    //     test node's working dir, minus predecessor-produced artifacts.
    let test_node = dag.node(test_idx).payload();
    let mut own: BTreeMap<String, String> = BTreeMap::new();
    if let Some(WorkPayload::Test { input_paths, .. }) = &test_node.payload {
        for input in input_paths {
            let resolved: Vec<PathBuf> = if cook_fingerprint::has_glob_meta(input) {
                cook_fingerprint::resolve_glob(&test_node.working_dir, input)
                    .into_iter()
                    .map(|rel| test_node.working_dir.join(rel))
                    .collect()
            } else {
                vec![test_node.working_dir.join(input)]
            };
            for abs in resolved {
                let abs = lexical_normalize(&abs);
                if produced_upstream(&abs) {
                    continue;
                }
                let hash = memo_hash(hash_memo, &abs);
                own.insert(root_rel_key(project_root, &abs), hash);
            }
        }
    }

    // (4) CLOSURE contribution: dep identity pairs + dep source hashes.
    let mut dep: BTreeMap<String, String> = BTreeMap::new();
    for &idx in &closure {
        let node = dag.node(idx).payload();
        let Some(meta) = &node.cache_meta else {
            continue;
        };
        dep.insert(
            format!(
                "unit:{}:{}:{}",
                meta.cookfile_path, meta.recipe_name, meta.cache_key
            ),
            format!("{:x}:{:x}", meta.command_hash, meta.env_contribution),
        );
        for input in &meta.input_paths {
            let resolved: Vec<PathBuf> = if cook_fingerprint::has_glob_meta(input) {
                cook_fingerprint::resolve_glob(&node.working_dir, input)
                    .into_iter()
                    .map(|rel| node.working_dir.join(rel))
                    .collect()
            } else {
                vec![node.working_dir.join(input)]
            };
            for abs in resolved {
                let abs = lexical_normalize(&abs);
                // Skip intermediate artifacts produced by another node in the
                // closure — they are stale at upfront-fingerprint time and
                // would cause once-per-edit fingerprint oscillation in ≥2-level
                // chains (e.g. lib → app → test where app's input_paths
                // includes build/lib.txt). Their sources are already captured
                // by the producing unit's own closure contribution entry.
                if produced_upstream(&abs) {
                    continue;
                }
                let hash = memo_hash(hash_memo, &abs);
                dep.insert(root_rel_key(project_root, &abs), hash);
            }
        }
    }

    (own.into_iter().collect(), dep.into_iter().collect())
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
/// * `no_prune` - When true, disables stale-output reconciliation (§17.7) for
///   this invocation (`--no-prune` / `COOK_NO_PRUNE`). Orphaned outputs are
///   retained instead of swept.
/// * `no_publish` - When true, suppresses ALL shared-store uploads for this
///   invocation (`--no-publish` / `COOK_NO_PUBLISH`). Fetch, drift-restore,
///   and `pinned` cold-fetch are unaffected. Combined with `[cloud] publish`
///   in `build_cache_ctx` to compute `CacheContext::publish_enabled`.
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
    no_prune: bool,
    no_publish: bool,
    on_event: F,
) -> Result<RunResult, EngineError>
where
    F: Fn(EngineEvent) + Send + Sync,
{
    let started = std::time::Instant::now();
    let cache_ctx = match build_cache_ctx(project_root, no_publish) {
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
        no_prune,
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
    no_prune: bool,
) -> Result<RunResult, EngineError>
where
    F: Fn(EngineEvent) + Send + Sync,
{
    // 0. §22.1.2 terminal-output rule: workspace-wide structural check.
    //    Collect ALL registered recipe units (not just the reachable closure)
    //    and verify that no recipe's literal inputs[] path is matched by
    //    another recipe's glob outputs[] pattern. This runs before any DAG
    //    construction or execution so the error surfaces at register time.
    {
        let all_workspace_units: Vec<RecipeUnits> = registered_workspace
            .units_by_recipe
            .values()
            .cloned()
            .collect();
        dag_builder::check_globbed_output_cross_recipe_edges(&all_workspace_units)?;
    }

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
        return Ok(RunResult {
            test_results: vec![],
            swept: vec![],
            kept_modified: vec![],
        });
    }

    // 5. Per-recipe cache managers. One per reachable recipe, anchored at
    //    that recipe's prefix's working_dir. Shared with `cook why` via
    //    `cache_managers_for_cli` so the two paths can never drift.
    let cache_managers = cache_managers_for_cli(registered_workspace, reachable);

    // §17.7 stale-output reconciliation: snapshot each reached recipe's prior
    // recorded outputs (absolute path → recorded content hash) BEFORE the run
    // overwrites the on-disk cache. Skipped entirely under --no-prune.
    let prior_outputs_by_recipe: BTreeMap<String, BTreeMap<PathBuf, u64>> = if no_prune {
        BTreeMap::new()
    } else {
        let mut map = BTreeMap::new();
        for name in reachable {
            let (Some(ru), Some(cm)) = (
                registered_workspace.units_by_recipe.get(name),
                cache_managers.get(name),
            ) else {
                continue;
            };
            let index_name = recipe_cache_index_name(ru, name);
            let prior = cm.get_or_load(&index_name);
            let mut outs: BTreeMap<PathBuf, u64> = BTreeMap::new();
            for step in prior.steps.values() {
                for o in &step.outputs {
                    outs.insert(ru.working_dir.join(&o.path), o.hash);
                }
            }
            if !outs.is_empty() {
                map.insert(name.clone(), outs);
            }
        }
        map
    };

    // Share the per-recipe cache managers with the post-run reconciliation
    // pass: execute_dag takes ownership below, but the Arcs alias the same
    // managers, so the in-memory caches it updates are visible here afterwards.
    let recon_managers = cache_managers.clone();

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
        let mut hash_memo: BTreeMap<PathBuf, String> = BTreeMap::new();
        for node_idx in 0..dag.len() {
            let work_node = dag.node(node_idx).payload();
            if let Some(WorkPayload::Test { .. }) = &work_node.payload {
                let env_keys: Vec<(String, String)> = work_node
                    .env_vars
                    .iter()
                    .filter(|(k, _)| !cache_ctx.denylist.is_ignored(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                // COOK-84: hash the test's transitive source closure —
                // own inputs (minus predecessor-produced artifacts, which
                // are stale at upfront time) plus every closure dep's
                // declared source inputs and unit identity. Editing a dep's
                // source or command busts the test's fingerprint without
                // waiting for artifacts to be rebuilt.
                let (cook_outputs, dep_outputs) = collect_test_file_inputs(
                    &dag,
                    node_idx,
                    &cache_ctx.project_root,
                    &mut hash_memo,
                );
                let inputs = FingerprintInputs {
                    cook_outputs,
                    dep_outputs,
                    env_keys,
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

    // §17.7 stale-output reconciliation: outputs are now materialised, so
    // compute the cross-recipe live output set and sweep orphaned files.
    let (swept, kept_modified) = if no_prune {
        (vec![], vec![])
    } else {
        reconcile_outputs(
            registered_workspace,
            reachable,
            &recon_managers,
            &prior_outputs_by_recipe,
        )
    };

    Ok(RunResult {
        test_results,
        swept,
        kept_modified,
    })
}

/// The on-disk index name a recipe's cache is stored under (`.cook/cache/<name>.toml`): the `recipe_name`
/// its captured units carry in their [`cook_contracts::CacheMeta`] (which the
/// executor uses as the manager's per-recipe key), falling back to the
/// recipe's own name for unit-less meta-targets.
pub(crate) fn recipe_cache_index_name(ru: &RecipeUnits, fallback: &str) -> String {
    ru.units
        .iter()
        .find_map(|u| u.cache_meta.as_ref().map(|m| m.recipe_name.clone()))
        .unwrap_or_else(|| fallback.to_string())
}

/// Sweep stale outputs for every reached recipe (§17.7).
///
/// Builds the cross-recipe *live* output set (every output declared by any
/// reached recipe this run, glob-resolved post-execution), then for each
/// recipe diffs its prior recorded outputs against that set and sweeps the
/// orphans via [`crate::reconcile::sweep`] (hash-guarded). Finally advances
/// each recipe's recorded set by pruning steps whose every output is gone.
///
/// Returns `(swept, kept_modified)` aggregated across all reached recipes so
/// the caller can report them after the progress renderer has finished.
fn reconcile_outputs(
    registered_workspace: &RegisteredWorkspace,
    reachable: &BTreeSet<String>,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    prior_outputs_by_recipe: &BTreeMap<String, BTreeMap<PathBuf, u64>>,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut all_swept: Vec<PathBuf> = Vec::new();
    let mut all_kept_modified: Vec<PathBuf> = Vec::new();
    // Current cross-recipe live set.
    let mut live: BTreeSet<PathBuf> = BTreeSet::new();
    for name in reachable {
        if let Some(ru) = registered_workspace.units_by_recipe.get(name) {
            for u in &ru.units {
                if let Some(m) = &u.cache_meta {
                    for rel in
                        crate::executor::resolve_output_paths(&m.output_paths, &ru.working_dir)
                    {
                        live.insert(ru.working_dir.join(rel));
                    }
                    // A discovered-inputs depfile is recorded as an implicit
                    // cache output (so a restore can pull it back) but is NOT a
                    // declared `output_path`. It is still a live file Cook means
                    // to keep — count it so §17.7 never sweeps it (COOK-75).
                    if let Some(di) = &m.discovered_inputs {
                        live.insert(ru.working_dir.join(&di.from));
                    }
                }
            }
        }
    }

    for name in reachable {
        let Some(prior) = prior_outputs_by_recipe.get(name) else {
            continue;
        };
        let recon = crate::reconcile::sweep(prior, &live);
        for p in recon.swept() {
            tracing::debug!("swept orphaned output: {}", p.display());
            all_swept.push(p.clone());
        }
        for p in recon.kept_modified() {
            tracing::debug!("{} changed since Cook wrote it — not removing", p.display());
            all_kept_modified.push(p.clone());
        }

        // Advance the recorded set: drop steps whose every output is no longer
        // declared so the cache stops claiming swept artifacts.
        if let (Some(cm), Some(ru)) = (
            cache_managers.get(name),
            registered_workspace.units_by_recipe.get(name),
        ) {
            let index_name = recipe_cache_index_name(ru, name);
            let wd = ru.working_dir.clone();
            let live_ref = &live;
            cm.retain_steps(&index_name, move |_k, step| {
                step.outputs.is_empty()
                    || step
                        .outputs
                        .iter()
                        .any(|o| live_ref.contains(&wd.join(&o.path)))
            });
        }
    }

    // Persist any pruned caches (flush_all is a no-op for unchanged recipes).
    for cm in cache_managers.values() {
        let _ = cm.flush_all();
    }

    all_swept.sort();
    all_kept_modified.sort();
    (all_swept, all_kept_modified)
}

/// Topologically sort `reachable` against the recipe-level `edges` map.
///
/// Returns recipe names in dependency-first order so that
/// [`dag_builder::build_dag`]'s intra-call `recipe_leaves` accumulator has
/// every dep present before the dependent recipe is processed. Recipes
/// missing from `edges` are placed first (no deps known).
pub(crate) fn toposort_reachable_pub(
    edges: &BTreeMap<String, Vec<String>>,
    reachable: &BTreeSet<String>,
) -> Result<Vec<String>, EngineError> {
    toposort_reachable(edges, reachable)
}

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
/// CLI helper: build a `CacheContext` for read-only introspection (cook why).
pub fn build_cache_ctx_for_cli(
    project_root: &Path,
    no_publish: bool,
) -> Result<Arc<CacheContext>, EngineError> {
    build_cache_ctx(project_root, no_publish)
}

/// CLI helper: per-recipe cache managers, identical to what `run_inner` builds.
pub fn cache_managers_for_cli(
    ws: &RegisteredWorkspace,
    reachable: &BTreeSet<String>,
) -> BTreeMap<String, Arc<ThreadSafeCacheManager>> {
    reachable
        .iter()
        .map(|name| {
            let prefix = split_recipe_name(name).0;
            let wd = ws
                .working_dir_by_prefix
                .get(&prefix)
                .cloned()
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let cache_dir = wd.join(".cook").join("cache");
            (name.clone(), Arc::new(ThreadSafeCacheManager::new(cache_dir)))
        })
        .collect()
}

pub(crate) fn build_cache_ctx(project_root: &Path, no_publish: bool) -> Result<Arc<CacheContext>, EngineError> {
    let cloud_config = CloudConfig::load_or_default(project_root)
        .map_err(|e| EngineError::CacheError(format!("invalid .cook/cloud.toml: {e}")))?;
    let mut denylist = EnvDenylist::baseline();
    denylist.extend_with(cloud_config.cache_ignore_env());
    let denylist = Arc::new(denylist);
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
    // COOK-168: read-only / publish-off client mode. Config opt-out
    // (`[cloud] publish = false`) OR an invocation flag (`--no-publish` /
    // `COOK_NO_PUBLISH`, passed as `no_publish`) suppresses every upload.
    // The flag can only turn publishing OFF, never force it on over a
    // `publish = false` config.
    let publish_enabled = cloud_config.publish() && !no_publish;
    Ok(Arc::new(CacheContext {
        denylist,
        backend,
        cloud_config: Arc::new(cloud_config),
        project_root: project_root.to_path_buf(),
        project_id,
        publish_enabled,
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
            false,
            false,
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
            false,
            false,
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
            false,
            false,
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
            false,
            false,
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

    // -----------------------------------------------------------------------
    // collect_test_file_inputs — COOK-84 transitive-closure fingerprinting
    // -----------------------------------------------------------------------

    fn cook_work_node(
        wd: &std::path::Path,
        recipe: &str,
        inputs: &[&str],
        outputs: &[&str],
        command_hash: u64,
    ) -> crate::WorkNode {
        crate::WorkNode {
            payload: Some(WorkPayload::Shell {
                cmd: "build".into(),
                line: 1,
            }),
            recipe_name: recipe.into(),
            cache_meta: Some(cook_contracts::CacheMeta {
                recipe_name: recipe.into(),
                project_id: String::new(),
                cookfile_path: "Cookfile".into(),
                cache_key: outputs.first().copied().unwrap_or("k").into(),
                input_paths: inputs.iter().map(|s| s.to_string()).collect(),
                output_paths: outputs.iter().map(|s| s.to_string()).collect(),
                command_hash,
                env_contribution: 0,
                consulted_env: Default::default(),
                discovered_inputs: None,
                seal_keys: Default::default(),
            sharing: Default::default(),
                record: false,
            }),
            working_dir: wd.to_path_buf(),
            env_vars: Default::default(),
        }
    }

    fn test_work_node(wd: &std::path::Path, input_paths: &[&str]) -> crate::WorkNode {
        crate::WorkNode {
            payload: Some(WorkPayload::Test {
                cmd: "check".into(),
                line: 1,
                timeout: 5,
                should_fail: false,
                suite_name: "s".into(),
                test_name: "t".into(),
                iteration_item: None,
                input_paths: input_paths.iter().map(|s| s.to_string()).collect(),
            }),
            recipe_name: "s".into(),
            // Binding invariant: Test nodes carry no cache_meta (the executor
            // relies on this — see cook-contracts WorkPayload::Test docs).
            cache_meta: None,
            working_dir: wd.to_path_buf(),
            env_vars: Default::default(),
        }
    }

    /// One cook node (`lib`) producing `build/lib.txt` from `src/lib.txt`,
    /// plus one test node depending on it that consumes its own source
    /// (`src/own.txt`) and the dep's declared artifact (`build/lib.txt`).
    fn closure_fixture(wd: &std::path::Path) -> (cook_dag::Dag<crate::WorkNode>, usize) {
        std::fs::create_dir_all(wd.join("src")).unwrap();
        std::fs::write(wd.join("src/lib.txt"), "ok").unwrap();
        std::fs::write(wd.join("src/own.txt"), "own").unwrap();

        let mut dag = cook_dag::Dag::new();
        let lib = dag
            .add_node(
                cook_work_node(wd, "lib", &["src/lib.txt"], &["build/lib.txt"], 11),
                &[],
            )
            .unwrap();
        let test = dag
            .add_node(
                test_work_node(wd, &["src/own.txt", "build/lib.txt"]),
                &[lib],
            )
            .unwrap();
        (dag, test)
    }

    #[test]
    fn test_fp_inputs_change_when_dep_source_changes() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let (dag, test_idx) = closure_fixture(wd);

        let mut memo = BTreeMap::new();
        let before = collect_test_file_inputs(&dag, test_idx, wd, &mut memo);

        std::fs::write(wd.join("src/lib.txt"), "broken").unwrap();

        let mut memo = BTreeMap::new();
        let after = collect_test_file_inputs(&dag, test_idx, wd, &mut memo);

        assert_ne!(
            before.1, after.1,
            "editing a dep's source file must change the closure contribution"
        );
    }

    #[test]
    fn test_fp_inputs_change_when_own_input_changes() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let (dag, test_idx) = closure_fixture(wd);

        let mut memo = BTreeMap::new();
        let before = collect_test_file_inputs(&dag, test_idx, wd, &mut memo);

        std::fs::write(wd.join("src/own.txt"), "changed").unwrap();

        let mut memo = BTreeMap::new();
        let after = collect_test_file_inputs(&dag, test_idx, wd, &mut memo);

        assert_ne!(
            before.0, after.0,
            "editing the test's own input must change the own-input contribution"
        );
    }

    #[test]
    fn test_fp_excludes_predecessor_produced_artifacts() {
        // The dep's declared output (build/lib.txt) is stale at upfront
        // fingerprint time. It must be EXCLUDED from the test's own inputs
        // (sources are hashed instead) so the fingerprint does not oscillate
        // once-per-edit as the artifact catches up.
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let (dag, test_idx) = closure_fixture(wd);

        let mut memo = BTreeMap::new();
        let before = collect_test_file_inputs(&dag, test_idx, wd, &mut memo);

        std::fs::create_dir_all(wd.join("build")).unwrap();
        std::fs::write(wd.join("build/lib.txt"), "artifact").unwrap();

        let mut memo = BTreeMap::new();
        let after = collect_test_file_inputs(&dag, test_idx, wd, &mut memo);

        assert_eq!(
            before, after,
            "predecessor-produced artifacts must not contribute to the fingerprint"
        );
    }

    #[test]
    fn test_fp_inputs_change_when_dep_command_changes() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        std::fs::create_dir_all(wd.join("src")).unwrap();
        std::fs::write(wd.join("src/lib.txt"), "ok").unwrap();
        std::fs::write(wd.join("src/own.txt"), "own").unwrap();

        let build_dag = |command_hash: u64| {
            let mut dag = cook_dag::Dag::new();
            let lib = dag
                .add_node(
                    cook_work_node(
                        wd,
                        "lib",
                        &["src/lib.txt"],
                        &["build/lib.txt"],
                        command_hash,
                    ),
                    &[],
                )
                .unwrap();
            let test = dag
                .add_node(
                    test_work_node(wd, &["src/own.txt", "build/lib.txt"]),
                    &[lib],
                )
                .unwrap();
            (dag, test)
        };

        let (dag_a, test_a) = build_dag(11);
        let (dag_b, test_b) = build_dag(12);

        let mut memo = BTreeMap::new();
        let a = collect_test_file_inputs(&dag_a, test_a, wd, &mut memo);
        let mut memo = BTreeMap::new();
        let b = collect_test_file_inputs(&dag_b, test_b, wd, &mut memo);

        assert_ne!(
            a.1, b.1,
            "editing a dep's command must change the closure contribution"
        );
    }

    /// Three-node DAG: lib (src/lib.txt → build/lib.txt) ← app
    /// (build/lib.txt → build/app.txt) ← test (build/app.txt).
    ///
    /// Materialising or overwriting the intermediate artifact build/lib.txt
    /// and the final artifact build/app.txt must NOT move the test's
    /// fingerprint, because those paths are declared outputs of nodes in the
    /// closure and must be excluded from hashing (produced_upstream exclusion
    /// now applied to both own inputs AND closure inputs).
    ///
    /// As a sanity-check the test also verifies that editing the original
    /// *source* (src/lib.txt) DOES change the fingerprint.
    #[test]
    fn test_fp_stable_across_two_level_chain_artifact_materialisation() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        std::fs::create_dir_all(wd.join("src")).unwrap();
        std::fs::write(wd.join("src/lib.txt"), "lib-source").unwrap();
        // Artifacts do NOT exist yet at upfront fingerprint time (stale build).

        let build_dag = || {
            let mut dag = cook_dag::Dag::new();
            // lib: src/lib.txt → build/lib.txt
            let lib = dag
                .add_node(
                    cook_work_node(wd, "lib", &["src/lib.txt"], &["build/lib.txt"], 11),
                    &[],
                )
                .unwrap();
            // app: build/lib.txt → build/app.txt (depends on lib)
            let app = dag
                .add_node(
                    cook_work_node(wd, "app", &["build/lib.txt"], &["build/app.txt"], 22),
                    &[lib],
                )
                .unwrap();
            // test: input_paths = ["build/app.txt"] (depends on app)
            let test = dag
                .add_node(test_work_node(wd, &["build/app.txt"]), &[app])
                .unwrap();
            (dag, test)
        };

        // Before: no artifacts on disk.
        let (dag, test_idx) = build_dag();
        let mut memo = BTreeMap::new();
        let before = collect_test_file_inputs(&dag, test_idx, wd, &mut memo);

        // Materialise both intermediate artifacts with arbitrary content.
        std::fs::create_dir_all(wd.join("build")).unwrap();
        std::fs::write(wd.join("build/lib.txt"), "stale-lib-artifact").unwrap();
        std::fs::write(wd.join("build/app.txt"), "stale-app-artifact").unwrap();

        // After: artifacts exist on disk.
        let (dag, test_idx) = build_dag();
        let mut memo = BTreeMap::new();
        let after = collect_test_file_inputs(&dag, test_idx, wd, &mut memo);

        assert_eq!(
            before, after,
            "materialising intermediate or final artifacts must not change the fingerprint \
             (produced_upstream exclusion must apply to closure inputs in ≥2-level chains)"
        );

        // Sanity: editing the *source* must change the closure contribution.
        std::fs::write(wd.join("src/lib.txt"), "lib-source-EDITED").unwrap();
        let (dag, test_idx) = build_dag();
        let mut memo = BTreeMap::new();
        let after_src_edit = collect_test_file_inputs(&dag, test_idx, wd, &mut memo);

        assert_ne!(
            before, after_src_edit,
            "editing the upstream source must change the fingerprint"
        );
    }
}
