//! Build a `Dag<WorkNode>` from a topologically-sorted list of `RecipeUnits`.
//!
//! Within-recipe wiring:
//! - `DepKind::Sequential` units depend on the current barrier (the set of
//!   nodes that must finish before the next sequential unit can start).
//! - `DepKind::StepGroup(idx)` units all share the same barrier (the one
//!   active when the group started). When the last member of a group is
//!   processed, all group members become the new barrier.
//!
//! Cross-recipe wiring (coarse):
//! - A recipe's root units (those with no within-recipe deps) additionally
//!   depend on the leaf barrier of every recipe listed in `deps`.
//!
//! Cross-recipe wiring (fine-grained):
//! - For each `(unit_idx, dep_recipe_name)` in `ru.dep_edges`, that specific
//!   unit additionally depends on the terminal nodes of the named recipe.
//!
//! Leaf pass-through (empty barrier):
//! - A recipe's "leaves" (what downstream recipes depend on via `deps` /
//!   `dep_edges`) are normally its final sequential barrier. But when a
//!   recipe finishes its unit loop with an EMPTY barrier — because it has
//!   zero units, or because its only units were all demand-pruned probes
//!   (§22.5.7; an all-probe recipe's `consumed` set is seeded only from
//!   non-probe units in the same recipe, so every probe is necessarily
//!   pruned) — it forwards its own `deps`' leaves instead of registering
//!   an empty set. The rule is "empty barrier ⇒ forward", not "zero units
//!   ⇒ forward": keep both the code and this comment phrased that way so
//!   they cannot drift apart. This makes a unit-less meta-target
//!   (`recipe middle : producer` with no body) transparent to ordering
//!   instead of severing the chain — a chain of such recipes forwards the
//!   original producer's leaves transitively, by induction over the
//!   topo-ordered slice.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

use cook_contracts::{CapturedUnit, DepKind, RecipeUnits, WorkPayload};
use cook_dag::Dag;

use crate::{EngineError, WorkNode};

/// Compute the set of probe keys reached by at least one non-probe consumer
/// in `units`, transitively closing under probe-on-probe `inputs.requires`.
///
/// A probe's upstream `inputs.requires` is read from two sources, depending on
/// where the probe was registered:
///
/// * **Top-level probes** (`cook.probe(...)` called outside any recipe body)
///   live in `probes` — the [`RecipeUnits.probes`] view drained from
///   `session_state.probes` in `register_cookfile`. Their `inputs.requires`
///   lives on the [`ProbeUnit`] directly.
/// * **Body-scope probes** (`cook.probe(...)` called inside a recipe body — or
///   inside a `require`d module first-loaded during a body) are pushed onto
///   `body.units` by `install_cook_probe` as `WorkPayload::Probe` entries with
///   their `inputs.requires` mirrored onto the surrounding [`CapturedUnit.probes`]
///   field. They do NOT appear in `probes` because the registry is drained
///   once before any body runs (see `cook-register::engine::register_cookfile`).
///
/// Both indexes are consulted during the transitive closure so a body-scope
/// consumer probe can legitimately pull its body-scope upstream into
/// `consumed` (without this, demand-driven pruning silently drops the upstream
/// and the executor later reports "requires upstream X which has no
/// fingerprint" — the canonical cook_cc `needs = {…}` shape that registers
/// `cc:find:NAME → cc:linker-search-dirs` chains body-scope).
///
/// Returns probe keys in deterministic sorted order via the underlying BTreeSet.
fn compute_consumed_probe_keys(
    units: &[CapturedUnit],
    probes: &[cook_contracts::ProbeUnit],
) -> BTreeSet<String> {
    // Top-level probes (ru.probes): inputs.requires lives on the ProbeUnit.
    let top_level_probe_by_key: BTreeMap<&str, &cook_contracts::ProbeUnit> =
        probes.iter().map(|p| (p.key.as_str(), p)).collect();

    // Body-scope probes (WorkPayload::Probe entries in ru.units): their
    // inputs.requires is carried on the CapturedUnit.probes field by
    // install_cook_probe.
    let body_probe_requires_by_key: BTreeMap<&str, &[String]> = units
        .iter()
        .filter_map(|u| match &u.payload {
            WorkPayload::Probe { key, .. } => Some((key.as_str(), u.probes.as_slice())),
            _ => None,
        })
        .collect();

    // Seed: keys listed by any non-probe unit's `probes`.
    let mut consumed: BTreeSet<String> = BTreeSet::new();
    for u in units {
        if !matches!(u.payload, WorkPayload::Probe { .. }) {
            for k in &u.probes {
                consumed.insert(k.clone());
            }
        }
    }

    // Transitive close under probe-on-probe inputs.requires.
    let mut worklist: Vec<String> = consumed.iter().cloned().collect();
    while let Some(k) = worklist.pop() {
        if let Some(probe) = top_level_probe_by_key.get(k.as_str()) {
            for upstream in &probe.inputs.requires {
                if consumed.insert(upstream.clone()) {
                    worklist.push(upstream.clone());
                }
            }
        }
        if let Some(requires) = body_probe_requires_by_key.get(k.as_str()) {
            for upstream in *requires {
                if consumed.insert(upstream.clone()) {
                    worklist.push(upstream.clone());
                }
            }
        }
    }
    consumed
}

