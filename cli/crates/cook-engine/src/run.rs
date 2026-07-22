//! Unified engine entry point for every build — a workspace of one or many.
//!
//! [`run`] takes a fully-built [`RegisteredWorkspace`] along with the
//! recipe-level dependency edges for the reachable target closure, then
//! executes the build by constructing a single unified work-unit DAG across
//! every reachable recipe and walking it with the shared executor pool.
//! Cross-recipe edges live directly on the unified DAG; there is no per-wave
//! register / DAG / execute loop (SHI-222 Phase 4).
//!
//! Callers build the [`RegisteredWorkspace`] via `pipeline::register_workspace`,
//! which runs `cook_register::register_cookfile` once per Cookfile and merges
//! per-import results. A single-Cookfile project (no imports) is a workspace
//! of one member — `register_workspace` is the only entry point.

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
    pub output_glob_warnings: Vec<OutputGlobWarning>,
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

/// Compute a test node's cache fingerprint at **ready time** — the point in
/// execution where all of the test's DAG predecessors have completed and
/// their declared outputs are materialised on disk (§18). Returns `None`
/// for a source-less test (no ingredients, no consumed predecessor output),
/// which per §8.6.1/§17.4 has no cache key and MUST always run.
///
/// COOK-211 / §17.4 rule 1: the key folds the OUTPUT CONTENT of the units
/// the test consumes — not their execution identity. Concretely it hashes:
///
/// * **Predecessor outputs** — the declared outputs of the test's IMMEDIATE
///   predecessors (`dag.deps`), content-hashed. These are exactly the
///   `$<NAME>` cross-recipe outputs the test references and the preceding
///   `cook` step's outputs that form its iteration source (§8.6.1). Hashing
///   only the immediate predecessors — not the transitive closure — is what
///   delivers early cutoff: a *transitive* dependency whose output stays
///   byte-identical does not re-invalidate the test, because its effect is
///   already captured in the immediate predecessor's (unchanged) output.
/// * **Own inputs** — the Test payload's `input_paths` that are NOT a
///   predecessor output (the test's own `ingredients`), content-hashed.
///
/// Because every folded path is content-hashed *after* its producer ran, a
/// dependency that re-executes but yields byte-identical output leaves the
/// test's fingerprint unchanged — the same early-cutoff a consuming `cook`
/// step gets (contrast the pre-COOK-211 fold, which mixed in the dep's
/// command hash and source inputs and so over-invalidated).
///
/// Pair keys are project-root-relative and forward-slashed (§17.4.4);
/// missing files hash to `"missing"`.
pub(crate) fn compute_ready_test_fingerprint(
    dag: &cook_dag::Dag<WorkNode>,
    test_idx: usize,
    cache_ctx: &CacheContext,
    probe_store: &cook_luaotp::ProbeValueStore,
) -> Option<String> {
    let project_root = cache_ctx.project_root.as_path();
    let test_node = dag.node(test_idx).payload();
    let WorkPayload::Test { input_paths, seal_keys, .. } = test_node.payload.as_ref()? else {
        return None;
    };

    let mut memo: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut pairs: BTreeMap<String, String> = BTreeMap::new();

    // (1) Immediate-predecessor outputs, content-hashed. Terminal (glob /
    //     directory) outputs are resolved against the now-materialised tree;
    //     literal outputs map directly. The normalized absolute set doubles
    //     as the "produced upstream" filter for step (2).
    let mut predecessor_outputs: BTreeSet<PathBuf> = BTreeSet::new();
    for &pred in dag.deps(test_idx) {
        let node = dag.node(pred).payload();
        let Some(meta) = &node.cache_meta else {
            continue;
        };
        for out in &meta.output_paths {
            if cook_fingerprint::is_terminal_output(out) {
                let pat = if out.ends_with('/') { format!("{out}**") } else { out.clone() };
                for rel in cook_fingerprint::resolve_glob(&node.working_dir, &pat) {
                    predecessor_outputs.insert(lexical_normalize(&node.working_dir.join(rel)));
                }
            } else {
                predecessor_outputs.insert(lexical_normalize(&node.working_dir.join(out)));
            }
        }
    }
    for abs in &predecessor_outputs {
        let hash = memo_hash(&mut memo, abs);
        pairs.insert(root_rel_key(project_root, abs), hash);
    }

    // (2) The test's own inputs (its `ingredients`) — every `input_paths`
    //     entry that is not itself a predecessor output.
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
            if predecessor_outputs.contains(&abs) {
                continue;
            }
            let hash = memo_hash(&mut memo, &abs);
            pairs.insert(root_rel_key(project_root, &abs), hash);
        }
    }

    // §8.6.1/§17.4: a source-less test — no ingredients, no consumed
    // predecessor output — has no cache key and MUST always run. Skipping
    // the fingerprint makes the executor bypass both the test-cache lookup
    // and the post-run write, so the test reports every invocation with no
    // `(cached)`. A stable command-text-only key would be a false green:
    // the true inputs of a `test { cargo test }` are opaque to Cook.
    //
    // CS-0159: a `seal` does NOT rescue a source-less test into being
    // cacheable. A seal ref is an *invalidate-only* determinant (§8.4.3), not
    // an input source — sealing `test { cargo test }` on a toolchain probe
    // would still leave the test's real inputs (the whole source tree)
    // unmodelled, so a key built from the seal alone would be exactly the
    // false green this guard exists to prevent. Seal narrows reuse of a key
    // that already exists; it never mints one.
    if pairs.is_empty() {
        return None;
    }

    let env_keys: Vec<(String, String)> = test_node
        .env_vars
        .iter()
        .filter(|(k, _)| !cache_ctx.denylist.is_ignored(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // CS-0159 (§17.4 rule 1): fold the effective seal set's probe values. The
    // register surface unioned these keys into the unit's probe-dependency
    // set, so every sealed probe has run and its value is in the store by the
    // time this ready-time fingerprint is computed — the same guarantee a
    // sealing cook unit relies on. `resolve_sealed_probes` is shared with the
    // cook path so both agree on the absent-key-folds-to-empty-string rule.
    let sealed_probes: Vec<(String, String)> =
        crate::seal::resolve_sealed_probes(seal_keys, probe_store)
            .into_iter()
            .collect();

    let inputs = FingerprintInputs {
        cook_outputs: pairs.into_iter().collect(),
        dep_outputs: vec![],
        env_keys,
        sealed_probes,
    };
    Some(cook_fingerprint::compute_test_fingerprint(
        test_node.payload.as_ref().expect("checked above"),
        &inputs,
    ))
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
            // `edges` is the closure graph and includes `orders` — names reached
            // only through fine-grained per-unit refs. Those carry their ordering
            // in `dep_edges`; promoting them here would manufacture the coarse
            // whole-recipe barrier the fine reference exists to avoid. Keep the
            // barrier set to what the recipe actually declared.
            let declared: std::collections::BTreeSet<&str> =
                units.deps.iter().map(|s| s.as_str()).collect();
            u.deps = deps
                .iter()
                .filter(|d| declared.contains(d.as_str()))
                .cloned()
                .collect();
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
            output_glob_warnings: vec![],
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

    // 6. Build the probe_units_by_node lookup from the unified DAG. Test
    //    fingerprints are no longer precomputed here: COOK-211 moved them to
    //    ready time (`compute_ready_test_fingerprint`), where a consumed
    //    dependency's output is materialised on disk and can be content-hashed
    //    for early cutoff — impossible upfront, before any dep has run.
    let test_cache = TestCache::new(cache_ctx.project_root.join(".cook"));
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
            rerun_patterns,
            &probe_units_by_node,
            dep_outputs,
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
    Ok(RunResult {
        test_results,
        swept,
        kept_modified,
        output_glob_warnings,
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

/// The on-disk index name a recipe's cache is stored under (`.cook/cache/<name>.toml`): the `recipe_name`
/// its captured units carry in their [`cook_contracts::CacheMeta`] (which the
/// executor uses as the manager's per-recipe key), falling back to the
/// recipe's own name for unit-less meta-targets.
///
/// `pub` because `cook-dag-viewer` performs the same lookup when it loads
/// per-recipe cache indexes for import members (workspace key "rust.build",
/// Cookfile-local index name "build").
pub fn recipe_cache_index_name(ru: &RecipeUnits, fallback: &str) -> String {
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
        if let Err(e) = cm.flush_all() {
            tracing::warn!("recipe cache not persisted: {e}; next run will re-execute");
        }
    }

    all_swept.sort();
    all_kept_modified.sort();
    (all_swept, all_kept_modified)
}

#[cfg(test)]
mod output_warning_tests {
    #[test]
    fn output_warning_keeps_recipe_attribution() {
        let tmp = tempfile::tempdir().unwrap();
        let warnings = super::collect_output_glob_warnings_for_recipe(
            "assets",
            tmp.path(),
            &["dist/**".to_string(), "manifest.json".to_string()],
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].pattern, "dist/**");
        assert_eq!(warnings[0].recipe, "assets");
    }
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
            warnings: Vec::new(),
            names: Vec::new(),
            units_by_recipe: BTreeMap::new(),
            probes: BTreeMap::new(),
            final_env_by_cookfile: BTreeMap::new(),
            working_dir_by_prefix: BTreeMap::new(),
            alias_dirs_by_prefix: BTreeMap::new(),
            terminal_outputs: BTreeMap::new(),
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
    // compute_ready_test_fingerprint — COOK-211 ready-time content fold (§17.4)
    // -----------------------------------------------------------------------

    fn make_cache_ctx(wd: &std::path::Path) -> CacheContext {
        use cook_cache::{backend::LocalBackend, cloud_config::CloudConfig};
        use cook_fingerprint::EnvDenylist;
        CacheContext {
            denylist: std::sync::Arc::new(EnvDenylist::baseline()),
            backend: std::sync::Arc::new(LocalBackend::new(wd.join("cloud"))),
            cloud_config: std::sync::Arc::new(CloudConfig::default()),
            project_root: wd.to_path_buf(),
            project_id: "test".to_string(),
            publish_enabled: true,
        }
    }

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
                seal_keys: Default::default(),
                cmd: "check".into(),
                line: 1,
                timeout: 5,
                should_fail: false,
                suite_name: "s".into(),
                test_name: "t".into(),
                iteration_item: None,
                lua_code: None,
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

    /// One cook node (`lib`) producing `build/lib.txt` (command hash
    /// `lib_cmd`), plus a test node consuming its own source (`src/own.txt`)
    /// and the dep artifact (`build/lib.txt`). The dep output is materialised
    /// on disk with `lib_output`, as it would be at the test's ready time.
    fn materialised_fixture(
        wd: &std::path::Path,
        lib_cmd: u64,
        lib_output: &str,
        own: &str,
    ) -> (cook_dag::Dag<crate::WorkNode>, usize) {
        std::fs::create_dir_all(wd.join("src")).unwrap();
        std::fs::create_dir_all(wd.join("build")).unwrap();
        std::fs::write(wd.join("src/lib.txt"), "lib-src").unwrap();
        std::fs::write(wd.join("src/own.txt"), own).unwrap();
        std::fs::write(wd.join("build/lib.txt"), lib_output).unwrap();

        let mut dag = cook_dag::Dag::new();
        let lib = dag
            .add_node(
                cook_work_node(wd, "lib", &["src/lib.txt"], &["build/lib.txt"], lib_cmd),
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

    /// COOK-211 / §17.4: a dependency that re-executes (different command
    /// hash) but produces byte-identical output leaves the consuming test's
    /// fingerprint UNCHANGED — the fold is over the dep's output content, not
    /// its execution identity. This is the early cutoff the fix delivers.
    #[test]
    fn test_fp_stable_when_dep_output_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let ctx = make_cache_ctx(wd);

        let (dag_a, test_a) = materialised_fixture(wd, 11, "ok", "own");
        let before = compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());

        // lib's command changes (rebuild) but its output stays "ok".
        let (dag_b, test_b) = materialised_fixture(wd, 12, "ok", "own");
        let after = compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

        assert!(before.is_some());
        assert_eq!(
            before, after,
            "a byte-identical dep OUTPUT must leave the test fingerprint stable \
             even when the dep's command hash changes (early cutoff)"
        );
    }

    /// The consuming test IS re-keyed when the dep's OUTPUT CONTENT changes.
    #[test]
    fn test_fp_changes_when_dep_output_content_changes() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let ctx = make_cache_ctx(wd);

        let (dag_a, test_a) = materialised_fixture(wd, 11, "ok", "own");
        let before = compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());

        let (dag_b, test_b) = materialised_fixture(wd, 11, "broken", "own");
        let after = compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

        assert_ne!(
            before, after,
            "changing the dep's output content must re-key the test"
        );
    }

    /// Editing the test's own ingredient re-keys it.
    #[test]
    fn test_fp_changes_when_own_input_changes() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let ctx = make_cache_ctx(wd);

        let (dag_a, test_a) = materialised_fixture(wd, 11, "ok", "own");
        let before = compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());

        let (dag_b, test_b) = materialised_fixture(wd, 11, "ok", "changed");
        let after = compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

        assert_ne!(
            before, after,
            "editing the test's own ingredient must re-key the test"
        );
    }

    /// A source-less test — no ingredients, no consumed predecessor output —
    /// has no cache key and always runs (§8.6.1/§17.4).
    #[test]
    fn test_fp_none_for_sourceless_test() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let ctx = make_cache_ctx(wd);

        let mut dag = cook_dag::Dag::new();
        let test = dag.add_node(test_work_node(wd, &[]), &[]).unwrap();

        assert_eq!(
            compute_ready_test_fingerprint(&dag, test, &ctx, &cook_luaotp::ProbeValueStore::new()),
            None,
            "a source-less test must have no fingerprint (always runs)"
        );
    }

    /// Three-node chain lib → app → test (test consumes only `build/app.txt`).
    /// A change to the TRANSITIVE dep's output (`build/lib.txt`) that leaves
    /// the DIRECT dep's output (`build/app.txt`) byte-identical must NOT
    /// re-key the test — only the immediate predecessor's output is folded,
    /// so early cutoff chains correctly. Changing `build/app.txt` DOES re-key.
    #[test]
    fn test_fp_two_level_chain_early_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        let ctx = make_cache_ctx(wd);

        let build = |lib_out: &str, app_out: &str| {
            std::fs::create_dir_all(wd.join("build")).unwrap();
            std::fs::write(wd.join("build/lib.txt"), lib_out).unwrap();
            std::fs::write(wd.join("build/app.txt"), app_out).unwrap();
            let mut dag = cook_dag::Dag::new();
            let lib = dag
                .add_node(cook_work_node(wd, "lib", &[], &["build/lib.txt"], 11), &[])
                .unwrap();
            let app = dag
                .add_node(
                    cook_work_node(wd, "app", &["build/lib.txt"], &["build/app.txt"], 22),
                    &[lib],
                )
                .unwrap();
            let test = dag
                .add_node(test_work_node(wd, &["build/app.txt"]), &[app])
                .unwrap();
            (dag, test)
        };

        let (dag_a, test_a) = build("lib-v1", "app-out");
        let base = compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());
        assert!(base.is_some());

        // Transitive dep output changes; direct dep output unchanged → stable.
        let (dag_b, test_b) = build("lib-v2", "app-out");
        assert_eq!(
            base,
            compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new()),
            "a transitive dep's output change must not re-key the test when the \
             immediate predecessor's output is byte-identical"
        );

        // Direct dep output changes → re-key.
        let (dag_c, test_c) = build("lib-v2", "app-out-CHANGED");
        assert_ne!(
            base,
            compute_ready_test_fingerprint(&dag_c, test_c, &ctx, &cook_luaotp::ProbeValueStore::new()),
            "the immediate predecessor's output change must re-key the test"
        );
    }
}
