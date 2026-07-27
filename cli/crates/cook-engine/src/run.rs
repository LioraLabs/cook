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
    cloud_config::CloudConfig, ThreadSafeCacheManager,
};
use cook_contracts::{RecipeUnits, WorkPayload};
use cook_fingerprint::{CacheBackend, EnvDenylist};

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

/// Resolve a test node's input set at **ready time** — the point in execution
/// where all of the test's DAG predecessors have completed and their declared
/// outputs are materialised on disk (§18). Returns paths relative to the test
/// unit's own working directory, ready to be handed to
/// `needs_rebuild_cook` / `record_completion` exactly as a `cook` unit's
/// declared inputs are.
///
/// Returns `None` for a source-less test (no ingredients, no consumed
/// predecessor output), which per §8.6.1/§17.4 has no cache key and MUST
/// always run.
///
/// **What CS-0186 changed here, and what it did not.** This was
/// `compute_ready_test_fingerprint`, and it hashed the same paths into a
/// content-addressed digest that addressed a store of its own. It now yields
/// the PATHS and stops. Hashing them is `check_inputs`' job, recording them is
/// `record_completion`'s, and both already did it for every other unit. What
/// is unchanged is WHICH paths — the selection below is the same selection,
/// because it was never the part that differed:
///
/// * **Predecessor outputs** — the declared outputs of the test's IMMEDIATE
///   predecessors (`dag.deps`). These are exactly the `$<NAME>` cross-recipe
///   outputs the test references and the preceding `cook` step's outputs that
///   form its iteration source (§8.6.1). Taking only the immediate
///   predecessors — not the transitive closure — is what delivers early
///   cutoff: a *transitive* dependency whose output stays byte-identical does
///   not re-invalidate the test, because its effect is already captured in the
///   immediate predecessor's (unchanged) output.
/// * **Own inputs** — the unit's `input_paths` (its `ingredients`), less any
///   that are already a predecessor output.
///
/// The early-cutoff property the content fold delivered is preserved, and by
/// the same mechanism a consuming `cook` step has always used: these paths are
/// content-hashed when the unit becomes ready, which is *after* their
/// producers ran, so a dependency that re-executes to byte-identical output
/// leaves this unit's recorded inputs unchanged and its record replayable.
/// That equivalence is the substance of the fold — see §17.4's closing note.
pub(crate) fn consumed_inputs_at_ready_time(
    dag: &cook_dag::Dag<WorkNode>,
    test_idx: usize,
    project_root: &Path,
) -> Option<Vec<String>> {
    let test_node = dag.node(test_idx).payload();
    let WorkPayload::Test { input_paths, consumes, .. } = test_node.payload.as_ref()?
    else {
        return None;
    };

    // §8.6.1 / §17.4 rule 1 (COOK-342): whether a test is cached at all is
    // derived from the recipe's DECLARED source, and must be decided before
    // any fold — not inferred afterwards from whatever happened to land in
    // `pairs`. §8.6.1 defines the source as the preceding cook step's outputs,
    // falling back to the recipe's resolved ingredients; §17.4 rule 1 admits
    // only outputs the test CONSUMES (source-list flattening) or REFERENCES
    // (`$<NAME>` / `cook.dep_output`, both of which land in `input_paths`).
    //
    // A dep-list entry (`recipe check: build`) is none of those. It is a
    // whole-recipe ordering barrier, and it was minting a key for tests the
    // Standard says have none: `test { cargo test }` — §8.6.1's Example 8.6.2
    // verbatim — became cacheable, and its correctness became hostage to a
    // file it never reads. It re-ran only when the unrelated dependency
    // changed, which is the false green §8.6 names outright.
    //
    // The guard is deliberately on the declared source rather than on the
    // predecessor fold, so a test that DOES declare a source keeps folding
    // every predecessor's outputs exactly as before — CS-0175's `consumes`
    // narrowing operates on that fold and is untouched.
    let has_declared_source = !input_paths.is_empty()
        || dag.deps(test_idx).iter().any(|&pred| {
            let node = dag.node(pred).payload();
            node.recipe_name == test_node.recipe_name
                && node
                    .cache_meta
                    .as_ref()
                    .is_some_and(|m| !m.output_paths.is_empty())
        });
    if !has_declared_source {
        return None;
    }

    // Collected relative to the test unit's own working directory and sorted,
    // so the recorded input list is a deterministic function of the graph
    // rather than of iteration order. `check_inputs` compares this set against
    // the recorded one, so an unstable order would read as an input-set change
    // and rebuild on every run.
    let mut resolved: BTreeSet<String> = BTreeSet::new();

    // (1) Immediate-predecessor outputs. Terminal (glob / directory) outputs
    //     are resolved against the now-materialised tree; literal outputs map
    //     directly. The normalized absolute set doubles as the "produced
    //     upstream" filter for step (2).
    let mut predecessor_outputs: BTreeSet<PathBuf> = BTreeSet::new();
    for &pred in dag.deps(test_idx) {
        let node = dag.node(pred).payload();
        let Some(meta) = &node.cache_meta else {
            continue;
        };
        for out in &meta.output_paths {
            if cook_fingerprint::is_terminal_output(out) {
                // CS-0085 normalisation, the same the executor applies when
                // the producing unit captures these outputs
                // (executor.rs `normalize_glob_pattern`). Without it a
                // trailing bare `**` resolves to NOTHING here — the glob
                // crate treats it as directories-only and `resolve_glob`
                // then drops directories — so a predecessor declaring the
                // canonical `dist/**` contributed no content to this key at
                // all. That is under-keying: a real dependency change left
                // the check's fingerprint untouched and replayed a stale
                // pass. Directory outputs (`dist/`) were broken the same
                // way, via the old `{out}**`.
                let pat = cook_fingerprint::normalize_glob_pattern(out);
                for rel in cook_fingerprint::resolve_glob(&node.working_dir, &pat) {
                    predecessor_outputs.insert(lexical_normalize(&node.working_dir.join(rel)));
                }
            } else {
                predecessor_outputs.insert(lexical_normalize(&node.working_dir.join(out)));
            }
        }
    }
    // `consumes` narrows WHICH of those outputs fold, without touching the
    // set itself: `predecessor_outputs` stays whole so step (2) still treats
    // an excluded artifact as produced-upstream and does not re-admit it
    // through the test's own glob inputs. A dependency's `dist/**` ingested
    // wholesale re-keys this check on artifacts it never opens — sourcemaps
    // above all, which esbuild-family tools rewrite on any comment edit.
    // Empty filter, or a filter that matches nothing, folds everything
    // (cook_fingerprint::consumes: over-folding is the safe failure).
    let all: Vec<PathBuf> = predecessor_outputs.iter().cloned().collect();
    let folded: Vec<&PathBuf> = match cook_fingerprint::ConsumesFilter::compile(consumes) {
        Ok(f) => f.select(&all, |p| root_rel_key(project_root, p)),
        // Patterns are validated at register time; a bad one reaching here
        // folds everything rather than silently narrowing the key.
        Err(_) => all.iter().collect(),
    };
    for abs in folded {
        resolved.insert(unit_rel_path(&test_node.working_dir, abs));
    }

    // (2) The test's own inputs (its `ingredients`) — every `input_paths`
    //     entry that is not itself a predecessor output.
    for input in input_paths {
        let matches: Vec<PathBuf> = if cook_fingerprint::has_glob_meta(input) {
            cook_fingerprint::resolve_glob(&test_node.working_dir, input)
                .into_iter()
                .map(|rel| test_node.working_dir.join(rel))
                .collect()
        } else {
            vec![test_node.working_dir.join(input)]
        };
        for abs in matches {
            let abs = lexical_normalize(&abs);
            if predecessor_outputs.contains(&abs) {
                continue;
            }
            resolved.insert(unit_rel_path(&test_node.working_dir, &abs));
        }
    }

    // §8.6.1/§17.4: a source-less test — no ingredients, no consumed
    // predecessor output — has no cache key and MUST always run. Returning
    // `None` makes the executor treat the unit as uncacheable at both the
    // check and the publish, so the test reports every invocation with no
    // `(cached)` and leaves no record behind. A stable command-text-only key
    // would be a false green: the true inputs of a `test { cargo test }` are
    // opaque to Cook.
    //
    // Reachable even past the `has_declared_source` guard above, which reads
    // DECLARATIONS: a declared glob matching nothing, or a predecessor whose
    // terminal output resolves empty, declares a source that turns out to name
    // no file.
    //
    // CS-0159: a `seal` does NOT rescue a source-less test into being
    // cacheable. A seal ref is an *invalidate-only* determinant (§8.4.3), not
    // an input source — sealing `test { cargo test }` on a toolchain probe
    // would still leave the test's real inputs (the whole source tree)
    // unmodelled, so a key built from the seal alone would be exactly the
    // false green this guard exists to prevent. Seal narrows reuse of a key
    // that already exists; it never mints one. Nothing below folds a seal:
    // the effective seal set reaches the unit's key through the shared
    // `seal_contribution` every unit uses (§17.1.1), which is where CS-0159
    // has always wanted it.
    if resolved.is_empty() {
        return None;
    }
    Some(resolved.into_iter().collect())
}

