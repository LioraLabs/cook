//! Read-only `cook why` determinant explanation (COOK-165, §17.1.6).
//!
//! Builds, per cacheable unit, the COMPLETE attributed cache key K and a
//! hit/miss classification; on a shared miss it diffs the consumer's resolved
//! determinants against the producer determinant manifest (COOK-166) fetched by
//! K, naming the differing determinant(s). Executes nothing.

use std::collections::BTreeMap;

use cook_cache::backend::DeterminantManifest;
/// CS-0157: fresh PATH resolution for `cook why`'s tool-path display — the
/// sealed value no longer carries a path, so the CLI resolves it at query
/// time (re-exported here so cook-cli needs no direct fingerprint dep).
pub use cook_cache::resolve_tool_path;

/// How a unit's cache lookup resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    LocalHit,
    SharedHit,
    SharedMiss,
    LocalOnlyMiss,
    PinnedColdMiss,
    /// A declared input is absent on disk: the unit cannot be a clean hit and no
    /// key is computed (mirrors `hash_input_paths` returning None). Rendered as
    /// `MISS (input '<path>' missing)`.
    ///
    /// CS-0173: reserved for an input **no unit in the closure produces**. An
    /// input that is merely not restored yet is resolved from its producer
    /// instead; one whose producer rebuilds is `ForcedByUpstream`.
    MissingInput { path: String },
    /// CS-0173: an input to this unit is an output of a unit that will itself
    /// rebuild, so the bytes this unit would consume do not exist yet in their
    /// final form and cannot be known without running the producer. No key is
    /// computable, so none is reported.
    ForcedByUpstream { producer: String, path: String },
    /// A sealed produce-body probe has never materialised a value. `why` does
    /// not execute produce bodies, so the unit has no computable key.
    UnmaterialisedProbe { key: String },
}

/// One determinant difference found when diffing consumer determinants against a
/// producer manifest on a shared miss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeterminantDiff {
    CommandHash { ours: u64, theirs: u64 },
    EnvContribution { ours: u64, theirs: u64 },
    SealContribution { ours: u64, theirs: u64 },
    Input { path: String, ours: Option<u64>, theirs: Option<u64> },
    Env { key: String, ours: Option<String>, theirs: Option<String> },
    Probe { key: String, ours: Option<String>, theirs: Option<String> },
    OutputPaths { ours: Vec<String>, theirs: Vec<String> },
}

/// CS-0173: what one declared output will contain by the time a downstream unit
/// reads it, established by classifying the producing unit first.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Prediction {
    /// The producing unit is served from cache, so its output bytes are already
    /// determined and this is their content hash — whether or not they have been
    /// restored to the working tree yet.
    Known(u64),
    /// The producing unit will rebuild. Its output bytes cannot be known without
    /// running it, so no downstream key over them is computable.
    Unknowable,
}

/// Absolute output path → (what will be there, which recipe puts it there).
///
/// Keyed by ABSOLUTE path because a producer and its consumer routinely sit in
/// different Cookfiles with different working directories: `build/theme.css`
/// declared in `apps/web/theme` and `theme/build/theme.css` consumed from
/// `apps/web` are one file, and only the resolved path says so.
type Predictions = BTreeMap<std::path::PathBuf, (Prediction, String)>;

/// The consumer-side resolved determinants for one unit (the data behind K).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitDeterminants {
    pub command_hash: u64,
    pub env_contribution: u64,
    pub seal_contribution: u64,
    pub inputs: BTreeMap<String, u64>,
    pub output_paths: Vec<String>,
    pub consulted_env: BTreeMap<String, String>,
    pub sealed_probes: BTreeMap<String, String>,
    /// CS-0173: declared inputs whose content is not yet determined, because the
    /// unit producing them will itself rebuild. Path → producing recipe name.
    ///
    /// A path listed here is deliberately ABSENT from `inputs` rather than
    /// present with a stale or zero hash. Reporting the bytes currently on disk
    /// as this unit's determinant would be reporting a value the run will not
    /// use, which is the precise error CS-0173 exists to remove.
    pub pending_inputs: BTreeMap<String, String>,
    /// CS-0245 / §17.1.6.1: keys of this unit's effective seal set whose
    /// reported value came from a PRIOR invocation rather than being resolved
    /// or produced by this one — case (c) of §17.1.6.1's trichotomy. Neither
    /// this query's own fresh re-resolution (case a: a `files`/`tools`
    /// declaration synthesised with no VM) nor this invocation's own
    /// registration pre-pass (case b: a `gather <probe>` source or a
    /// register-phase Lua read) produced the value; it is whatever a prior
    /// invocation last materialised. Empty for a unit with no sealed probes at
    /// all in either category.
    pub prior_invocation_probes: BTreeSet<String>,
    /// CS-0245 / §17.1.6.1: fresh-resolution failures for keys in this unit's
    /// effective seal set — a `tools` name no longer on PATH (CS-0214), or a
    /// `files` match that exists but cannot be read (CS-0241). Failing to
    /// resolve fresh does not fail the query: the key falls back to its last
    /// materialised value (and so is also in `prior_invocation_probes`), and
    /// the failure text is carried here for the renderer.
    pub probe_lookup_failures: BTreeMap<String, String>,
}