/// Build a `Dag<WorkNode>` from a topologically-sorted list of `RecipeUnits`.
///
/// **Unified-call contract (SHI-222).** The caller passes _every_ reachable
/// recipe in a single invocation; cross-recipe edges (both coarse `deps` and
/// fine-grained `dep_edges`) are resolved intra-call against the running
/// `recipe_leaves` accumulator, so the caller is responsible for passing
/// recipes in topological order. The old wave-based call-site hid this
/// property externally by passing one wave per call; the unified-DAG path
/// collapses to a single call. See `tests/unified_dag_build.rs` for the
/// integration pin.
///
/// A `dep_edges` entry that names a recipe absent from the passed slice is
/// rejected up front with [`EngineError::DanglingDepEdge`] — this channel is
/// not analyzer-validated upstream the way `deps`/`requires` is, so
/// `build_dag` is the last chance to catch it (see the pre-walk check at the
/// top of this function). A `deps`/`requires` entry naming an unknown recipe
/// is a different story: `cook_register`'s analyzer rejects it before
/// `build_dag` ever runs, so an absent `cross_deps` lookup below is already
/// unreachable on the live path and is left unchecked here.
///
/// Performs plan-time validation that no two non-dep-related recipes declare
/// the same canonical output path. If two recipes with no recipe-level
/// dependency edge between them (in either direction) both claim the same
/// `working_dir.join(output_path)`, this returns
/// [`EngineError::OutputCollision`] before any work is dispatched. This
/// prevents silent races under `--jobs > 1` where two recipes write the same
/// artifact concurrently with no enforced ordering.
///
/// Also performs demand-driven probe scheduling (§22.5.7): probe units whose
/// keys are not transitively referenced by any non-probe unit's `probes`
/// field are silently omitted from the DAG. No fingerprint is computed, no
/// diagnostic is emitted.
///
/// **Leaf pass-through.** A recipe's entry in `recipe_leaves` is normally its
/// final sequential barrier, but a recipe that ends its unit loop with an
/// EMPTY barrier forwards its own `deps`' leaves instead of registering an
/// empty set — see the "Leaf pass-through" section of the module doc comment
/// at the top of this file (including the ways a barrier ends up empty
/// despite non-empty `units`). The trigger is "empty
/// barrier", not "zero units". This keeps a unit-less meta-target
/// (`recipe middle : producer`) transparent to downstream ordering instead
/// of silently severing it.
pub fn build_dag(recipe_units: Vec<RecipeUnits>) -> Result<Dag<WorkNode>, EngineError> {
    // ── Plan-time output-collision check ─────────────────────────────────────
    // Accumulate every (canonical_output_path -> {recipe_name, ...}) pair from
    // all CacheMetas across all recipes in the wave. Two recipes that share a
    // canonical output path with no dependency path between them are racing
    // silently; reject the plan.
    if let Some(err) = detect_output_collisions(&recipe_units) {
        return Err(err);
    }

    // ── Pre-walk: dep_edges closure validation ──────────────────────────────
    // `dep_edges` (populated by `cook.dep_output` / `$<sigil>` refs) is a
    // separate, older channel from `requires`/`deps`: nothing upstream
    // validates its recipe names the way `cook_register`'s analyzer rejects
    // an unknown `requires` name before `build_dag` ever runs. Left
    // unchecked, an entry naming a recipe absent from `recipe_units`
    // silently produced no edge for that dep (see the `ru.dep_edges` lookup
    // in the unit loop below) instead of raising a diagnostic.
    //
    // This check walks the full slice up front, against the complete set of
    // recipe names present in the call — NOT against `recipe_leaves` below,
    // which is an intra-call accumulator populated only as the topo-ordered
    // slice is walked. Checking against `recipe_leaves` per-unit would
    // false-positive on any dep_edges target not yet reached in topo order;
    // checking the full name set up front avoids that entanglement and
    // correctly treats "present in the slice but zero units" (a legitimate
    // `Some(empty)` leaf set) as distinct from "absent from the slice"
    // (the genuine error).
    let known_recipe_names: BTreeSet<&str> =
        recipe_units.iter().map(|ru| ru.recipe_name.as_str()).collect();
    for ru in &recipe_units {
        for (_, dep_name) in &ru.dep_edges {
            if !known_recipe_names.contains(dep_name.as_str()) {
                return Err(EngineError::DanglingDepEdge {
                    referring_recipe: ru.recipe_name.clone(),
                    dep_name: dep_name.clone(),
                });
            }
        }
    }

    let mut dag = Dag::new();

    // Map from recipe name -> its leaf node ids: normally its final barrier,
    // but forwarded from its own `deps`' leaves when that barrier is empty
    // (see the "Leaf pass-through" section in the module doc comment above).
    let mut recipe_leaves: BTreeMap<String, Vec<usize>> = BTreeMap::new();

    for ru in &recipe_units {
        // Build a per-recipe index of probe key → unit index so we can
        // wire probe→consumer edges from CapturedUnit.probes (CS-0074 Bug 2).
        let probe_unit_index_by_key: BTreeMap<String, usize> = ru
            .units
            .iter()
            .enumerate()
            .filter_map(|(idx, u)| {
                if let WorkPayload::Probe { key, .. } = &u.payload {
                    Some((key.clone(), idx))
                } else {
                    None
                }
            })
            .collect();

        // Probe metadata index keyed by probe key. `ru.probes` is the
        // authoritative ProbeUnit list (it carries top-level register-block
        // probes that are NOT present in `ru.units` as `WorkPayload::Probe`
        // entries — see SHI-222 Phase 8). Body-scope probes appear in both,
        // and `probe_unit_index_by_key` takes precedence for those when
        // wiring consumer edges so we reuse the body-scope DAG node.
        let probe_meta_by_key: BTreeMap<&str, &cook_contracts::ProbeUnit> =
            ru.probes.iter().map(|p| (p.key.as_str(), p)).collect();

        // dag_id_by_unit_idx: populated as each unit is added; lets us resolve
        // probe-unit dag IDs when wiring CapturedUnit.probes edges.
        let mut dag_id_by_unit_idx: BTreeMap<usize, usize> = BTreeMap::new();

        // Demand-driven probe scheduling (§22.5.7): compute which probe keys
        // are transitively required by a non-probe consumer; probe units
        // whose key is not in this set are pruned from the DAG.
        let consumed = compute_consumed_probe_keys(&ru.units, &ru.probes);
        let skip_indices: BTreeSet<usize> = ru
            .units
            .iter()
            .enumerate()
            .filter_map(|(i, u)| match &u.payload {
                WorkPayload::Probe { key, .. } if !consumed.contains(key) => Some(i),
                _ => None,
            })
            .collect();

        // SHI-222 Phase 8 — pre-materialise Probe DAG nodes for consumer-
        // referenced probe keys that have no `WorkPayload::Probe` entry in
        // `ru.units`. These are top-level probes registered via
        // `cook.probe(...)` at register-block scope (e.g. through helpers like
        // `cook_cc.checks.has_header`). Their metadata lives in `ru.probes`
        // (drained from `RegisteredCookfile.probes`) but they were silently
        // dropped from the DAG before this fix, so the consumer's
        // `cook.probes.get(probe_key)` returned nil at execute time.
        //
        // We honour the same demand-driven pruning rule as body-scope probes
        // (§22.5.7): only keys present in `consumed` (the transitive closure
        // of non-probe consumer `probes` lists under probe-on-probe
        // `inputs.requires`) get a synthesised node. Synthesised nodes are
        // wired in dependency order so a probe whose `inputs.requires`
        // references another synthesised probe edges to it correctly.
        //
        // Synthesised nodes inherit the prevailing `cross_deps` (recipe-level
        // coarse deps) as their root deps so they cannot run before
        // prerequisite recipes finish. They do NOT participate in the
        // sequential `barrier` because they are inserted up-front, before
        // any `ru.units` are walked.
        //
        // Asymmetry note: unlike `ru.dep_edges` (validated by the pre-walk
        // check above), a `ru.deps` entry naming a recipe absent from
        // `recipe_leaves` is not diagnosed here. `deps` lowers from
        // `requires`, which `cook_register`'s analyzer (`build_adjacency`)
        // already rejects when it names an unknown recipe — so on the live
        // path this lookup missing is already impossible, and a redundant
        // check here would only duplicate that upstream validation.
        let mut cross_deps_for_synth: Vec<usize> = Vec::new();
        for dep_name in &ru.deps {
            if let Some(leaves) = recipe_leaves.get(dep_name) {
                cross_deps_for_synth.extend(leaves);
            }
        }

        let mut synthesised_probe_dag_ids: BTreeMap<String, usize> = BTreeMap::new();

        // Collect keys to synthesise: any consumed key that lacks a unit-
        // backed Probe payload but has metadata in `ru.probes`.
        let synth_keys: Vec<String> = consumed
            .iter()
            .filter(|k| !probe_unit_index_by_key.contains_key(k.as_str()))
            .filter(|k| probe_meta_by_key.contains_key(k.as_str()))
            .cloned()
            .collect();

        // Topologically order synth_keys by probe-on-probe `inputs.requires`
        // so an upstream probe is added before a downstream probe that wants
        // it as a dep. Kahn-style walk over the induced subgraph restricted
        // to keys actually being synthesised; cycles (unreachable in
        // practice — engine.rs validates `inputs.requires` at register time)
        // fall through and edges to the missing upstream are simply omitted.
        let synth_key_set: BTreeSet<&str> = synth_keys.iter().map(|s| s.as_str()).collect();
        let mut indegree: BTreeMap<&str, usize> = BTreeMap::new();
        for k in &synth_keys {
            indegree.insert(k.as_str(), 0);
        }
        for k in &synth_keys {
            if let Some(meta) = probe_meta_by_key.get(k.as_str()) {
                for upstream in &meta.inputs.requires {
                    if synth_key_set.contains(upstream.as_str()) {
                        *indegree.get_mut(k.as_str()).unwrap() += 1;
                    }
                }
            }
        }
        let mut queue: VecDeque<&str> = indegree
            .iter()
            .filter_map(|(k, &d)| if d == 0 { Some(*k) } else { None })
            .collect();
        let mut ordered_synth: Vec<String> = Vec::with_capacity(synth_keys.len());
        while let Some(k) = queue.pop_front() {
            ordered_synth.push(k.to_string());
            // For each other synth_key whose upstream list contains k, decrement.
            for other in &synth_keys {
                if other.as_str() == k {
                    continue;
                }
                if let Some(meta) = probe_meta_by_key.get(other.as_str()) {
                    if meta.inputs.requires.iter().any(|u| u == k) {
                        let entry = indegree.get_mut(other.as_str()).unwrap();
                        *entry = entry.saturating_sub(1);
                        if *entry == 0 {
                            queue.push_back(other.as_str());
                        }
                    }
                }
            }
        }
        // If any keys were not popped (cycle — should be unreachable),
        // append them anyway so they at least get nodes (edges to upstream
        // synthesised peers will be missing, mirroring the
        // probe-declared-after-consumer fallback below).
        for k in &synth_keys {
            if !ordered_synth.contains(k) {
                ordered_synth.push(k.clone());
            }
        }

        for key in &ordered_synth {
            let meta = probe_meta_by_key
                .get(key.as_str())
                .expect("synth_keys filtered on probe_meta_by_key membership");
            let mut deps: Vec<usize> = cross_deps_for_synth.clone();
            for upstream in &meta.inputs.requires {
                if let Some(&id) = synthesised_probe_dag_ids.get(upstream) {
                    if !deps.contains(&id) {
                        deps.push(id);
                    }
                }
                // Upstream that resolves to a body-scope probe in
                // `probe_unit_index_by_key` cannot be wired here because
                // that unit hasn't been added yet (we're pre-materialising
                // before the unit walk). Body-scope probes wiring upstream
                // to a top-level probe is the only direction we support
                // (top-level → top-level transitive), which matches the
                // typical helper-emitted probe shape. Future work: if
                // body-scope probes start declaring `requires` against
                // top-level keys, extend the unit walk to look up
                // synthesised IDs.
            }
            let work_node = WorkNode {
                payload: Some(WorkPayload::Probe {
                    key: meta.key.clone(),
                    produce: meta.produce_source.clone(),
                    line: meta.produce_line,
                }),
                recipe_name: ru.recipe_name.clone(),
                cache_meta: None,
                test_name: None,
                member: None,
                working_dir: ru.working_dir.clone(),
                env_vars: ru.env_vars.clone(),
                // A probe runs a Lua `produce` body (reads `cook.env` via the
                // full map); it spawns no shell step, so its process-env
                // subset is empty.
                process_env_vars: std::collections::BTreeMap::new(),
            };
            let dag_id = dag
                .add_node(work_node, &deps)
                .expect("synthesised probe deps originated from prior add_node calls");
            synthesised_probe_dag_ids.insert(key.clone(), dag_id);
        }

        // Collect cross-recipe dependency ids: the leaf nodes of every
        // prerequisite recipe.
        //
        // Asymmetry note: same as `cross_deps_for_synth` above — a `ru.deps`
        // entry naming a recipe absent from `recipe_leaves` is not diagnosed
        // here because `deps` is analyzer-validated upstream (unlike
        // `ru.dep_edges`, which the pre-walk check at the top of this
        // function now validates).
        let mut cross_deps: Vec<usize> = Vec::new();
        for dep_name in &ru.deps {
            if let Some(leaves) = recipe_leaves.get(dep_name) {
                cross_deps.extend(leaves);
            }
        }

        // Build a quick lookup: unit index -> which step_group it belongs to,
        // and at what position within that group.
        let mut unit_group_info: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        for (gi, group) in ru.step_groups.iter().enumerate() {
            for (pos, &unit_idx) in group.iter().enumerate() {
                unit_group_info.insert(unit_idx, (gi, pos));
            }
        }

        // Current barrier: the set of dag node ids that the next sequential
        // unit should depend on.
        let mut barrier: Vec<usize> = Vec::new();

        // Track dag node ids for each step group so we can form the barrier
        // when the group ends.
        let mut group_dag_ids: BTreeMap<usize, Vec<usize>> = BTreeMap::new();

        for (unit_idx, unit) in ru.units.iter().enumerate() {
            // Demand-driven prune (§22.5.7): probe units whose key is not
            // transitively consumed are silently omitted. The `barrier`
            // carries forward unchanged since we skip before any add_node /
            // barrier mutation; the probe→consumer edge wiring below
            // safely no-ops for these probes since their dag_ids are never
            // inserted into `dag_id_by_unit_idx`.
            //
            // Load-bearing invariant: probes are always emitted with
            // `DepKind::Sequential` by `probe_api.rs::install_cook_probe`,
            // so pruning them does not desync step-group accounting —
            // step-group bookkeeping below (group_dag_ids, barrier
            // promotion on group boundaries) only fires for the
            // `DepKind::StepGroup` variant,
            // never for probes. A future change that broadens probes'
            // `dep_kind` MUST revisit this skip to keep the accounting
            // consistent.
            if skip_indices.contains(&unit_idx) {
                continue;
            }

            // Probes are pure fact-gathering and do not participate in the
            // sequential barrier — they only depend on cross-recipe deps,
            // explicit `dep_edges`, and their own `inputs.requires` upstream
            // probes (wired below via `unit.probes`). Without this, every
            // sibling probe within a recipe ended up chained one-after-another
            // because `install_cook_probe` emits all probes with
            // `DepKind::Sequential`, and the consumer's `probes`-edges
            // already guarantee correct ordering relative to non-probe work.
            let is_probe = matches!(unit.payload, WorkPayload::Probe { .. });

            // Determine within-recipe dependencies for this unit.
            let within_deps: Vec<usize> = if is_probe {
                Vec::new()
            } else {
                match &unit.dep_kind {
                    DepKind::Sequential => barrier.clone(),
                    DepKind::StepGroup(_) => barrier.clone(),
                    // `DepKind` is `#[non_exhaustive]`; treat any future variant
                    // conservatively as a sequential barrier until the dag-builder
                    // is taught the new semantics.
                    _ => barrier.clone(),
                }
            };

            // Combine within-recipe and cross-recipe deps.
            // Coarse cross-recipe deps only apply to root units (units with no
            // within-recipe deps).
            let mut all_deps = if within_deps.is_empty() {
                cross_deps.clone()
            } else {
                within_deps
            };

            // Fine-grained dep edges: add terminal nodes of specific recipes
            // for this exact unit, regardless of whether it has within-recipe deps.
            for (dep_unit_idx, dep_recipe_name) in &ru.dep_edges {
                if *dep_unit_idx == unit_idx {
                    if let Some(terminal_nodes) = recipe_leaves.get(dep_recipe_name) {
                        all_deps.extend(terminal_nodes);
                    }
                }
            }

            // Probe→consumer edges from CapturedUnit.probes (CS-0074 Bug 2).
            // For each probe key in unit.probes, find the probe's dag_id (which
            // must already be known since probes appear before consumers) and add it
            // as a dependency of this unit.
            //
            // Resolution order (SHI-222 Phase 8):
            //   1. Body-scope probe units (`probe_unit_index_by_key` →
            //      `dag_id_by_unit_idx`). These were captured as
            //      `WorkPayload::Probe` entries inside `ru.units`.
            //   2. Synthesised top-level probes (`synthesised_probe_dag_ids`).
            //      These were pre-materialised above from `ru.probes` for
            //      keys with no unit-backed entry.
            // A key present in BOTH categories prefers the body-scope unit so
            // sequencing relative to its surrounding units is preserved.
            for req_key in &unit.probes {
                let mut wired = false;
                if let Some(&probe_unit_idx) = probe_unit_index_by_key.get(req_key) {
                    if let Some(&probe_dag_id) = dag_id_by_unit_idx.get(&probe_unit_idx) {
                        if !all_deps.contains(&probe_dag_id) {
                            all_deps.push(probe_dag_id);
                        }
                        wired = true;
                    }
                    // If the probe dag_id isn't known yet (probe declared after consumer
                    // in units), the edge is silently skipped. In practice this cannot
                    // happen: engine.rs validates all probe keys exist as registered
                    // probes, and probes are pushed into units when cook.probe is called
                    // (before cook.add_unit in the same register block).
                }
                if !wired {
                    if let Some(&probe_dag_id) = synthesised_probe_dag_ids.get(req_key) {
                        if !all_deps.contains(&probe_dag_id) {
                            all_deps.push(probe_dag_id);
                        }
                    }
                }
            }

            // Build the WorkNode.
            // Per-unit env vars (e.g. chore param exports, COOK-36 §7.1.2)
            // are merged on top of the recipe-level env vars. Unit env wins
            // on conflicts (params shadow any recipe-level key of the same name).
            let merged_env_vars: std::collections::BTreeMap<String, String> = {
                let mut m = ru.env_vars.clone();
                m.extend(unit.unit_env_vars.iter().map(|(k, v)| (k.clone(), v.clone())));
                m
            };
            // R1 (CS-0164): only per-unit exports (chore params) go into the
            // spawned step's process environment. Config `var.*` values
            // (`ru.env_vars`) stay in `env_vars` for `cook.env` / consulted /
            // probe lookup but are `$<NAME>`-only — never injected into a step.
            let process_env_vars: std::collections::BTreeMap<String, String> = unit
                .unit_env_vars
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let work_node = if is_presatisfied(unit) {
                WorkNode {
                    payload: None,
                    recipe_name: ru.recipe_name.clone(),
                    cache_meta: None,
                    working_dir: ru.working_dir.clone(),
                    env_vars: merged_env_vars,
                    process_env_vars,
                    test_name: unit.test_name.clone(),
                    member: unit.member.clone(),
                }
            } else {
                WorkNode {
                    payload: Some(unit.payload.clone()),
                    recipe_name: ru.recipe_name.clone(),
                    cache_meta: unit.cache_meta.clone(),
                    working_dir: ru.working_dir.clone(),
                    env_vars: merged_env_vars,
                    process_env_vars,
                    test_name: unit.test_name.clone(),
                    member: unit.member.clone(),
                }
            };

            // Builder invariant: every id in `all_deps` originated from a
            // prior `add_node` call (cross-recipe leaves and within-recipe
            // barriers), so the call cannot fail with `DependencyOutOfRange`.
            let dag_id = dag
                .add_node(work_node, &all_deps)
                .expect("dag_builder produced an out-of-range dep id (bug)");

            // Record dag_id so later units can resolve probe→consumer edges.
            dag_id_by_unit_idx.insert(unit_idx, dag_id);

            // Update barrier / group tracking. Probes never advance the
            // barrier: they're pure fact-gathering and must not serialise
            // surrounding work. The pre-existing barrier flows through to the
            // next non-probe unit unchanged.
            if is_probe {
                continue;
            }
            match &unit.dep_kind {
                DepKind::Sequential => {
                    barrier = vec![dag_id];
                }
                DepKind::StepGroup(gi) => {
                    group_dag_ids.entry(*gi).or_default().push(dag_id);

                    // Check if this is the last member of the group.
                    if let Some(&(_, pos)) = unit_group_info.get(&unit_idx) {
                        let group_size = ru.step_groups[*gi].len();
                        if pos + 1 == group_size {
                            // Last member processed: group members become the
                            // new barrier.
                            barrier = group_dag_ids[gi].clone();
                        }
                    }
                }
                // `DepKind` is `#[non_exhaustive]`; treat unknown future
                // variants as a fresh sequential barrier.
                _ => {
                    barrier = vec![dag_id];
                }
            }
        }

        // Record this recipe's leaves: normally its final sequential
        // barrier, but when that barrier is EMPTY, forward `cross_deps`
        // instead (the union of this recipe's own `deps`' leaves, computed
        // above). The trigger is "empty barrier", not "zero units" — see
        // the module doc comment's "Leaf pass-through" section for the list
        // of ways a non-empty `units` still ends the loop with an empty
        // barrier, all of which must forward too, so a
        // downstream root unit still reaches the real upstream work instead
        // of silently losing the edge. `cross_deps` is itself already a
        // forwarded/transitive set by induction (every recipe earlier in
        // this topo-ordered slice applied the same rule), so a chain of
        // empty-barrier recipes forwards the original producer's leaves the
        // whole way down. When `cross_deps` is also empty (no deps of its
        // own), empty forwards empty — there is nothing to forward.
        let leaves = if barrier.is_empty() { cross_deps } else { barrier };
        recipe_leaves.insert(ru.recipe_name.clone(), leaves);
    }

    Ok(dag)
}

