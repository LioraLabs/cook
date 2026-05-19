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
/// `recipe_leaves` accumulator. A recipe that references a dep name not
/// present in the passed slice will silently get no edge for that dep, so
/// the caller is responsible for ensuring closure under dep edges and for
/// passing recipes in topological order. The old wave-based call-site hid
/// this property externally by passing one wave per call; the unified-DAG
/// path collapses to a single call. See `tests/unified_dag_build.rs` for the
/// integration pin.
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
pub fn build_dag(recipe_units: Vec<RecipeUnits>) -> Result<Dag<WorkNode>, EngineError> {
    // ── Plan-time output-collision check ─────────────────────────────────────
    // Accumulate every (canonical_output_path -> {recipe_name, ...}) pair from
    // all CacheMetas across all recipes in the wave. Two recipes that share a
    // canonical output path with no dependency path between them are racing
    // silently; reject the plan.
    if let Some(err) = detect_output_collisions(&recipe_units) {
        return Err(err);
    }

    let mut dag = Dag::new();

    // Map from recipe name -> its final barrier (leaf node ids).
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
        // `cook.cache.get(probe_key)` returned nil at execute time.
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
                working_dir: ru.working_dir.clone(),
                env_vars: ru.env_vars.clone(),
            };
            let dag_id = dag
                .add_node(work_node, &deps)
                .expect("synthesised probe deps originated from prior add_node calls");
            synthesised_probe_dag_ids.insert(key.clone(), dag_id);
        }

        // Collect cross-recipe dependency ids: the leaf nodes of every
        // prerequisite recipe.
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
            // `DepKind::StepGroup` / `DepKind::TestSibling` variants,
            // never for probes. A future change that broadens probes'
            // `dep_kind` MUST revisit this skip to keep the accounting
            // consistent.
            if skip_indices.contains(&unit_idx) {
                continue;
            }

            // Determine within-recipe dependencies for this unit.
            let within_deps: Vec<usize> = match &unit.dep_kind {
                DepKind::Sequential => barrier.clone(),
                DepKind::StepGroup(_) => barrier.clone(),
                DepKind::TestSibling(_) => barrier.clone(),
                // `DepKind` is `#[non_exhaustive]`; treat any future variant
                // conservatively as a sequential barrier until the dag-builder
                // is taught the new semantics.
                _ => barrier.clone(),
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
            let work_node = if is_presatisfied(unit) {
                WorkNode {
                    payload: None,
                    recipe_name: ru.recipe_name.clone(),
                    cache_meta: None,
                    working_dir: ru.working_dir.clone(),
                    env_vars: ru.env_vars.clone(),
                }
            } else {
                WorkNode {
                    payload: Some(unit.payload.clone()),
                    recipe_name: ru.recipe_name.clone(),
                    cache_meta: unit.cache_meta.clone(),
                    working_dir: ru.working_dir.clone(),
                    env_vars: ru.env_vars.clone(),
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

            // Update barrier / group tracking.
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
                DepKind::TestSibling(gi) => {
                    // Same group tracking as StepGroup — but edges will be
                    // annotated as TestSibling so cancel_subtree skips them.
                    group_dag_ids.entry(*gi).or_default().push(dag_id);

                    if let Some(&(_, pos)) = unit_group_info.get(&unit_idx) {
                        let group_size = ru.step_groups[*gi].len();
                        if pos + 1 == group_size {
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

        // Record this recipe's final barrier as its leaves.
        recipe_leaves.insert(ru.recipe_name.clone(), barrier);
    }

    Ok(dag)
}

/// A unit is presatisfied (cached) when it has an empty shell command and no
/// cache_meta.
fn is_presatisfied(unit: &CapturedUnit) -> bool {
    match &unit.payload {
        WorkPayload::Shell { cmd, .. } => cmd.is_empty() && unit.cache_meta.is_none(),
        WorkPayload::Test { cmd, .. } => cmd.is_empty() && unit.cache_meta.is_none(),
        _ => false,
    }
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

/// Compute the minimal set of unit indices required to execute every test
/// unit in `units`. Test units themselves are always included; non-test units
/// (cook/shell/lua) are included only if at least one test (transitively)
/// depends on them via `dep_edges`.
///
/// `dep_edges` is a slice of `(unit_index, output_path)` tuples meaning:
/// "unit at `unit_index` depends on the output at `output_path`". A
/// non-test unit that produces `output_path` is pulled into the slice.
///
/// Phase 3 of the runner pipeline per
/// docs/superpowers/specs/2026-05-07-test-runner-design.md §4.3.
pub fn build_test_slice(
    units: &[cook_contracts::CapturedUnit],
    dep_edges: &[(usize, String)],
) -> Vec<usize> {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use cook_contracts::WorkPayload;

    // Build output_path -> producing unit index from LuaChunk outputs and
    // CacheMeta output_paths (both can declare outputs).
    let mut producer_by_output: BTreeMap<String, usize> = BTreeMap::new();
    for (i, u) in units.iter().enumerate() {
        match &u.payload {
            WorkPayload::LuaChunk { outputs, .. } => {
                for out in outputs {
                    producer_by_output.insert(out.clone(), i);
                }
            }
            _ => {}
        }
        // Also index CacheMeta output_paths (covers shell/cook steps with cache info).
        if let Some(meta) = &u.cache_meta {
            for out in &meta.output_paths {
                producer_by_output.insert(out.clone(), i);
            }
        }
    }

    // BFS backward from every test unit, following dep_edges.
    let mut visited: BTreeSet<usize> = BTreeSet::new();
    let mut queue: VecDeque<usize> = units
        .iter()
        .enumerate()
        .filter(|(_, u)| matches!(u.payload, WorkPayload::Test { .. }))
        .map(|(i, _)| i)
        .collect();

    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }
        // Find all dep_edges for this unit and enqueue their producers.
        for (uid, dep_output) in dep_edges {
            if *uid != id {
                continue;
            }
            if let Some(&producer) = producer_by_output.get(dep_output) {
                if !visited.contains(&producer) {
                    queue.push_back(producer);
                }
            }
        }
    }

    let mut slice: Vec<usize> = visited.into_iter().collect();
    slice.sort();
    slice
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn shell(cmd: &str) -> WorkPayload {
        WorkPayload::Shell {
            cmd: cmd.to_string(),
            line: 0,
        }
    }

    fn default_wd() -> PathBuf {
        PathBuf::from(".")
    }

    fn default_env() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn probe(key: &str) -> WorkPayload {
        WorkPayload::Probe {
            key: key.to_string(),
            produce: "return 1".to_string(),
            line: 0,
        }
    }

    /// CS-0074 Bug 2 regression: DAG builder must add probe→consumer edges from
    /// CapturedUnit.probes. This verifies that when a probe unit precedes a
    /// consumer unit in units and the consumer's probes lists the probe key,
    /// the resulting DAG consumer node has the probe node as a dependency.
    #[test]
    fn dag_builder_adds_probe_to_consumer_edge() {
        let units = RecipeUnits {
            recipe_name: "build".into(),
            deps: vec![],
            units: vec![
                // Probe unit first (as cook.probe is called first in register block)
                CapturedUnit {
                    payload: probe("cc:zlib"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                },
                // Consumer unit with probes = ["cc:zlib"]
                CapturedUnit {
                    payload: shell("gcc -o app main.c"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec!["cc:zlib".to_string()],
                },
            ],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let dag = build_dag(vec![units]).expect("no collision");
        assert_eq!(dag.len(), 2);
        // Probe node (0) has no deps.
        assert_eq!(dag.node(0).remaining_deps(), 0, "probe node must have no deps");
        // Consumer node (1) depends on: sequential barrier (probe node 0) + probes edge (also probe 0).
        // The probes edge is deduplicated since it's the same node, so remaining_deps = 1.
        assert_eq!(
            dag.node(1).remaining_deps(),
            1,
            "consumer must depend on probe node via probes edge"
        );
    }

    #[test]
    fn test_build_single_recipe_sequential() {
        let units = RecipeUnits {
            recipe_name: "build".into(),
            deps: vec![],
            units: vec![
                CapturedUnit {
                    payload: shell("echo a"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                },
                CapturedUnit {
                    payload: shell("echo b"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                },
            ],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let dag = build_dag(vec![units]).expect("no collision");
        assert_eq!(dag.len(), 2);
        // Second node should depend on first
        assert_eq!(dag.node(0).remaining_deps(), 0);
        assert_eq!(dag.node(1).remaining_deps(), 1);
    }

    #[test]
    fn test_build_step_group() {
        // A step group of 2 units, then a sequential unit after
        let units = RecipeUnits {
            recipe_name: "build".into(),
            deps: vec![],
            units: vec![
                CapturedUnit {
                    payload: shell("gcc -c a.c"),
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                    probes: vec![],
                },
                CapturedUnit {
                    payload: shell("gcc -c b.c"),
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                    probes: vec![],
                },
                CapturedUnit {
                    payload: shell("ar rcs lib.a"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                },
            ],
            step_groups: vec![vec![0, 1]],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let dag = build_dag(vec![units]).expect("no collision");
        assert_eq!(dag.len(), 3);
        // Step group units have 0 deps (first in recipe)
        assert_eq!(dag.node(0).remaining_deps(), 0);
        assert_eq!(dag.node(1).remaining_deps(), 0);
        // Sequential unit after group depends on both group members
        assert_eq!(dag.node(2).remaining_deps(), 2);
    }

    #[test]
    fn test_build_cross_recipe_deps() {
        let setup = RecipeUnits {
            recipe_name: "setup".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("mkdir build"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let build = RecipeUnits {
            recipe_name: "build".into(),
            deps: vec!["setup".into()],
            units: vec![CapturedUnit {
                payload: shell("gcc main.c"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let dag = build_dag(vec![setup, build]).expect("no collision");
        assert_eq!(dag.len(), 2);
        // build's unit should depend on setup's unit
        assert_eq!(dag.node(1).remaining_deps(), 1);
    }

    #[test]
    fn test_build_empty() {
        let dag = build_dag(vec![]).expect("no collision");
        assert!(dag.is_empty());
    }

    #[test]
    fn test_fine_grained_cross_recipe_deps() {
        // libmath: compile group (2 units) -> archive (sequential)
        let libmath = RecipeUnits {
            recipe_name: "libmath".into(),
            deps: vec![],
            units: vec![
                CapturedUnit {
                    payload: shell("gcc -c add.c"),
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                    probes: vec![],
                },
                CapturedUnit {
                    payload: shell("gcc -c mul.c"),
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                    probes: vec![],
                },
                CapturedUnit {
                    payload: shell("ar rcs libmath.a"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                },
            ],
            step_groups: vec![vec![0, 1]],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec!["libmath.a".into()],
            dep_edges: vec![],
            probes: vec![],
        };

        // app: compile (1 unit, step group) -> link (sequential, depends on libmath)
        let app = RecipeUnits {
            recipe_name: "app".into(),
            deps: vec![],
            units: vec![
                CapturedUnit {
                    payload: shell("gcc -c main.c"),
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                    probes: vec![],
                },
                CapturedUnit {
                    payload: shell("gcc -o app main.o libmath.a"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                },
            ],
            step_groups: vec![vec![0]],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec!["app".into()],
            dep_edges: vec![(1, "libmath".into())], // unit 1 (link) depends on libmath
            probes: vec![],
        };

        let dag = build_dag(vec![libmath, app]).expect("no collision");
        assert_eq!(dag.len(), 5);

        // Nodes: 0=add.c, 1=mul.c, 2=archive, 3=main.c, 4=link

        // app's compile (node 3) should have 0 deps — can run in parallel with libmath
        assert_eq!(
            dag.node(3).remaining_deps(),
            0,
            "app compile should start immediately (no cross-recipe dep)"
        );

        // app's link (node 4) should depend on:
        // - node 3 (within-recipe: sequential after step group [3])
        // - node 2 (fine-grained: libmath's terminal node = archive)
        // Total: 2 deps
        assert_eq!(
            dag.node(4).remaining_deps(),
            2,
            "app link should depend on app compile + libmath archive"
        );
    }

    #[test]
    fn test_fine_grained_no_dep_edges_unchanged() {
        // Verify backward compat: recipes with dep_edges: vec![] behave as before
        let setup = RecipeUnits {
            recipe_name: "setup".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("mkdir build"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let build = RecipeUnits {
            recipe_name: "build".into(),
            deps: vec!["setup".into()],
            units: vec![CapturedUnit {
                payload: shell("gcc main.c"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let dag = build_dag(vec![setup, build]).expect("no collision");
        assert_eq!(dag.len(), 2);
        // build's unit depends on setup's unit via coarse deps
        assert_eq!(dag.node(1).remaining_deps(), 1);
    }

    #[test]
    fn test_build_presatisfied_units() {
        let units = RecipeUnits {
            recipe_name: "build".into(),
            deps: vec![],
            units: vec![
                CapturedUnit {
                    payload: WorkPayload::Shell {
                        cmd: String::new(),
                        line: 0,
                    },
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                },
                CapturedUnit {
                    payload: shell("echo real work"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                },
            ],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let dag = build_dag(vec![units]).expect("no collision");
        assert_eq!(dag.len(), 2);
        // First node is presatisfied (no payload)
        assert!(dag.node(0).payload().payload.is_none());
        // Second node has payload
        assert!(dag.node(1).payload().payload.is_some());
    }

    fn cache_meta_for(recipe: &str, outputs: &[&str]) -> cook_contracts::CacheMeta {
        cook_contracts::CacheMeta {
            recipe_name: recipe.to_string(),
            project_id: String::new(),
            cookfile_path: String::new(),
            cache_key: format!("k_{recipe}"),
            input_paths: vec![],
            output_paths: outputs.iter().map(|s| s.to_string()).collect(),
            command_hash: 0,
            context_hash: 0,
            env_contribution: 0,
            consulted_env: BTreeMap::new(),
            discovered_inputs: None,
        }
    }

    #[test]
    fn test_output_collision_unrelated_recipes_rejected() {
        // Two recipes, no dep edge, both declare the same output path.
        // build_dag MUST return EngineError::OutputCollision at plan time.
        let a = RecipeUnits {
            recipe_name: "a".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("touch out"),
                cache_meta: Some(cache_meta_for("a", &["build/shared.bin"])),
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec!["build/shared.bin".into()],
            dep_edges: vec![],
            probes: vec![],
        };
        let b = RecipeUnits {
            recipe_name: "b".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("touch out"),
                cache_meta: Some(cache_meta_for("b", &["build/shared.bin"])),
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec!["build/shared.bin".into()],
            dep_edges: vec![],
            probes: vec![],
        };
        let err = build_dag(vec![a, b]).expect_err("expected OutputCollision");
        match err {
            EngineError::OutputCollision { path, recipes } => {
                assert_eq!(path, default_wd().join("build/shared.bin"));
                assert!(recipes.contains(&"a".to_string()));
                assert!(recipes.contains(&"b".to_string()));
            }
            other => panic!("expected OutputCollision, got: {other:?}"),
        }
    }

    #[test]
    fn test_output_collision_dep_related_recipes_allowed() {
        // Two recipes, b depends on a, both touch same output. Allowed because
        // the dep edge enforces ordering — no race.
        let a = RecipeUnits {
            recipe_name: "a".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("touch out"),
                cache_meta: Some(cache_meta_for("a", &["build/shared.bin"])),
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec!["build/shared.bin".into()],
            dep_edges: vec![],
            probes: vec![],
        };
        let b = RecipeUnits {
            recipe_name: "b".into(),
            deps: vec!["a".into()],
            units: vec![CapturedUnit {
                payload: shell("touch out"),
                cache_meta: Some(cache_meta_for("b", &["build/shared.bin"])),
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec!["build/shared.bin".into()],
            dep_edges: vec![],
            probes: vec![],
        };
        let dag = build_dag(vec![a, b]).expect("dep edge allows shared output");
        assert_eq!(dag.len(), 2);
    }

    #[test]
    fn unreached_probe_is_pruned_from_dag() {
        use cook_contracts::{CapturedUnit, DepKind, ProbeUnit, ProbeInputs, WorkPayload};

        let probe_payload = WorkPayload::Probe {
            key: "k:unused".to_string(),
            produce: "return 1".to_string(),
            line: 1,
        };
        let probe_meta = ProbeUnit {
            key: "k:unused".to_string(),
            produce_source: "return 1".to_string(),
            produce_line: 1,
            inputs: ProbeInputs::default(),
        };

        let units = RecipeUnits {
            recipe_name: "r".to_string(),
            deps: vec![],
            units: vec![
                CapturedUnit {
                    payload: probe_payload,
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                },
                CapturedUnit {
                    payload: WorkPayload::Shell {
                        cmd: "echo hello".to_string(),
                        line: 2,
                    },
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                },
            ],
            step_groups: vec![],
            working_dir: std::path::PathBuf::from("/"),
            env_vars: std::collections::BTreeMap::new(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![probe_meta],
        };

        let dag = build_dag(vec![units]).expect("dag build");
        let probe_nodes: Vec<_> = (0..dag.len())
            .map(|i| dag.node(i))
            .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
            .collect();
        assert!(probe_nodes.is_empty(), "unreached probe must not appear in DAG");
    }

    #[test]
    fn probe_chain_keeps_upstream_when_downstream_consumed() {
        use cook_contracts::{CapturedUnit, DepKind, ProbeUnit, ProbeInputs, WorkPayload};

        let probe_a_payload = WorkPayload::Probe {
            key: "k:a".to_string(),
            produce: "return 1".to_string(),
            line: 1,
        };
        let probe_b_payload = WorkPayload::Probe {
            key: "k:b".to_string(),
            produce: "return 2".to_string(),
            line: 2,
        };
        let probe_a_meta = ProbeUnit {
            key: "k:a".to_string(),
            produce_source: "return 1".to_string(),
            produce_line: 1,
            inputs: ProbeInputs::default(),
        };
        let probe_b_meta = ProbeUnit {
            key: "k:b".to_string(),
            produce_source: "return 2".to_string(),
            produce_line: 2,
            inputs: ProbeInputs {
                requires: vec!["k:a".to_string()],
                ..ProbeInputs::default()
            },
        };
        let probe_a = CapturedUnit {
            payload: probe_a_payload,
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
        };
        let probe_b = CapturedUnit {
            payload: probe_b_payload,
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
        };
        let consumer = CapturedUnit {
            payload: WorkPayload::Shell { cmd: "echo".to_string(), line: 3 },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec!["k:b".to_string()],
        };

        let make_ru = |units: Vec<CapturedUnit>| RecipeUnits {
            recipe_name: "r".to_string(),
            units,
            deps: vec![],
            step_groups: vec![],
            working_dir: std::path::PathBuf::from("/"),
            env_vars: std::collections::BTreeMap::new(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![probe_a_meta.clone(), probe_b_meta.clone()],
        };

        let with_consumer = make_ru(vec![probe_a.clone(), probe_b.clone(), consumer]);
        let dag = build_dag(vec![with_consumer]).unwrap();
        let probe_count = (0..dag.len())
            .map(|i| dag.node(i))
            .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
            .count();
        assert_eq!(probe_count, 2, "both probes must be present when downstream is consumed");

        let without_consumer = make_ru(vec![probe_a, probe_b]);
        let dag2 = build_dag(vec![without_consumer]).unwrap();
        let probe_count2 = (0..dag2.len())
            .map(|i| dag2.node(i))
            .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
            .count();
        assert_eq!(probe_count2, 0, "both probes must be pruned when nothing consumes downstream");
    }

    /// SHI-222 Phase 8 regression: top-level register-scope probes (whose
    /// metadata flows into `RecipeUnits.probes` but which are NOT present as
    /// `WorkPayload::Probe` entries in `RecipeUnits.units`) must materialise
    /// as DAG nodes when a consumer's `probes` field references them.
    /// Pre-fix, these probes were silently dropped; the consumer's
    /// `cook.cache.get` returned nil at execute time.
    #[test]
    fn top_level_probe_materialises_when_consumer_references_it() {
        use cook_contracts::{CapturedUnit, DepKind, ProbeInputs, ProbeUnit, WorkPayload};

        let probe_meta = ProbeUnit {
            key: "cc:has_stdint_h".to_string(),
            produce_source: "return { ok = true }".to_string(),
            produce_line: 7,
            inputs: ProbeInputs::default(),
        };
        let ru = RecipeUnits {
            recipe_name: "game".into(),
            deps: vec![],
            // Note: NO Probe entry in units — only the consumer.
            units: vec![CapturedUnit {
                payload: shell("cc -o game main.c"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec!["cc:has_stdint_h".into()],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![probe_meta],
        };
        let dag = build_dag(vec![ru]).expect("no collision");
        assert_eq!(
            dag.len(),
            2,
            "expected synthesised probe node + consumer; got {} nodes",
            dag.len()
        );
        // Node 0 should be the synthesised Probe (no deps).
        assert!(
            matches!(dag.node(0).payload().payload, Some(WorkPayload::Probe { .. })),
            "node 0 must be the synthesised Probe"
        );
        assert_eq!(dag.node(0).remaining_deps(), 0);
        // Node 1 (consumer) depends on the synthesised probe.
        assert_eq!(
            dag.node(1).remaining_deps(),
            1,
            "consumer must depend on synthesised probe"
        );
    }

    /// SHI-222 Phase 8: synthesis must respect demand-driven scheduling —
    /// a top-level probe that no consumer references is not synthesised.
    #[test]
    fn top_level_probe_not_synthesised_when_no_consumer() {
        use cook_contracts::{CapturedUnit, DepKind, ProbeInputs, ProbeUnit, WorkPayload};

        let probe_meta = ProbeUnit {
            key: "cc:unused".to_string(),
            produce_source: "return 1".to_string(),
            produce_line: 1,
            inputs: ProbeInputs::default(),
        };
        let ru = RecipeUnits {
            recipe_name: "r".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("true"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![], // no references
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![probe_meta],
        };
        let dag = build_dag(vec![ru]).expect("no collision");
        let probe_nodes = (0..dag.len())
            .filter(|i| matches!(dag.node(*i).payload().payload, Some(WorkPayload::Probe { .. })))
            .count();
        assert_eq!(probe_nodes, 0, "unreferenced top-level probe must not be synthesised");
    }

    /// SHI-222 Phase 8: probe-on-probe transitive synthesis. If consumer
    /// references probe B, and probe B's `inputs.requires` lists probe A,
    /// both A and B must be synthesised, with B depending on A.
    #[test]
    fn top_level_probe_chain_synthesised_transitively() {
        use cook_contracts::{CapturedUnit, DepKind, ProbeInputs, ProbeUnit, WorkPayload};

        let probe_a = ProbeUnit {
            key: "cc:a".into(),
            produce_source: "return 1".into(),
            produce_line: 1,
            inputs: ProbeInputs::default(),
        };
        let probe_b = ProbeUnit {
            key: "cc:b".into(),
            produce_source: "return 2".into(),
            produce_line: 2,
            inputs: ProbeInputs {
                requires: vec!["cc:a".into()],
                ..ProbeInputs::default()
            },
        };
        let ru = RecipeUnits {
            recipe_name: "r".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("true"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec!["cc:b".into()],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![probe_a, probe_b],
        };
        let dag = build_dag(vec![ru]).expect("no collision");
        // 2 probes + 1 consumer = 3
        assert_eq!(dag.len(), 3);
        // Find nodes by key.
        let mut a_id = None;
        let mut b_id = None;
        for i in 0..dag.len() {
            if let Some(WorkPayload::Probe { key, .. }) = &dag.node(i).payload().payload {
                if key == "cc:a" {
                    a_id = Some(i);
                } else if key == "cc:b" {
                    b_id = Some(i);
                }
            }
        }
        let a_id = a_id.expect("probe A must be synthesised");
        let b_id = b_id.expect("probe B must be synthesised");
        // A has no deps, B depends on A.
        assert_eq!(dag.node(a_id).remaining_deps(), 0, "probe A must have no deps");
        assert_eq!(dag.node(b_id).remaining_deps(), 1, "probe B must depend on probe A");
        // Topo order: A added before B (A's dag_id < B's).
        assert!(a_id < b_id, "probe A must be added before probe B");
    }

    /// Regression: body-scope probe-on-body-scope-probe chains must NOT be
    /// demand-pruned. The cook_cc `needs = {...}` shape registers a chain
    /// where `cc:find:NAME` (body-scope) declares `inputs.requires =
    /// ["cc:linker-search-dirs", ...]` and `cc:linker-search-dirs` is also a
    /// body-scope probe. Pre-fix, `compute_consumed_probe_keys` only walked
    /// upstreams through `ru.probes` (top-level), so the body-scope upstream
    /// was never added to `consumed` and got dropped from the DAG, causing
    /// `cook-fingerprint` to fail with "requires upstream X which has no
    /// fingerprint" at execute time.
    #[test]
    fn body_scope_probe_chain_not_pruned() {
        use cook_contracts::{CapturedUnit, DepKind, WorkPayload};

        // Body-scope upstream probe (e.g. `cc:linker-search-dirs`).
        let upstream_probe = CapturedUnit {
            payload: WorkPayload::Probe {
                key: "cc:linker-search-dirs".into(),
                produce: "return {}".into(),
                line: 1,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![], // no upstream of its own
        };
        // Body-scope consumer probe (e.g. `cc:find:SDL3`) requiring the
        // upstream body-scope probe.
        let downstream_probe = CapturedUnit {
            payload: WorkPayload::Probe {
                key: "cc:find:SDL3".into(),
                produce: "return {}".into(),
                line: 2,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec!["cc:linker-search-dirs".into()],
        };
        // Non-probe consumer (the link unit) listing only the downstream
        // probe in its `probes`. The upstream must still survive the
        // demand-driven prune via the transitive closure across body-scope
        // probes.
        let link_unit = CapturedUnit {
            payload: shell("link"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec!["cc:find:SDL3".into()],
        };

        let ru = RecipeUnits {
            recipe_name: "game".into(),
            deps: vec![],
            units: vec![upstream_probe, downstream_probe, link_unit],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![], // body-scope probes are NOT mirrored into ru.probes
        };
        let dag = build_dag(vec![ru]).expect("no collision");
        // Both probe nodes must survive; otherwise pruning regressed.
        let probe_keys: BTreeSet<String> = (0..dag.len())
            .filter_map(|i| match &dag.node(i).payload().payload {
                Some(WorkPayload::Probe { key, .. }) => Some(key.clone()),
                _ => None,
            })
            .collect();
        assert!(
            probe_keys.contains("cc:linker-search-dirs"),
            "body-scope upstream probe must survive demand prune, got nodes: {probe_keys:?}"
        );
        assert!(
            probe_keys.contains("cc:find:SDL3"),
            "body-scope consumer probe must survive demand prune, got nodes: {probe_keys:?}"
        );
        assert_eq!(dag.len(), 3, "expected 2 probes + 1 link unit");
    }

    #[test]
    fn multi_recipe_wave_prunes_independently() {
        use cook_contracts::{CapturedUnit, DepKind, ProbeUnit, ProbeInputs, WorkPayload};

        fn make_recipe(name: &str, has_consumer: bool) -> RecipeUnits {
            let probe_meta = ProbeUnit {
                key: "k:p".to_string(),
                produce_source: "return 1".to_string(),
                produce_line: 1,
                inputs: ProbeInputs::default(),
            };
            let mut units = vec![CapturedUnit {
                payload: WorkPayload::Probe {
                    key: "k:p".to_string(),
                    produce: "return 1".to_string(),
                    line: 1,
                },
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }];
            units.push(CapturedUnit {
                payload: WorkPayload::Shell { cmd: "echo".to_string(), line: 2 },
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: if has_consumer { vec!["k:p".to_string()] } else { vec![] },
            });
            RecipeUnits {
                recipe_name: name.to_string(),
                units,
                deps: vec![],
                step_groups: vec![],
                working_dir: std::path::PathBuf::from("/"),
                env_vars: std::collections::BTreeMap::new(),
                terminal_outputs: vec![],
                dep_edges: vec![],
                probes: vec![probe_meta],
            }
        }

        let foo = make_recipe("foo", true);
        let bar = make_recipe("bar", false);
        let dag = build_dag(vec![foo, bar]).unwrap();
        let probe_node_recipes: Vec<String> = (0..dag.len())
            .map(|i| dag.node(i))
            .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
            .map(|n| n.payload().recipe_name.clone())
            .collect();
        assert_eq!(probe_node_recipes, vec!["foo".to_string()],
            "probe present only in the recipe that consumes it");
    }

    #[test]
    fn test_output_collision_distinct_outputs_allowed() {
        let a = RecipeUnits {
            recipe_name: "a".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("touch out"),
                cache_meta: Some(cache_meta_for("a", &["build/a.bin"])),
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec!["build/a.bin".into()],
            dep_edges: vec![],
            probes: vec![],
        };
        let b = RecipeUnits {
            recipe_name: "b".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("touch out"),
                cache_meta: Some(cache_meta_for("b", &["build/b.bin"])),
                dep_kind: DepKind::Sequential,
                probes: vec![],
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec!["build/b.bin".into()],
            dep_edges: vec![],
            probes: vec![],
        };
        let dag = build_dag(vec![a, b]).expect("distinct outputs OK");
        assert_eq!(dag.len(), 2);
    }
}

#[cfg(test)]
mod test_slice_tests {
    use super::*;
    use cook_contracts::{CapturedUnit, DepKind, StepKind, WorkPayload};
    use std::collections::BTreeSet;

    /// Build a LuaChunk unit that declares the given output paths.
    /// Used as the "cook step" stand-in since WorkPayload has no Cook variant;
    /// LuaChunk is the payload emitted for declarative cook steps.
    fn mk_cook(outputs: &[&str]) -> CapturedUnit {
        CapturedUnit {
            payload: WorkPayload::LuaChunk {
                code: "cook.sh(\"echo > \" .. output)".into(),
                inputs: vec![],
                outputs: outputs.iter().map(|s| s.to_string()).collect(),
                ingredient_groups: vec![],
                step_kind: StepKind::Cook,
                is_chore: false,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
        }
    }

    fn mk_test() -> CapturedUnit {
        CapturedUnit {
            payload: WorkPayload::Test {
                cmd: "true".into(),
                line: 1,
                timeout: 30,
                should_fail: false,
                suite_name: "r".into(),
                test_name: "t".into(),
                iteration_item: None,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
        }
    }

    #[test]
    fn build_test_slice_excludes_unrelated_cook_units() {
        // Units:
        //   #0: cook produces "needed.bin"  (test #2 depends on this)
        //   #1: cook produces "unrelated.bin" (no test depends)
        //   #2: test depends on "needed.bin" via dep_edges
        //   #3: test (one-shot, no deps)
        let units = vec![
            mk_cook(&["needed.bin"]),
            mk_cook(&["unrelated.bin"]),
            mk_test(),
            mk_test(),
        ];
        let dep_edges = vec![(2usize, "needed.bin".to_string())];

        let slice = build_test_slice(&units, &dep_edges);
        let s: BTreeSet<_> = slice.iter().copied().collect();
        assert!(s.contains(&0), "cook needed by a test must be in slice");
        assert!(s.contains(&2), "test units always in slice");
        assert!(s.contains(&3), "one-shot test always in slice");
        assert!(!s.contains(&1), "unrelated cook must be excluded");
    }

    #[test]
    fn build_test_slice_handles_transitive_deps() {
        // #0 cook produces "a.out"
        // #1 cook produces "b.out", depends on "a.out"
        // #2 test depends on "b.out"
        let units = vec![
            mk_cook(&["a.out"]),
            mk_cook(&["b.out"]),
            mk_test(),
        ];
        let dep_edges = vec![
            (1usize, "a.out".to_string()),
            (2usize, "b.out".to_string()),
        ];
        let slice = build_test_slice(&units, &dep_edges);
        assert_eq!(slice.len(), 3, "transitive cook deps must be included; got: {slice:?}");
    }

    #[test]
    fn build_test_slice_empty_when_no_tests() {
        let units = vec![mk_cook(&["x.out"])];
        let dep_edges = vec![];
        let slice = build_test_slice(&units, &dep_edges);
        assert!(slice.is_empty(), "no test units => empty slice");
    }

    #[test]
    fn build_test_slice_all_tests_no_deps() {
        let units = vec![mk_test(), mk_test(), mk_test()];
        let dep_edges = vec![];
        let slice = build_test_slice(&units, &dep_edges);
        assert_eq!(slice, vec![0, 1, 2], "all test units with no deps");
    }
}