/// Diff the consumer determinants against a producer manifest, in a stable order
/// (the variant order above, keys sorted within).
pub fn diff_against_manifest(
    ours: &UnitDeterminants,
    theirs: &DeterminantManifest,
) -> Vec<DeterminantDiff> {
    let mut out = Vec::new();
    if ours.command_hash != theirs.command_hash {
        out.push(DeterminantDiff::CommandHash { ours: ours.command_hash, theirs: theirs.command_hash });
    }
    if ours.env_contribution != theirs.env_contribution {
        out.push(DeterminantDiff::EnvContribution { ours: ours.env_contribution, theirs: theirs.env_contribution });
    }
    if ours.seal_contribution != theirs.seal_contribution {
        out.push(DeterminantDiff::SealContribution { ours: ours.seal_contribution, theirs: theirs.seal_contribution });
    }
    diff_map_u64(&ours.inputs, &theirs.inputs, |path, o, t| {
        out.push(DeterminantDiff::Input { path, ours: o, theirs: t });
    });
    diff_map_str(&ours.consulted_env, &theirs.consulted_env, |key, o, t| {
        out.push(DeterminantDiff::Env { key, ours: o, theirs: t });
    });
    diff_map_str(&ours.sealed_probes, &theirs.sealed_probes, |key, o, t| {
        out.push(DeterminantDiff::Probe { key, ours: o, theirs: t });
    });
    if ours.output_paths != theirs.output_paths {
        out.push(DeterminantDiff::OutputPaths {
            ours: ours.output_paths.clone(),
            theirs: theirs.output_paths.clone(),
        });
    }
    out
}

fn diff_map_u64(
    ours: &BTreeMap<String, u64>,
    theirs: &BTreeMap<String, u64>,
    mut emit: impl FnMut(String, Option<u64>, Option<u64>),
) {
    let keys: std::collections::BTreeSet<&String> = ours.keys().chain(theirs.keys()).collect();
    for k in keys {
        let (o, t) = (ours.get(k).copied(), theirs.get(k).copied());
        if o != t { emit(k.clone(), o, t); }
    }
}

fn diff_map_str(
    ours: &BTreeMap<String, String>,
    theirs: &BTreeMap<String, String>,
    mut emit: impl FnMut(String, Option<String>, Option<String>),
) {
    let keys: std::collections::BTreeSet<&String> = ours.keys().chain(theirs.keys()).collect();
    for k in keys {
        let (o, t) = (ours.get(k).cloned(), theirs.get(k).cloned());
        if o != t { emit(k.clone(), o, t); }
    }
}

// ---------------------------------------------------------------------------
// Read-only `explain()` walk (COOK-165 Task 3)
// ---------------------------------------------------------------------------

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use cook_cache::cache_ctx::CacheContext;
use cook_cache::ThreadSafeCacheManager;

use crate::{dag_builder, RegisteredWorkspace, WorkNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Unannotated,
    Local,
    Pinned,
}

#[derive(Debug, Clone)]
pub struct WhyUnit {
    pub recipe_name: String,
    pub cache_key: String,
    pub key_hex: String,
    pub disposition: Disposition,
    pub status: CacheStatus,
    /// COOK-276: explicit local-tier answer, independent of the shared tier.
    /// `status` alone conflates them — a locally-warm unit whose artifact is
    /// absent from the shared store must not read as "will rebuild".
    pub local_hit: bool,
    /// COOK-276: explicit shared-tier answer. `None` when the unit never
    /// consults the shared tier (`local` sharing, or no key computable).
    pub shared_present: Option<bool>,
    pub determinants: UnitDeterminants,
    pub line: u32,
    /// On a shared-tier miss: Some(diffs) if a producer manifest was found
    /// (empty ⇒ determinants identical to ours), None if no manifest exists
    /// for K.
    pub manifest_diff: Option<Vec<DeterminantDiff>>,
    /// CS-0174: on a local-tier miss, which determinant changed since this
    /// unit's previous fingerprint record — the same attribution the executor
    /// prints as `rebuild (input changed: <path>)`. `None` on a hit, or on a
    /// cold unit with no previous record to differ from.
    ///
    /// This is the local-tier counterpart of `manifest_diff`: that names why
    /// the SHARED store could not serve this key, this names why the LOCAL
    /// index could not.
    pub local_cause: Option<String>,
    /// CS-0245 / §17.1.6.1: `Some` ONLY when `local_cause` came from
    /// `RebuildReason::SealChanged` — the local miss the executor would render
    /// as `rebuild (seal changed)`. `None` for every other cause (including a
    /// hit or a cold unit), so a renderer can tell "this is a seal-changed
    /// miss" from the presence of the option alone, without re-matching
    /// `local_cause`'s rendered text (`cause_summary()`'s string is not a
    /// second source of truth for this — see the constitution's
    /// duplicate-literals rule). Each entry names a sealed key and how its
    /// value moved (§{cat.probes.exec}'s local per-probe record vs the value
    /// this query just resolved).
    ///
    /// Can be `Some(vec![])` even on a genuine seal-changed miss (§17.1.6.1's
    /// I5): the local per-probe record moves whenever a probe is REACHED,
    /// while the unit's index record moves only when the unit last EXECUTED —
    /// two different events. When the record cannot account for the
    /// divergence, this is `Some(vec![])` and the renderer MUST say "the seal
    /// set diverged, no entry nameable" rather than presenting an empty diff
    /// as the full account.
    pub seal_deltas: Option<Vec<(String, cook_contracts::probe_value::ProbeDelta)>>,
}

#[derive(Debug, Clone)]
pub struct WhyReport {
    pub recipe: String,
    pub units: Vec<WhyUnit>,
}