/// A unit is presatisfied (cached) when it has an empty shell command and
/// takes no part in caching.
///
/// COOK-360: the second half was written as `cache_meta.is_none()`, which
/// conflated "never cached" with "cached, but with no outputs". Exactly
/// equivalent today — nothing yet declares a cacheable output-less unit — and
/// stated so that when one does, it is not silently swept in here as a no-op.
fn is_presatisfied(unit: &CapturedUnit) -> bool {
    let uncacheable = cook_contracts::cache::record::cacheability(unit.cache_meta.as_ref())
        == cook_contracts::cache::record::Cacheability::Uncacheable;
    match &unit.payload {
        WorkPayload::Shell { cmd, .. } => cmd.is_empty() && uncacheable,
        // CS-0191: this needed a `Test` arm, to keep a Lua-body test — which
        // had an empty `cmd` by construction — from reading as the legacy
        // empty-shell no-op. The payload now says it: a Lua body is a
        // `LuaChunk` and falls through to `false`, and a command test is a
        // `Shell` the arm above judges on its own terms.
        _ => false,
    }
}

/// §22.1.2 terminal-output rule: check that no recipe's literal `inputs[]`
/// path is matched by another recipe's glob `outputs[]` pattern.
///
/// Detection is purely syntactic — no filesystem access, no working-directory
/// resolution. For each recipe that has any glob in its `outputs[]` entries,
/// we compile each glob pattern and test it against every literal `inputs[]`
/// string in every other recipe. On a positive match the function returns an
/// `EngineError::GlobbedOutputCrossRecipeEdge` naming both recipes, the
/// offending input path, and the matching pattern.
///
/// A `requires` edge alone (DAG ordering with no file-content dependency)
/// is NOT affected: this check only fires when a downstream recipe lists a
/// matching path as a literal member of its `inputs[]`.
pub(crate) fn check_globbed_output_cross_recipe_edges(
    recipes: &[RecipeUnits],
) -> Result<(), EngineError> {
    use globset::Glob;

    // Collect terminal output patterns per recipe name. Terminal outputs are
    // glob patterns (has_glob_meta) AND directory outputs (trailing `/`); both
    // are "build-owned" and must not be referenced as literal file inputs by
    // other recipes.  Use BTreeMap for deterministic iteration order so the
    // first error emitted is stable.
    let mut globbed_outputs_by_recipe: std::collections::BTreeMap<&str, Vec<&str>> =
        std::collections::BTreeMap::new();
    for r in recipes {
        for unit in &r.units {
            if let Some(meta) = &unit.cache_meta {
                for entry in &meta.output_paths {
                    if cook_cache::is_terminal_output(entry) {
                        globbed_outputs_by_recipe
                            .entry(r.recipe_name.as_str())
                            .or_default()
                            .push(entry.as_str());
                    }
                }
            }
        }
    }

    if globbed_outputs_by_recipe.is_empty() {
        return Ok(());
    }

    // Compile each pattern once. Patterns that fail to compile are silently
    // skipped here; downstream, resolve_glob in cook-fingerprint will also
    // treat them as matching no files, so the unit re-runs every invocation
    // without raising a diagnostic. This is a known footgun; a future CS
    // could add a register-time validity check.
    //
    // Asymmetry note: we use `globset` 0.4 here for pure string matching
    // (no filesystem access, per §22.1.2 "MUST NOT consult the filesystem").
    // `globset` 0.4's `**` semantics differ from `glob` 0.3's; in particular
    // `globset` matches `"build/**"` against `"build/foo.o"` correctly
    // without the trailing-`**` normalisation that `resolve_output_paths`
    // applies before passing patterns to `glob` 0.3. Do not add
    // `normalize_glob_pattern` here — it would either be a no-op or weaken
    // matching against legitimate paths.
    //
    // Directory-output normalisation: a trailing-`/` entry such as `"pkg/"`
    // has no glob metacharacter, so `Glob::new("pkg/")` would only match the
    // literal path `"pkg/"` — not files inside the directory. Expand it to
    // `"pkg/**"` so that any literal path under the subtree is matched.  The
    // raw entry string (`"pkg/"`) is kept for the diagnostic message so the
    // user sees the pattern they actually wrote in their Cookfile.
    let matchers: Vec<(&str, Vec<(&str, globset::GlobMatcher)>)> = globbed_outputs_by_recipe
        .iter()
        .map(|(name, patterns)| {
            let built: Vec<(&str, globset::GlobMatcher)> = patterns
                .iter()
                .filter_map(|p| {
                    let pat = if p.ends_with('/') {
                        format!("{p}**")
                    } else {
                        (*p).to_string()
                    };
                    Glob::new(&pat).ok().map(|g| (*p, g.compile_matcher()))
                })
                .collect();
            (*name, built)
        })
        .collect();

    // Test every other recipe's literal inputs[] strings against each pattern.
    for r in recipes {
        for unit in &r.units {
            if let Some(meta) = &unit.cache_meta {
                for entry in &meta.inputs {
                    // Skip entries that are themselves patterns — §22.1.2 only
                    // prohibits literal downstream inputs matching upstream
                    // glob patterns. A downstream pattern input is a different
                    // semantic (expansion at execute time) and is not checked
                    // here. Read off the declaration (§17.1.1.2) rather than
                    // re-scanned for metacharacters, so a real file whose name
                    // contains one is checked as the literal it is.
                    if entry.is_pattern() {
                        continue;
                    }
                    let input_path = &entry.path;
                    for (upstream_name, patterns) in &matchers {
                        // A recipe cannot violate its own terminal-output rule.
                        if *upstream_name == r.recipe_name.as_str() {
                            continue;
                        }
                        for (pat_str, matcher) in patterns {
                            if matcher.is_match(input_path.as_str()) {
                                return Err(EngineError::GlobbedOutputCrossRecipeEdge {
                                    upstream: upstream_name.to_string(),
                                    downstream: r.recipe_name.clone(),
                                    input: input_path.clone(),
                                    pattern: pat_str.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// §16.1.2 read-after-write rule: a literal `outputs[]` entry of recipe `A`
/// equal to a literal `inputs[]` entry of recipe `B`, both in the current
/// build closure, with no ordering path `B → A`, is rejected at plan time.
///
/// **A diagnostic, NOT an inferred edge.** §10.6 is explicit that only name
/// references create edges and that an implementation MUST NOT infer an edge
/// from path-string equality. So this function rejects the plan and
/// synthesises nothing — the author must name the producer.
///
/// # Why the predicate is DIRECTED (and §16.1.1's is not)
///
/// [`detect_output_collisions`] (§16.1.1, write-write) builds an UNDIRECTED
/// recipe graph and passes any connected pair. That is correct there: two
/// recipes writing the same path race, and EITHER ordering serialises the
/// race, so any path suffices.
///
/// Read-after-write is asymmetric. Only `B requires A` — producer first — is
/// correct. The reverse path, `A requires B`, orders the consumer BEFORE the
/// write: that is not an ambiguous race that scheduling might happen to win,
/// but a DETERMINISTIC stale/missing read, strictly worse than the unordered
/// case. An undirected predicate would wave it through as "dep-related,
/// fine". So this function deliberately does NOT reuse `connected()`; it uses
/// [`requires_transitively`] and passes only when `B` transitively requires
/// `A`. Every other configuration, the reverse path included, diagnoses.
///
/// # Why the check is CLOSURE-scoped (and §22.1.2's is not)
///
/// The caller passes the build closure, not the whole workspace. A literal
/// output is not build-owned, so `cook producer && cook consumer` is a
/// legitimate sequential workflow and MUST NOT be rejected — with only
/// `consumer` in the closure there is no producer to compare against and this
/// function stays silent. §22.1.2's glob rule is workspace-wide precisely
/// because terminality there is an OWNERSHIP claim, which holds regardless of
/// what is being built. (`tests/raw_path_cross_recipe_edge.rs` pins the
/// out-of-closure case; a workspace-wide check here would turn it red.)
///
/// Terminal (glob / directory) outputs are [`check_globbed_output_cross_recipe_edges`]'s
/// business and are skipped here so the two rules never double-report.
///
/// # Precedence over §16.1.1
///
/// §16.1.2 states normatively that where a closure violates BOTH this rule and
/// §16.1.1, the §16.1.1 collision is what must be reported: with two unordered
/// writers of one path, the producer this scan names is arbitrary (whichever
/// `BTreeSet` yields first), and the fix it advises would silence the
/// read-after-write while leaving the write-write race intact.
///
/// This function nonetheless runs BEFORE [`detect_output_collisions`] (which
/// opens [`build_dag`]), and that is deliberate, not an oversight. The
/// ordering is unobservable here: [`detect_output_collisions`] is undirected
/// over a graph that INCLUDES the requested target, so every recipe in a
/// single-target closure is connected to the target and therefore to every
/// other member — it cannot return `Some` for any closure `run_inner` builds.
/// There is nothing for this check to mask. Adding a pre-pass to "fix" the
/// order would buy an inert scan. If §16.1.1's predicate is ever narrowed so
/// it can fire on a real closure, the precedence must be pinned at that point.
///
/// # Which surfaces this can actually see
///
/// Detection needs a SOURCE-DECLARED path — one fixed by the Cookfile text,
/// not by what is on disk. `cook.add_unit`'s `inputs[]`/`outputs[]` and `cook`
/// step output literals qualify. An `ingredients` literal does NOT: it is a
/// glob resolved against the filesystem at register time (§21.2.1), so an
/// absent artifact matches zero files and reaches `input_paths` as nothing at
/// all. Covering it would invert the rule — silent on the cold build that
/// actually races, loud only once a stale artifact already exists. §16.1.2's
/// enumeration is therefore closed over the two surfaces above, and Note
/// 16.1.2.2 records the exclusion. (§10.6's *prohibition* still covers
/// `ingredients` literals; that is a rule about what must not happen and
/// needs no detection.)
pub(crate) fn check_literal_read_after_write(
    recipe_units: &[RecipeUnits],
) -> Result<(), EngineError> {
    // canonical path -> recipes declaring it as a LITERAL output.
    let mut producers_by_path: BTreeMap<PathBuf, BTreeSet<&str>> = BTreeMap::new();
    for ru in recipe_units {
        for unit in &ru.units {
            let Some(meta) = &unit.cache_meta else {
                continue;
            };
            for output in &meta.output_paths {
                // §22.1.2 owns terminal outputs — do not double-report.
                if cook_cache::is_terminal_output(output) {
                    continue;
                }
                producers_by_path
                    .entry(ru.working_dir.join(output))
                    .or_default()
                    .insert(ru.recipe_name.as_str());
            }
        }
    }

    if producers_by_path.is_empty() {
        return Ok(());
    }

    // DIRECTED recipe graph: name -> the recipes it requires.
    //
    // Direction convention: an entry "A requires B" means B executes BEFORE A
    // (`build_adjacency` sets `deps[name] = info.requires`, and `visit()`
    // recurses into deps before pushing the node). So an edge here points
    // from a recipe to something that runs earlier, and "B transitively
    // requires A" is exactly the ordering this rule demands.
    //
    // Both cross-recipe channels count as ordering: coarse `deps` (the
    // recipe-header colon list, into which the analyzer also folds
    // `cook.require_recipe`) and fine-grained `dep_edges` (`$<sigil>` /
    // `cook.dep_output`). Both are name references, so both are §10.6 edges.
    let mut requires: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for ru in recipe_units {
        let entry = requires.entry(ru.recipe_name.as_str()).or_default();
        for dep in &ru.deps {
            entry.insert(dep.as_str());
        }
        for (_, dep_name) in &ru.dep_edges {
            entry.insert(dep_name.as_str());
        }
    }

    for ru in recipe_units {
        for unit in &ru.units {
            let Some(meta) = &unit.cache_meta else {
                continue;
            };
            for entry in &meta.inputs {
                // A pattern input expands at execute time — not a literal
                // path match, and not this rule's business.
                if entry.is_pattern() {
                    continue;
                }
                let input = &entry.path;
                let canonical = ru.working_dir.join(input);
                let Some(producers) = producers_by_path.get(&canonical) else {
                    continue;
                };
                for producer in producers {
                    // A recipe's own units are already ordered within the
                    // recipe; reading your own output is not a race.
                    if *producer == ru.recipe_name.as_str() {
                        continue;
                    }
                    if requires_transitively(&requires, ru.recipe_name.as_str(), producer) {
                        continue;
                    }
                    return Err(EngineError::LiteralReadAfterWrite {
                        producer: (*producer).to_string(),
                        consumer: ru.recipe_name.clone(),
                        path: input.clone(),
                    });
                }
            }
        }
    }

    Ok(())
}

/// BFS reachability over the DIRECTED recipe `requires` graph: does `from`
/// transitively require `to`?
///
/// Deliberately NOT [`connected`] — see [`check_literal_read_after_write`] for
/// why §16.1.2 needs a directed predicate where §16.1.1 wants an undirected
/// one. Do not "unify" the two.
fn requires_transitively(graph: &BTreeMap<&str, BTreeSet<&str>>, from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(from);
    seen.insert(from);
    while let Some(node) = queue.pop_front() {
        if node == to {
            return true;
        }
        if let Some(deps) = graph.get(node) {
            for d in deps {
                if seen.insert(*d) {
                    queue.push_back(*d);
                }
            }
        }
    }
    false
}

/// Detect non-dep-related recipes that declare the same canonical output path.
///
/// Returns `Some(EngineError::OutputCollision)` for the first colliding path
/// found (deterministic — driven by `BTreeMap` iteration order). Returns
/// `None` when the wave is collision-free.
fn detect_output_collisions(recipe_units: &[RecipeUnits]) -> Option<EngineError> {
    // path -> set of recipe names that declare it
    let mut by_path: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for ru in recipe_units {
        for unit in &ru.units {
            let Some(meta) = &unit.cache_meta else {
                continue;
            };
            for output in &meta.output_paths {
                let canonical = ru.working_dir.join(output);
                by_path
                    .entry(canonical)
                    .or_default()
                    .insert(ru.recipe_name.clone());
            }
        }
    }

    // Build a recipe-level dep graph from RecipeUnits.deps. Edges are
    // bidirectional for the "dep-related" reachability check, since either
    // direction (A depends on B, or B depends on A) imposes ordering.
    let mut undirected: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ru in recipe_units {
        undirected.entry(ru.recipe_name.clone()).or_default();
        for dep in &ru.deps {
            undirected
                .entry(ru.recipe_name.clone())
                .or_default()
                .insert(dep.clone());
            undirected
                .entry(dep.clone())
                .or_default()
                .insert(ru.recipe_name.clone());
        }
    }

    for (path, recipes) in &by_path {
        if recipes.len() < 2 {
            continue;
        }
        // Pick any two recipes from the colliding set and check whether they
        // are connected in the undirected dep graph. If any pair is
        // disconnected, we have a true collision.
        let names: Vec<&String> = recipes.iter().collect();
        for i in 0..names.len() {
            for j in (i + 1)..names.len() {
                if !connected(&undirected, names[i], names[j]) {
                    return Some(EngineError::OutputCollision {
                        path: path.clone(),
                        recipes: recipes.iter().cloned().collect(),
                    });
                }
            }
        }
    }

    None
}

/// BFS reachability over the undirected recipe dep graph.
fn connected(graph: &BTreeMap<String, BTreeSet<String>>, a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(a);
    seen.insert(a);
    while let Some(node) = queue.pop_front() {
        if node == b {
            return true;
        }
        if let Some(neighbors) = graph.get(node) {
            for n in neighbors {
                if seen.insert(n.as_str()) {
                    queue.push_back(n.as_str());
                }
            }
        }
    }
    false
}

#[cfg(test)]
#[path = "tests/dag_builder_tests.rs"]
mod tests;