/// Express `abs` relative to the unit's working directory, forward-slashed.
///
/// Every recorded input path is resolved as `working_dir.join(path)`, so this
/// is the form the record must hold. `..` segments are expected and legal
/// (§17.5 rule 3): a test consuming a predecessor from another Cookfile
/// reaches upward, and joining resolves it. An absolute path would work
/// mechanically — `join` replaces on an absolute argument — and would break
/// §17.5 rule 1 the moment the workspace moved on disk, so it is the fallback
/// only when no relative path exists (a different drive on Windows).
fn unit_rel_path(working_dir: &Path, abs: &Path) -> String {
    pathdiff::diff_paths(abs, working_dir)
        .unwrap_or_else(|| abs.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
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
            let index_name = recipe_cache_index_name(ru, name);
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

    // 6. Build the probe_units_by_node lookup from the unified DAG. A test
    //    unit's input set is not precomputed here: COOK-211 moved it to ready
    //    time (`consumed_inputs_at_ready_time`), where a consumed dependency's
    //    output is materialised on disk — impossible upfront, before any dep
    //    has run.
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

/// The on-disk index name a recipe's cache is stored under (`.cook/cache/<name>.idx`): the `recipe_name`
/// its captured units carry in their [`cook_contracts::CacheMeta`] (which the
/// executor uses as the manager's per-recipe key), falling back to the
/// recipe's own name for unit-less meta-targets.
///
/// `pub` because `cook-graph` performs the same lookup when it loads
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
#[path = "tests/run_tests.rs"]
mod tests;