/// Build a read-only determinant explanation for `target`'s reachable closure.
/// Executes nothing. `probes_dir` is `<workspace>/.cook/probes`; sealed probe
/// values are read through from there (materialised on a prior run), except
/// where this function's own CS-0245 fresh-resolution pass (below) has
/// already inserted a value THIS query just observed.
///
/// `probe_snapshot` is the RAW `.cook/probes/` directory content (file name ->
/// bytes) taken by the caller BEFORE registration ran — registration can
/// overwrite a probe's own record file during this same invocation, so a
/// snapshot taken any later would have already lost the value CS-0245's
/// local-miss attribution needs as "prior".
#[allow(clippy::too_many_arguments)]
pub fn explain(
    target: &str,
    registered_workspace: &RegisteredWorkspace,
    edges: &BTreeMap<String, Vec<String>>,
    reachable: &BTreeSet<String>,
    cache_ctx: &CacheContext,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    probes_dir: &Path,
    project_root: &Path,
    probe_snapshot: &BTreeMap<String, Vec<u8>>,
) -> Result<WhyReport, crate::EngineError> {
    let topo = cook_contracts::unit_graph::toposort_recipes(edges, reachable)
        .map_err(crate::EngineError::from)?;
    let mut all_units = Vec::with_capacity(topo.len());
    for name in &topo {
        let units = registered_workspace
            .units_by_recipe
            .get(name)
            .ok_or_else(|| crate::EngineError::UnknownRecipe(name.clone()))?;
        let mut u = units.clone();
        if let Some(deps) = edges.get(name) {
            // Same declared-requires filter as `run_inner` (COOK-402): the
            // closure map merges `orders` in, and stamping those as coarse
            // deps would give this explanation DAG whole-recipe barriers the
            // scheduler never imposes.
            u.deps = cook_contracts::unit_graph::declared_coarse_deps(&units.deps, deps);
        }
        all_units.push(u);
    }
    let dag = dag_builder::build_dag(all_units)?;

    // The set of qualified probe keys some unit in the QUERIED closure
    // actually seals. The fresh-resolution pass below is scoped to this set.
    // This scoping is NOT what stops one member's probe being attributed to
    // another — the qualified keys the pass carries do that on their own,
    // proven by mutation: with qualification in place, removing this guard
    // changes no byte of output. What the guard actually buys is two things,
    // both real: §17.1.6.1 states the freshness obligation over "a sealed
    // probe" of "every cacheable unit in the closure"
    // (standard/src/content/docs/17-cache.mdx, "Determinant reporting"), so
    // resolving outside the closure is simply out of scope for the query;
    // and skipping the rest of the workspace also skips the cost —
    // re-globbing every member's declared `files` set on every `cook why`
    // would price the whole workspace instead of the queried closure.
    // `WorkNode::recipe_name` is already workspace-qualified (unlike
    // `CacheMeta::recipe_name`, which stays Cookfile-local by design,
    // §20.2.3), so it is the right base for `qualify_for_recipe` here too.
    let mut sealed_in_closure: BTreeSet<cook_contracts::probe_key::QualifiedProbeKey> =
        BTreeSet::new();
    for idx in 0..dag.len() {
        let node = dag.node(idx).payload();
        let Some(meta) = &node.cache_meta else {
            continue;
        };
        for k in &meta.seal_keys {
            sealed_in_closure
                .insert(cook_contracts::probe_key::qualify_for_recipe(&node.recipe_name, k));
        }
    }

    // CS-0245 / §17.1.6.1 case (a): re-resolve every probe whose value can be
    // observed with no VM — a top-level `files`/`tools` declaration,
    // synthesised fresh from the current tree — so the key this report names
    // is the key a run would compute, not what a PRIOR invocation last wrote
    // to `.cook/probes/<key>.json`. `fresh_values` is keyed by `matched_key`
    // (the workspace-QUALIFIED map key, CS-0242 — see the note below on why
    // that is the only key any of this pass's collections may carry) and is
    // folded into each unit's own `ProbeValueStore` in the
    // classification loop below, shadowing exactly the file its disk
    // fallback would otherwise read, so `seal.rs`'s fold picks the fresh
    // bytes up with no change to `seal.rs` itself.
    //
    // `fresh_resolved` and `probe_lookup_failures` feed §17.1.6.1's
    // provenance obligation (`UnitDeterminants::prior_invocation_probes` /
    // `::probe_lookup_failures`, populated per-unit below): a sealed key is
    // case (a) — fresh here — case (b) — this invocation's own registration
    // pre-pass produced it, `registered_workspace.resolved_probe_keys` — or
    // else case (c), reported from a prior invocation.
    let mut fresh_values:
        BTreeMap<cook_contracts::probe_key::QualifiedProbeKey, Vec<u8>> = BTreeMap::new();
    let mut fresh_resolved:
        BTreeSet<cook_contracts::probe_key::QualifiedProbeKey> = BTreeSet::new();
    let mut probe_lookup_failures:
        BTreeMap<cook_contracts::probe_key::QualifiedProbeKey, String> = BTreeMap::new();
    for (matched_key, pu) in &registered_workspace.probes {
        if !sealed_in_closure.contains(matched_key) {
            continue;
        }
        // COOK-510: derive the declaring working dir from the qualified map
        // key, not the local `ProbeUnit.key`.
        let prefix = matched_key.import_prefix();
        let working_dir = registered_workspace
            .working_dir_by_prefix
            .get(prefix)
            .or_else(|| registered_workspace.working_dir_by_prefix.get(""))
            .cloned()
            .unwrap_or_else(|| project_root.to_path_buf());
        let ctx = cook_probe::eval::EvalCtx {
            working_dir: &working_dir,
            project_root: Some(project_root),
            declaring_prefix: matched_key.import_prefix(),
        };
        // `prepass_resolved: false` is mandatory (COOK-526's cross-phase
        // single-flight arm): `true` would make `lookup` read the record file
        // back, which is the stale value this whole pass exists to stop
        // trusting. With `false`, only a `files`/`tools` sentinel resolves
        // here (`Some((bytes, ValueSource::Produced))`); anything needing a
        // `produce` body answers `None` and is left exactly as `probe_store`
        // would otherwise have answered — its last materialised value via the
        // disk fallback, including a value THIS invocation's own registration
        // pre-pass just produced (case b, already on disk before this loop
        // runs). Never calls `evaluate`, never constructs a `ProduceRunner`,
        // never calls `record`: this query writes nothing (§17.1.6).
        match cook_probe::eval::lookup(pu, &ctx, false) {
            Ok(cook_probe::eval::Lookup {
                resolved: Some((bytes, cook_probe::eval::ValueSource::Produced)),
                ..
            }) => {
                fresh_values.insert(matched_key.clone(), bytes);
                fresh_resolved.insert(matched_key.clone());
            }
            Ok(_) => {
                // Needs a `produce` body (or, unreachably with
                // `prepass_resolved: false`, a prepass serve) — leave the key
                // on whatever the per-unit `probe_store` answers.
            }
            Err(e) => {
                // A `tools` name gone from PATH (CS-0214) or an unreadable
                // `files` match (CS-0241) MUST NOT fail `cook why` — it falls
                // back to the last materialised value, and the failure text
                // is carried for the renderer.
                probe_lookup_failures.insert(matched_key.clone(), e.message);
            }
        }
    }

    let mut units = Vec::new();
    // CS-0173: what each already-classified unit will leave on disk, threaded
    // forward so a consumer is classified against its producer's answer rather
    // than against the working tree's current state.
    //
    // Iterating node indices IS a topological order: `Dag::add_node` takes the
    // indices a node depends on, which must already exist, so every producer
    // has a lower index than its consumers. Classifying in this order means a
    // consumer's producers are always resolved before it is reached.
    let mut predictions: Predictions = BTreeMap::new();
    // Hoisted out of the loop below: whether the probes directory exists
    // cannot change over the course of one report.
    let probes_dir_exists = probes_dir.exists();
    for idx in 0..dag.len() {
        let node = dag.node(idx).payload();
        let Some(meta) = &node.cache_meta else {
            continue;
        };
        // A `ProbeValueStore` view scoped to this unit's recipe. Readers keep
        // using the local keys in `meta.seal_keys`; the view qualifies each
        // lookup for the shared store and `.cook/probes/<qualified-key>.json`
        // record. Thus imported recipes may share a local key without either
        // their fresh values or prior-invocation records colliding (CS-0250).
        let probe_store = cook_probe::store::ProbeValueStore::new().for_recipe(&node.recipe_name);
        if probes_dir_exists {
            probe_store.attach_dir(probes_dir.to_path_buf());
        }
        for k in &meta.seal_keys {
            if let Some(bytes) = fresh_values
                .get(&cook_contracts::probe_key::qualify_for_recipe(&node.recipe_name, k))
            {
                probe_store.insert(k, bytes.clone());
            }
        }
        // COOK-350: an output-less unit is reported like any other. The guard
        // that stood here skipped every one of them, so a whole unit kind was
        // invisible to the transparency surface: `cook why --level unit`
        // printed a test's body text and nothing else, and the header counted
        // `0 hit, 0 rebuild` for units demonstrably being served from cache.
        // The answer to "why did my test re-run" was to go and instrument it.
        //
        // It skipped them because before CS-0186 an empty output list meant
        // "not in the step index" — true then, since a separate store answered
        // for tests, and false now. Nothing below needs an output: the key is
        // over determinants and input contents, and §17.1.1.1 gives an
        // output-less unit an identity of its own.
        //
        // The two other empty-output guards in this file and in verify.rs stay.
        // They mean "nothing in the CAS to publish", which remains correct.
        let det = resolve_unit_determinants(
            node,
            meta,
            &probe_store,
            &predictions,
            &fresh_resolved,
            &registered_workspace.resolved_probe_keys,
            &probe_lookup_failures,
        );
        // A unit waiting on input bytes, or on a sealed probe value that this
        // read-only query cannot produce, has no computable key.
        // Report the cause and, deliberately, no key: a key over the stale or
        // absent bytes would be a number that matches nothing and means nothing.
        let pending_input = det
            .pending_inputs
            .iter()
            .next()
            .map(|(path, producer)| (path.clone(), producer.clone()));
        let unmaterialised_probe = det
            .sealed_probes
            .iter()
            .find(|(_, value)| value.is_empty())
            .map(|(probe, _)| probe.clone());
        let (key_hex, c) = match (pending_input, unmaterialised_probe) {
            (Some((path, producer)), _) => (
                String::new(),
                Classification {
                    status: CacheStatus::ForcedByUpstream {
                        producer,
                        path,
                    },
                    local_hit: false,
                    local_cause: None,
                    seal_deltas: None,
                    shared_present: None,
                    manifest_diff: None,
                    shared_output_hashes: BTreeMap::new(),
                },
            ),
            (None, Some(key)) => (
                String::new(),
                Classification {
                    status: CacheStatus::UnmaterialisedProbe { key },
                    local_hit: false,
                    local_cause: None,
                    seal_deltas: None,
                    shared_present: None,
                    manifest_diff: None,
                    shared_output_hashes: BTreeMap::new(),
                },
            ),
            (None, None) => {
                let key_hex = unit_key_hex(meta, &det);
                let c = classify(
                    node,
                    meta,
                    cache_ctx,
                    cache_managers,
                    &det,
                    &key_hex,
                    &predictions,
                    probe_snapshot,
                    &probe_store,
                );
                (key_hex, c)
            }
        };
        record_predictions(&mut predictions, node, meta, &c, cache_managers);
        let disposition = match meta.sharing {
            cook_contracts::Sharing::Local => Disposition::Local,
            cook_contracts::Sharing::Pinned => Disposition::Pinned,
            cook_contracts::Sharing::Shared => Disposition::Unannotated,
        };
        units.push(WhyUnit {
            // NOTE: intentionally `node.recipe_name` (the workspace-qualified
            // key the unit was found under in `units_by_recipe`/`RecipeUnits`,
            // e.g. `api.compile`), NOT `meta.recipe_name` — `CacheMeta`'s copy
            // stays the Cookfile-LOCAL declaration name by design (§20.2.3;
            // it feeds the StepEntry index name and cache-key namespace, so
            // must never be repointed at the qualified name). Using the local
            // name here would misattribute an imported dependency's unit to
            // its bare local name (or, when it collides with the querying
            // recipe's own name, to the wrong recipe entirely).
            recipe_name: node.recipe_name.clone(),
            cache_key: meta.cache_key.clone(),
            key_hex,
            disposition,
            status: c.status,
            local_hit: c.local_hit,
            local_cause: c.local_cause,
            seal_deltas: c.seal_deltas,
            shared_present: c.shared_present,
            determinants: det,
            line: node_line(node),
            manifest_diff: c.manifest_diff,
        });
    }
    Ok(WhyReport {
        recipe: target.to_string(),
        units,
    })
}

