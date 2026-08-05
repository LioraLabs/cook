//! Unified engine entry point for every build — a workspace of one or many.
//!
//! [`run`] takes a fully-built [`RegisteredWorkspace`] along with the
//! recipe-level dependency edges for the reachable target closure, then
//! executes the build by constructing a single unified work-unit DAG across
//! every reachable recipe and walking it with the shared executor pool.
//! Cross-recipe edges live directly on the unified DAG; there is no per-wave
//! register / DAG / execute loop (SHI-222 Phase 4).
//!
//! Callers build the [`RegisteredWorkspace`] via
//! `cook_plan::register_workspace`, which runs registration once per
//! Cookfile and merges per-import results. A single-Cookfile project (no
//! imports) is a workspace of one member — `register_workspace` is the only
//! entry point. This crate consumes the result as contract data; it has no
//! dependency on cook-plan or cook-register (COOK-428).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;

use cook_cache::{
    backend::LocalBackend, cache_ctx::CacheContext, cloud_backend::CloudBackend,
    cloud_config::CloudConfig, ThreadSafeCacheManager,
};
use cook_contracts::{RecipeUnits, WorkPayload};
use cook_cache::{CacheBackend, EnvDenylist};

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
    /// Stale-output reconciliation summary (§17.7). Empty under `--no-prune`
    /// or when nothing was orphaned. `swept` are the orphaned outputs Cook
    /// removed; `kept_modified` are orphans kept because they changed since
    /// Cook wrote them. Surfaced here (rather than printed by the engine) so
    /// the CLI can report them after the progress renderer has finished.
    pub swept: Vec<std::path::PathBuf>,
    pub kept_modified: Vec<std::path::PathBuf>,
    pub output_glob_warnings: Vec<OutputGlobWarning>,
    /// How many CAS publish operations this run attempted — each increment
    /// precedes its `put`, so a failed write still counts. That is the
    /// conservative direction for a zero / non-zero gate: over-counting costs
    /// one unnecessary store walk, under-counting would skip a check that was
    /// due.
    ///
    /// Read once, after every worker has joined, and consumed purely as a gate
    /// for the end-of-run store-budget check: a run that published no outputs
    /// skips walking the shared store entirely, which is what keeps a settled
    /// no-op build at zero added cost.
    ///
    /// Read the gate precisely: this counts **published outputs**, which is
    /// narrower than "the store did not grow". The register-phase probe
    /// pre-pass writes probe values to the CAS outside every publish guard and
    /// outside this counter (COOK-339), so a run can add a handful of
    /// `probe_value` objects while this still reads 0. Those objects are
    /// kilobytes against a budget in gigabytes, so the only consequence is
    /// that an over-budget warning waits for the next run that publishes
    /// something.
    ///
    /// That bound is the honest one, and it is weaker than "one build later":
    /// the budget check is stateless by design (no stamp file, no rate
    /// limiting), so *only* a publishing run reports. A store pushed over
    /// budget — by the pre-pass writes above, by a concurrent project sharing
    /// a `[cache] cache_dir`, or by an earlier run whose warning scrolled past
    /// — stays quietly over budget for as long as subsequent runs publish
    /// nothing. A settled build publishes nothing and therefore never warns;
    /// that is the intended cadence, not a defect. `cook cache du` is the
    /// on-demand way to ask regardless of what the last run published.
    pub published_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputGlobWarning {
    pub pattern: String,
    pub recipe: String,
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
    replay_logs: bool,
    on_event: F,
) -> Result<RunResult, EngineError>
where
    F: Fn(EngineEvent) + Send + Sync,
{
    let started = std::time::Instant::now();
    let mut cache_ctx = match build_cache_ctx(project_root, no_publish) {
        Ok(c) => c,
        Err(e) => {
            on_event(EngineEvent::Finished {
                elapsed: started.elapsed(),
                success: false,
            });
            return Err(e);
        }
    };
    Arc::make_mut(&mut cache_ctx).replay_logs = replay_logs;
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
    let topo_order = cook_contracts::unit_graph::toposort_recipes(edges, reachable)
        .map_err(EngineError::from)?;
    let mut all_units: Vec<RecipeUnits> = Vec::with_capacity(topo_order.len());
    for name in &topo_order {
        let units = registered_workspace
            .units_by_recipe
            .get(name)
            .ok_or_else(|| EngineError::UnknownRecipe(name.clone()))?;
        let mut u = units.clone();
        if let Some(deps) = edges.get(name) {
            // `edges` is the closure graph and includes `orders` — names reached
            // only through fine-grained per-unit refs. Those carry their ordering
            // in `dep_edges`; promoting them here would manufacture the coarse
            // whole-recipe barrier the fine reference exists to avoid. Keep the
            // barrier set to what the recipe actually declared. The filter is
            // shared with every other closure-stamping call site so `run`,
            // `why`, and the graph renderer agree on which barriers exist.
            u.deps = cook_contracts::unit_graph::declared_coarse_deps(&units.deps, deps);
        }
        all_units.push(u);
    }

    // 1b. §16.1.2 read-after-write rule: CLOSURE-scoped structural check.
    //     A literal outputs[] entry of one recipe equal to a literal inputs[]
    //     entry of another, with no ordering path from the reader to the
    //     writer, is a silent stale read under --jobs > 1. Rejected here —
    //     NOT repaired: §10.6 forbids inferring an edge from path equality.
    //
    //     Scope is `all_units` (the reachable closure), deliberately NOT the
    //     workspace-wide `all_workspace_units` used by §22.1.2 above. A
    //     literal output is not build-owned, so `cook producer && cook
    //     consumer` is legitimate and MUST NOT be rejected; §22.1.2's
    //     terminality, by contrast, is an ownership claim that holds
    //     workspace-wide. See `tests/raw_path_cross_recipe_edge.rs`, which
    //     pins the out-of-closure case a workspace-wide check would break.
    dag_builder::check_literal_read_after_write(&all_units)?;

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
        // The kind on `RegisteredRecipePub` is the register-phase
        // `cook_contracts::registration::RecipeKind`, which has the same
        // variants but is a distinct type (Task 4.1).
        let kind_by_name: BTreeMap<&str, RecipeKind> = registered_workspace
            .names
            .iter()
            .map(|r| {
                let kind = match r.kind {
                    cook_contracts::registration::RecipeKind::Recipe => RecipeKind::Recipe,
                    cook_contracts::registration::RecipeKind::Chore => RecipeKind::Chore,
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
            output_glob_warnings: vec![],
            // Nothing was ever dispatched, so nothing published.
            published_count: 0,
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
            let index_name = cook_contracts::cache::recipe_cache_index_name(ru, name);
            let prior = cm.get_or_load(&index_name);
            let mut outs: BTreeMap<PathBuf, u64> = BTreeMap::new();
            for step in prior.steps.values() {
                for o in &step.outputs {
                    outs.insert(ru.working_dir.join(&*o.path), o.hash);
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

    // 6. Build the probe_units_by_node lookup from the unified DAG. No test
    //    unit bookkeeping rides along: a test unit's inputs are on its
    //    `CacheMeta` like every other unit's (CS-0186), and the engine expands
    //    any pattern among them when the unit is ready — impossible upfront,
    //    before the dependency that writes them has run.
    let probe_units_by_key: BTreeMap<String, cook_contracts::ProbeUnit> = registered_workspace
        .probes
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let probe_units_by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = (0..dag.len())
        .filter_map(|node_idx| {
            let work_node = dag.node(node_idx).payload();
            if let Some(WorkPayload::Probe { key, .. }) = &work_node.payload {
                // The payload key is Cookfile-local, but `RegisteredWorkspace
                // .probes` keys imported-Cookfile probes workspace-qualified
                // (registers.rs `qualify`). A probe unit always registers in
                // the same Cookfile as its surrounding recipe, and recipe
                // local names never contain '.', so the recipe's qualified
                // prefix locates the entry. Without this, every imported-
                // Cookfile probe missed its metadata here and silently lost
                // fingerprint caching (always re-ran); CS-0148's `files`
                // sentinel made the miss loud by reaching a worker as Lua.
                let qualified = match work_node.recipe_name.rfind('.') {
                    Some(idx) => {
                        format!("{}.{}", &work_node.recipe_name[..idx], key)
                    }
                    None => key.clone(),
                };
                probe_units_by_key
                    .get(&qualified)
                    .or_else(|| probe_units_by_key.get(key))
                    .map(|pu| (node_idx, pu.clone()))
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
    let dep_outputs: cook_luaotp::WorkerDepOutputs =
        std::sync::Arc::new(registered_workspace.terminal_outputs.clone());

    // CAS publish counter for the end-of-run store-budget check. Owned here,
    // bumped once per publish inside the executor, read back below once every
    // worker has joined. Only its zero / non-zero-ness is consumed. See
    // `RunResult::published_count` for what the gate does and does not claim.
    let published = std::sync::atomic::AtomicU64::new(0);

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
            rerun_patterns,
            &probe_units_by_node,
            dep_outputs,
            &published,
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

    let output_glob_warnings = collect_output_glob_warnings(registered_workspace, reachable);
    // Every worker has joined by now (execute_dag returned), so a single
    // relaxed load sees the run's total.
    let published_count = published.load(std::sync::atomic::Ordering::Relaxed);
    Ok(RunResult {
        test_results,
        swept,
        kept_modified,
        output_glob_warnings,
        published_count,
    })
}

fn collect_output_glob_warnings(
    registered_workspace: &RegisteredWorkspace,
    reachable: &BTreeSet<String>,
) -> Vec<OutputGlobWarning> {
    let mut warnings = Vec::new();
    for recipe in reachable {
        let Some(units) = registered_workspace.units_by_recipe.get(recipe) else {
            continue;
        };
        for unit in &units.units {
            for warning in collect_output_glob_warnings_for_recipe(
                recipe,
                &units.working_dir,
                &unit.output_paths,
            ) {
                warnings.push(warning);
            }
        }
    }
    warnings
}

fn collect_output_glob_warnings_for_recipe(
    recipe: &str,
    working_dir: &Path,
    output_paths: &[String],
) -> Vec<OutputGlobWarning> {
    crate::executor::resolve_output_paths_with_unmatched(output_paths, working_dir)
        .unmatched_patterns
        .into_iter()
        .map(|pattern| OutputGlobWarning {
            pattern,
            recipe: recipe.to_string(),
        })
        .collect()
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
            let index_name = cook_contracts::cache::recipe_cache_index_name(ru, name);
            let wd = ru.working_dir.clone();
            let live_ref = &live;
            cm.retain_steps(&index_name, move |_k, step| {
                step.outputs.is_empty()
                    || step
                        .outputs
                        .iter()
                        .any(|o| live_ref.contains(&wd.join(&*o.path)))
            });
        }
    }

    // Persist any pruned caches (flush_all is a no-op for unchanged recipes).
    for cm in cache_managers.values() {
        if let Err(e) = cm.flush_all() {
            tracing::warn!("recipe cache not persisted: {e}; next run will re-execute");
        }
    }

    all_swept.sort();
    all_kept_modified.sort();
    (all_swept, all_kept_modified)
}

#[cfg(test)]
#[path = "tests/output_warning_tests.rs"]
mod output_warning_tests;

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
            let cache_dir = cook_contracts::layout::cache_dir(&wd);
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
    // COOK-232: shared with `CloudConfig::resolved_cache_dir` so `cook cache
    // du` reports on the exact directory the engine writes to.
    let cache_dir = cloud_config.resolved_cache_dir();
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
    // CS-0196 (COOK-364): key-side identity is configured-or-empty. The
    // directory-name fallback never enters a key.
    let project_id = cloud_config.project_id_for_keys();
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
        replay_logs: false,
    }))
}

#[cfg(test)]
#[path = "tests/run_tests.rs"]
mod tests;