fn node_line(node: &WorkNode) -> u32 {
    // CS-0191: one accessor, every payload. This enumerated three variants and
    // silently reported 0 for the other two, so `cook why` on a `>{ }` body or
    // an interactive step cited no line at all.
    node.payload.as_ref().map(|p| p.line()).unwrap_or(0) as u32
}

#[allow(clippy::too_many_arguments)]
fn resolve_unit_determinants(
    node: &WorkNode,
    meta: &cook_contracts::CacheMeta,
    probe_store: &cook_probe::store::ProbeValueStore,
    predictions: &Predictions,
    fresh_resolved: &BTreeSet<cook_contracts::probe_key::QualifiedProbeKey>,
    resolved_probe_keys: &BTreeSet<cook_contracts::probe_key::QualifiedProbeKey>,
    lookup_failures: &BTreeMap<cook_contracts::probe_key::QualifiedProbeKey, String>,
) -> UnitDeterminants {
    let mut inputs = BTreeMap::new();
    let mut pending_inputs = BTreeMap::new();
    // CS-0186: the same resolution the cache performs, so `cook why` reports
    // the set the unit is actually keyed on rather than the raw declaration.
    // A declared pattern whose producer has not run resolves to nothing here —
    // `cook why` is read-only and does not build — which is honest: that is
    // what the cache would compute at this moment too. A literal input a unit
    // in this closure produces is answered by `predictions` below, unchanged.
    let declared =
        cook_cache::resolve_declared_inputs(&meta.inputs, &meta.consumes, &node.working_dir);
    for p in &declared {
        let abs = node.working_dir.join(p);
        // CS-0173: an input that some unit in this closure produces is answered
        // by that producer, never by whatever is on disk right now. Reading disk
        // is wrong in both directions: on a cold tree the file is absent though
        // its bytes are already determined by an upstream hit, and after an edit
        // it is present but stale, holding bytes the run will overwrite before
        // this unit ever reads them.
        match predictions.get(&abs) {
            Some((Prediction::Known(h), _)) => {
                inputs.insert(p.clone(), *h);
                continue;
            }
            Some((Prediction::Unknowable, producer)) => {
                pending_inputs.insert(p.clone(), producer.clone());
                continue;
            }
            None => {}
        }
        let h = cook_cache::hash_file(&abs).unwrap_or(0);
        inputs.insert(p.clone(), h);
    }
    let seal_contribution = crate::seal::seal_contribution(&meta.seal_keys, probe_store);
    // C2: share the producer's sealed-probe resolution (absent → empty string)
    // so a shared-miss diff doesn't falsely report a probe difference against a
    // manifest that persisted the empty-string encoding.
    let sealed_probes = crate::seal::resolve_sealed_probes(&meta.seal_keys, probe_store);
    // CS-0245 / §17.1.6.1's provenance obligation, scoped to this unit's
    // effective seal set: a key is case (a) if `fresh_resolved` produced it
    // just now, case (b) if this invocation's own registration pre-pass
    // resolved it (`resolved_probe_keys`), else case (c) — reported from a
    // prior invocation, and so belongs here.
    //
    // `qualify_for_recipe` is the one local→qualified join all three
    // collections below use.
    let prior_invocation_probes: BTreeSet<String> = meta
        .seal_keys
        .iter()
        .filter(|k| {
            let qualified = cook_contracts::probe_key::qualify_for_recipe(&node.recipe_name, k);
            !fresh_resolved.contains(&qualified) && !resolved_probe_keys.contains(&qualified)
        })
        .map(ToString::to_string)
        .collect();
    let probe_lookup_failures: BTreeMap<String, String> = meta
        .seal_keys
        .iter()
        .filter_map(|k| {
            let qualified = cook_contracts::probe_key::qualify_for_recipe(&node.recipe_name, k);
            lookup_failures.get(&qualified).map(|m| (k.to_string(), m.clone()))
        })
        .collect();
    UnitDeterminants {
        command_hash: meta.command_hash,
        env_contribution: meta.env_contribution,
        seal_contribution,
        inputs,
        output_paths: meta.output_paths.clone(),
        consulted_env: meta.consulted_env.clone(),
        sealed_probes,
        pending_inputs,
        prior_invocation_probes,
        probe_lookup_failures,
    }
}

fn unit_key_hex(meta: &cook_contracts::CacheMeta, det: &UnitDeterminants) -> String {
    let mut sorted: Vec<u64> = det.inputs.values().copied().collect();
    sorted.sort();
    let recipe_namespace = cook_cache::recipe_namespace(
        &meta.project_id,
        &meta.cookfile_path,
        &meta.recipe_name,
    );
    let k = cook_cache::cloud_key(&cook_cache::CloudKeyInputs {
        schema_version: crate::executor::cache_version(),
        recipe_namespace: &recipe_namespace,
        command_hash: det.command_hash,
        env_contribution: det.env_contribution,
        seal_contribution: det.seal_contribution,
        sorted_input_content_hashes: &sorted,
    });
    hex::encode(k)
}

/// Both cache tiers, answered independently (COOK-276), plus the legacy
/// single-status classification derived from them.
struct Classification {
    status: CacheStatus,
    local_hit: bool,
    /// CS-0174: on a local miss, the determinant that changed since this unit's
    /// previous fingerprint record. `None` on a hit or a cold unit.
    local_cause: Option<String>,
    /// CS-0245: `Some` only when `local_cause` came from
    /// `RebuildReason::SealChanged`. See `WhyUnit::seal_deltas`.
    seal_deltas: Option<Vec<(String, cook_contracts::probe_value::ProbeDelta)>>,
    /// `None` when the shared tier is not consulted (`local` sharing or no key).
    shared_present: Option<bool>,
    manifest_diff: Option<Vec<DeterminantDiff>>,
    /// CS-0173: content hash of each artifact the shared-tier probe drained,
    /// by the output path it was keyed under. The probe already streams every
    /// byte for CS-0054 verification, so hashing costs nothing beyond the read
    /// and yields exactly what a downstream `hash_file` would compute once the
    /// artifact is restored. Empty when the shared tier was not consulted.
    shared_output_hashes: BTreeMap<String, u64>,
}

#[allow(clippy::too_many_arguments)]
fn classify(
    node: &WorkNode,
    meta: &cook_contracts::CacheMeta,
    cache_ctx: &CacheContext,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    det: &UnitDeterminants,
    key_hex: &str,
    predictions: &Predictions,
    probe_snapshot: &BTreeMap<String, Vec<u8>>,
    probe_store: &cook_probe::store::ProbeValueStore,
) -> Classification {
    // I1: a declared input absent on disk means the unit cannot be a clean hit
    // and no real key exists (mirrors `hash_input_paths` returning None at
    // executor.rs:710). Attribute the miss to that input rather than fabricating
    // a `0`-hash key that can never match a real manifest.
    if let Some(p) = first_missing_input(meta, &node.working_dir, predictions) {
        return Classification {
            status: CacheStatus::MissingInput { path: p },
            local_hit: false,
            local_cause: None,
            seal_deltas: None,
            shared_present: None,
            manifest_diff: None,
            shared_output_hashes: BTreeMap::new(),
        };
    }
    let (local_hit, local_cause, seal_deltas) =
        local_step_hit(node, meta, det, cache_managers, probe_snapshot, probe_store);
    if meta.sharing.is_local() {
        return Classification {
            status: if local_hit { CacheStatus::LocalHit } else { CacheStatus::LocalOnlyMiss },
            local_hit,
            local_cause,
            seal_deltas,
            shared_present: None,
            manifest_diff: None,
            shared_output_hashes: BTreeMap::new(),
        };
    }
    // C1: read-only shared-store probe — recompute artifact keys and confirm the
    // backend holds every output, but NEVER write to the working tree (unlike
    // `fetch_by_key`, which restores). `cook why` is strictly read-only.
    // COOK-276: probed even on a local hit, so both tiers get an explicit
    // answer (`[HIT (local), MISS (shared)]` instead of a bare tier label
    // that reads as "will rebuild").
    // An OBSERVING unit has no artifacts, so `shared_artifacts_present` below
    // is the wrong probe for it: `fetch_by_key` refuses an empty output list at
    // its own door and could never serve one. Its shared hit is a REPLAY of a
    // recorded observation, which is a different query, and it is the same
    // query the executor asks.
    //
    // COOK-401: this used to return `shared_present: None` and say "the
    // question does not apply". That reasoning was CS-0186's and was true when
    // written; CS-0189 taught the executor to serve observing units from a
    // recorded observation and did not revisit it, so `cook why` went silent
    // about a tier that would in fact serve the unit, on exactly the unit kind
    // CS-0189 had just taught to use it. `shared_observation` is now the one
    // implementation, and both sides call it.
    use cook_contracts::cache::record::{effect_kind, EffectKind};
    if effect_kind(meta) == EffectKind::Observed {
        let shared = decode_key_hex(key_hex)
            .and_then(|k| {
                cook_cache::shared_observation(cache_ctx.backend.as_ref(), &k)
            })
            .is_some();
        return Classification {
            status: if local_hit {
                CacheStatus::LocalHit
            } else if shared {
                CacheStatus::SharedHit
            } else {
                CacheStatus::SharedMiss
            },
            local_hit,
            local_cause,
            seal_deltas,
            shared_present: Some(shared),
            // An observing unit publishes no artifact list, so a
            // producer-shaped manifest diff would be noise either way.
            manifest_diff: None,
            shared_output_hashes: BTreeMap::new(),
        };
    }
    let probed = shared_artifacts_present(cache_ctx, key_hex, meta);
    let shared = probed.is_some();
    let shared_output_hashes = probed.unwrap_or_default();
    let manifest_diff = if shared { None } else { manifest_diff(cache_ctx, key_hex, det) };
    let status = if local_hit {
        CacheStatus::LocalHit
    } else if shared {
        CacheStatus::SharedHit
    } else if meta.sharing.is_pinned() {
        CacheStatus::PinnedColdMiss
    } else {
        CacheStatus::SharedMiss
    };
    Classification {
        status,
        local_hit,
        local_cause,
        seal_deltas,
        shared_present: Some(shared),
        manifest_diff,
        shared_output_hashes,
    }
}

fn first_missing_input(
    meta: &cook_contracts::CacheMeta,
    working_dir: &Path,
    predictions: &Predictions,
) -> Option<String> {
    // The declared entries, patterns included: a pattern that expands to
    // nothing IS a missing input for this report's purposes, and it is reported
    // as the author wrote it rather than as an empty expansion nobody can act
    // on.
    meta.inputs
        .iter()
        .map(|e| &e.path)
        .find(|p| {
            let abs = working_dir.join(p);
            // CS-0173: an input a producer in this closure will restore is not
            // missing, it is merely not here yet. Only a path nothing produces
            // is genuinely absent. (A path whose producer REBUILDS never reaches
            // this check: it is `ForcedByUpstream` before classify is called.)
            if predictions.contains_key(&abs) {
                return false;
            }
            cook_cache::hash_file(&abs).is_none()
        })
        .cloned()
}

/// CS-0173: record what this unit leaves on disk, for the consumers that follow
/// it in topological order.
///
/// A unit served from cache has determined outputs; one that rebuilds does not,
/// and saying so is the whole point. The fallbacks are ordered by how directly
/// each knows the bytes, and the final arm refuses to guess: an output we could
/// not learn the hash of is `Unknowable`, which costs a downstream unit its key
/// but never gives it a wrong one.
fn record_predictions(
    predictions: &mut Predictions,
    node: &WorkNode,
    meta: &cook_contracts::CacheMeta,
    c: &Classification,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
) {
    let served = c.local_hit || c.shared_present == Some(true);
    for p in &meta.output_paths {
        // A glob output is a pattern, not a path; it names no file a consumer
        // could declare as an input, so there is nothing to predict.
        if cook_cache::is_terminal_output(p) {
            continue;
        }
        let abs = node.working_dir.join(p);
        let prediction = if !served {
            Prediction::Unknowable
        } else if let Some(h) = c.shared_output_hashes.get(p) {
            Prediction::Known(*h)
        } else if let Some(h) = local_output_hash(node, meta, p, cache_managers) {
            Prediction::Known(h)
        } else if let Some(h) = cook_cache::hash_file(&abs) {
            Prediction::Known(h)
        } else {
            Prediction::Unknowable
        };
        predictions.insert(abs, (prediction, node.recipe_name.clone()));
    }
}

/// The content hash the local index recorded for one of this unit's outputs.
/// Only meaningful on a local hit, where `needs_rebuild_cook` has already
/// confirmed the recorded outputs still match what is on disk.
fn local_output_hash(
    node: &WorkNode,
    meta: &cook_contracts::CacheMeta,
    output_path: &str,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
) -> Option<u64> {
    let cm = cache_managers.get(&node.recipe_name)?;
    let cache = cm.get_or_load(&meta.recipe_name);
    let entry = cache.steps.get(&meta.cache_key)?;
    entry
        .outputs
        .iter()
        .find(|f| &*f.path == output_path)
        .map(|f| f.hash)
}

/// Read-only shared-store probe: recompute the artifact keys and check the
/// backend has every output, draining each reader for integrity verification
/// (CS-0054) but NEVER writing to the working tree. `cook why` is read-only.
/// CS-0173: returns `Some(path → content hash)` when every artifact is present,
/// `None` on the first absent or unreadable one. The hashes are a by-product of
/// the drain that already had to happen: they let a consumer of these outputs be
/// classified before anything is restored.
fn shared_artifacts_present(
    cache_ctx: &CacheContext,
    key_hex: &str,
    meta: &cook_contracts::CacheMeta,
) -> Option<BTreeMap<String, u64>> {
    let cloud_k = decode_key_hex(key_hex)?;
    if meta.output_paths.is_empty() {
        return None;
    }
    // COOK-278: for glob-output units the declared paths are raw patterns, not
    // the concrete names the publish path keyed its artifacts under — probing
    // them reports a false SharedMiss. When the key's determinant manifest is
    // present, probe its recorded concrete output list (files, implicit
    // depfile, empty dirs — publish index order) instead.
    let manifest_outputs: Option<Vec<String>> = cache_ctx
        .backend
        .get_manifest(&cloud_k)
        .ok()
        .flatten()
        .map(|m| {
            let mut list = m.output_paths;
            if let Some(di) = &meta.discovered_inputs {
                list.push(di.from.clone());
            }
            list.extend(m.empty_dir_outputs);
            list
        });
    let probe_paths: &[String] = match &manifest_outputs {
        Some(list) => list,
        None => &meta.output_paths,
    };
    let mut hashes = BTreeMap::new();
    for (idx, path) in probe_paths.iter().enumerate() {
        let artifact_k = cook_cache::artifact_key(&cloud_k, idx as u32, path);
        match cache_ctx.backend.get(&artifact_k) {
            Ok(Some(mut reader)) => {
                // Drain to trigger streaming verify-on-restore. CS-0173 keeps
                // the digest instead of discarding it: the bytes were read
                // either way, and `hash_reader` is `hash_file`'s streaming twin,
                // so this is exactly the hash a consumer would compute from the
                // restored file.
                let h = cook_cache::hash_reader(&mut reader)?;
                hashes.insert(path.clone(), h);
            }
            _ => return None,
        }
    }
    Some(hashes)
}

/// Decode a 64-char lowercase-hex string into a 32-byte cloud key. Returns None
/// on any length or non-hex error. C2: uses `hex::decode` (no hand-rolled hex).
fn decode_key_hex(key_hex: &str) -> Option<[u8; 32]> {
    hex::decode(key_hex).ok()?.try_into().ok()
}

/// CS-0174: the local-tier verdict AND, on a miss, the determinant that moved.
///
/// `needs_rebuild_cook` already computes the attribution — it is the same call
/// and the same `RebuildReason` the executor renders as `rebuild (input
/// changed: <path>)`. Reducing it to a boolean here was why a local miss listed
/// every determinant and named none, while a shared miss got a manifest diff:
/// the explain tool was strictly less informative than the build log it exists
/// to pre-empt.
///
/// `None` means no attribution is available, not that nothing changed: a unit
/// with no cache manager, no prior entry, or a `NoCacheEntry` verdict is cold,
/// and there is no previous record to have diverged from.
///
/// CS-0245: the third element is `Some(per-key seal delta)` ONLY when the
/// rebuild reason is `RebuildReason::SealChanged` — matched on the variant
/// itself, never on `cause_summary()`'s string, so a rendering change to that
/// string can never silently stop or start this attribution, and so a
/// renderer downstream can branch on `Some`/`None` instead of re-matching the
/// string itself (the constitution's duplicate-literals rule catches exactly
/// that copy). Each entry's "prior" bytes come from `probe_snapshot` (taken
/// before this invocation's own registration could overwrite a record file)
/// and "current" from `probe_store` (already carrying this query's CS-0245
/// fresh re-resolution). The inner `Vec` can be empty on a genuine
/// seal-changed miss — §17.1.6.1's I5: the local per-probe record moves on
/// every REACH, the index's seal digest only on the unit's last EXECUTION,
/// two different events — and an empty result here is exactly that case, not
/// a bug.
fn local_step_hit(
    node: &WorkNode,
    meta: &cook_contracts::CacheMeta,
    det: &UnitDeterminants,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    probe_snapshot: &BTreeMap<String, Vec<u8>>,
    probe_store: &cook_probe::store::ProbeValueStore,
) -> (bool, Option<String>, Option<Vec<(String, cook_contracts::probe_value::ProbeDelta)>>) {
    let Some(cm) = cache_managers.get(&node.recipe_name) else {
        return (false, None, None);
    };
    let cache = cm.get_or_load(&meta.recipe_name);
    let Some(entry) = cache.steps.get(&meta.cache_key) else {
        return (false, None, None);
    };
    // Resolved by the same call `check_node_cache` makes, so the query judges
    // the unit against the set the build would (§17.1.1.2).
    let resolved_inputs = cook_cache::resolve_declared_inputs(
        &meta.inputs,
        &meta.consumes,
        &node.working_dir,
    );
    let input_refs: Vec<&str> = resolved_inputs.iter().map(|s| s.as_str()).collect();
    // I2: for glob outputs the raw pattern strings don't exist on disk; passing
    // them to needs_rebuild_cook would trip OutputMissing → spurious miss. Mirror
    // check_node_cache (executor.rs:654-664) by substituting the StepEntry's
    // recorded concrete output paths when any declared output is a glob.
    let any_glob = meta.output_paths.iter().any(|s| cook_cache::is_terminal_output(s));
    let current_outputs_storage: Vec<String> = if any_glob {
        entry.outputs.iter().map(|f| f.path.to_string()).collect()
    } else {
        meta.output_paths.clone()
    };
    let outs: Vec<&str> = current_outputs_storage.iter().map(|s| s.as_str()).collect();
    let (result, _updated) = cook_cache::needs_rebuild_cook(
        Some(entry),
        &input_refs,
        &outs,
        det.command_hash,
        det.env_contribution,
        det.seal_contribution,
        &node.working_dir,
        None,
        meta.discovered_inputs.as_ref(),
        meta.record,
    );
    match result {
        cook_cache::RebuildResult::Skip => (true, None, None),
        cook_cache::RebuildResult::Rebuild(reason) => {
            let seal_deltas = if matches!(reason, cook_cache::RebuildReason::SealChanged) {
                Some(seal_deltas_for(&node.recipe_name, meta, probe_snapshot, probe_store))
            } else {
                None
            };
            let cause = reason.cause_summary();
            (false, cause, seal_deltas)
        }
    }
}

/// CS-0245: per-key delta for every sealed key this query can say anything
/// about. A key absent from `probe_snapshot` reports `FirstObservation`
/// (never a diff against a fabricated old value, per §17.1.6.1); a key with
/// no current value in `probe_store` at all, or whose prior and current bytes
/// are byte-identical, contributes nothing. A unit whose whole result is
/// empty is §17.1.6.1's I5 case — the seal set diverged but no entry can be
/// named — not a bug in this filter.
fn seal_deltas_for(
    recipe_name: &str,
    meta: &cook_contracts::CacheMeta,
    probe_snapshot: &BTreeMap<String, Vec<u8>>,
    probe_store: &cook_probe::store::ProbeValueStore,
) -> Vec<(String, cook_contracts::probe_value::ProbeDelta)> {
    meta.seal_keys
        .iter()
        .filter_map(|key| {
            let qualified = cook_contracts::probe_key::qualify_for_recipe(recipe_name, key);
            let prior = probe_snapshot
                .get(&cook_contracts::probe_value::probe_file_name(qualified.as_ref()))
                .map(|v| v.as_slice());
            let current = probe_store.get(key)?;
            cook_contracts::probe_value::probe_delta(prior, &current)
                .map(|delta| (key.to_string(), delta))
        })
        .collect()
}

fn manifest_diff(
    cache_ctx: &CacheContext,
    key_hex: &str,
    det: &UnitDeterminants,
) -> Option<Vec<DeterminantDiff>> {
    let k = decode_key_hex(key_hex)?;
    let manifest = cache_ctx.backend.get_manifest(&k).ok().flatten()?;
    Some(diff_against_manifest(det, &manifest))
}

#[cfg(test)]
#[path = "tests/why_tests.rs"]
mod tests;
