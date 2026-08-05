//! The unit-level dependency graph implied by registration (COOK-402).
//!
//! This is the wiring decision, written once: given the registered
//! `RecipeUnits` of a build closure, which units become graph nodes and which
//! dependencies each node has — with the reason each dependency exists. Two
//! crates must agree on the answer and neither owns it: `cook-engine` lowers
//! the plan into the `Dag<WorkNode>` it executes, and `cook-graph` renders it
//! as `cook why`'s structure. Before this module the renderer hand-mirrored
//! the engine's wiring and drifted twice — it kept the fine-covered
//! narrowing rule CS-0161 withdrew, and it dropped dependencies routed
//! through unit-less meta-targets (no leaf pass-through). One decision, one
//! home (CS-0202).
//!
//! Everything here is a function of its arguments: no filesystem, no
//! environment, no process. What the plan deliberately does NOT decide is
//! lowering policy — payload construction, env merging, the presatisfied
//! rule — which reads the same units but answers a question only the
//! executor asks.
//!
//! # The wiring rules
//!
//! Within-recipe:
//! - `DepKind::Sequential` units depend on the current barrier (the set of
//!   nodes that must finish before the next sequential unit can start).
//! - `DepKind::StepGroup(idx)` units all share the same barrier (the one
//!   active when the group started). When the last member of a group is
//!   processed, all group members become the new barrier.
//! - Probes never read or advance the barrier: they are pure fact-gathering,
//!   ordered only via `inputs.requires` and their consumers' `probes` lists.
//!
//! Cross-recipe (coarse):
//! - A recipe's root units (those with no within-recipe deps) additionally
//!   depend on the leaves of every recipe listed in `deps`. `deps` is the
//!   recipe's DECLARED `requires` — see [`declared_coarse_deps`] for why the
//!   closure edge map must be filtered before it is stamped here. This edge
//!   is strictly additive with the fine-grained channel (CS-0161, "Rejected
//!   alternative"): a fine ref never subtracts a declared barrier.
//!
//! Cross-recipe (fine-grained):
//! - For each `(unit_idx, dep_recipe_name)` in `ru.dep_edges`, that specific
//!   unit additionally depends on the leaves of the named recipe.
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
//!
//! Demand-driven probe scheduling (§22.5.7):
//! - Probe units whose keys are not transitively referenced by any non-probe
//!   unit's `probes` field are silently omitted. No diagnostic is emitted.
//! - Consumer-referenced probe keys with no `WorkPayload::Probe` entry in
//!   `ru.units` but metadata in `ru.probes` (top-level register-block probes,
//!   SHI-222 Phase 8) get a synthesised node, wired in dependency order.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{CapturedUnit, RecipeUnits, WorkPayload};
use crate::unit::DepKind;

/// Where a graph node came from, in terms a caller that holds the original
/// `RecipeUnits` slice can resolve: a captured unit is `(recipe, unit_idx)`;
/// a probe synthesised from register-block metadata has no unit index and is
/// identified by its probe key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeOrigin {
    Unit { recipe: String, unit_idx: usize },
    SynthProbe { recipe: String, probe_key: String },
}

/// Why the plan imposed a dependency. Recorded at the exact points the
/// dependency is assembled, so a consumer rendering the graph reports the
/// edges the scheduler imposes rather than re-deriving them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeProvenance {
    /// §15.1 sequential barrier between units of one recipe.
    Serial,
    /// Entry into a step group: each member depends on the barrier that was
    /// current when the group started.
    Group,
    /// A probe value consumed by a unit (§22.5.5), including probe-on-probe
    /// `inputs.requires`.
    Probe,
    /// Per-unit cross-recipe ordering — `cook.dep_order` / `cook.dep_output`
    /// (§22.10). Only the referencing unit waits.
    DepOrder,
    /// Whole-recipe barrier — a dep-list entry or `cook.require_recipe`
    /// (§22.8). Every root unit of the consumer waits for the referent's
    /// leaves. Strictly additive with [`EdgeProvenance::DepOrder`]
    /// (CS-0161): a fine ref never subtracts a declared coarse edge.
    Barrier,
}

/// One planned node: its origin and its dependencies, each carrying the
/// reason it exists. `deps` indexes into [`UnitGraph::nodes`] and only ever
/// points at earlier nodes, so a consumer adding nodes in order can resolve
/// every dependency as it goes. The same dependency index may appear under
/// two provenances — a `requires` barrier and a `dep_order` ref to the same
/// producer are both real (the additive rule) — but exact duplicates are
/// removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitGraphNode {
    pub origin: NodeOrigin,
    pub deps: Vec<(usize, EdgeProvenance)>,
}

/// The unit-level dependency graph: nodes in the order a consumer should
/// materialise them (dependencies strictly before dependents).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitGraph {
    pub nodes: Vec<UnitGraphNode>,
}

/// The two ways a closure can fail to plan. Everything else about a
/// registered workspace — output collisions, read-after-write hazards — is
/// execution policy and is checked by the crates that execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitGraphError {
    /// A `dep_edges` entry names a recipe absent from the slice. This
    /// channel is not analyzer-validated upstream the way `deps`/`requires`
    /// is, so the plan is the last chance to catch it.
    DanglingDepEdge {
        referring_recipe: String,
        dep_name: String,
    },
    /// The recipe-level edge map has a cycle; `unresolved` names the recipes
    /// with unresolved in-degree (part of, or downstream of, the cycle).
    Cycle { unresolved: Vec<String> },
}

impl std::fmt::Display for UnitGraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnitGraphError::DanglingDepEdge {
                referring_recipe,
                dep_name,
            } => write!(
                f,
                "recipe '{referring_recipe}' has a fine-grained dependency (dep_edges) on \
                 recipe '{dep_name}', which is not present in the build closure"
            ),
            UnitGraphError::Cycle { unresolved } => {
                write!(f, "cycle among recipes: {unresolved:?}")
            }
        }
    }
}

impl std::error::Error for UnitGraphError {}

/// Restrict a recipe's coarse dep list to what it actually declared.
///
/// The closure edge map (`cook-plan`'s `dependency_edges`) merges `requires`
/// with `orders` — names reached only through fine-grained per-unit refs
/// (`$<B>`, `cook.dep_output`, `cook.dep_order`). Those carry their ordering
/// in `dep_edges`; promoting them to `RecipeUnits::deps` would manufacture
/// the coarse whole-recipe barrier the fine reference exists to avoid. Every
/// caller that stamps closure edges onto a `RecipeUnits` slice bound for
/// [`plan`] MUST filter through this helper so the run path, the explanation
/// path, and the graph renderer agree on which barriers exist.
pub fn declared_coarse_deps(declared: &[String], closure_deps: &[String]) -> Vec<String> {
    let declared_set: BTreeSet<&str> = declared.iter().map(|s| s.as_str()).collect();
    closure_deps
        .iter()
        .filter(|d| declared_set.contains(d.as_str()))
        .cloned()
        .collect()
}

/// Topologically sort `reachable` against the recipe-level `edges` map.
///
/// Returns recipe names in dependency-first order, which is the order
/// [`plan`] requires: its intra-call leaves accumulator must have every dep
/// present before the dependent recipe is walked. Recipes missing from
/// `edges` are placed first (no deps known). Deterministic: Kahn's walk over
/// `BTreeMap`/`BTreeSet`, ties in name order.
pub fn toposort_recipes(
    edges: &BTreeMap<String, Vec<String>>,
    reachable: &BTreeSet<String>,
) -> Result<Vec<String>, UnitGraphError> {
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
        // Only nodes with unresolved in-degree are part of (or downstream
        // of) the cycle. Naming them is far more useful than dumping the
        // entire reachable set.
        let unresolved: Vec<String> = in_degree
            .iter()
            .filter(|(_, &d)| d > 0)
            .map(|(name, _)| name.clone())
            .collect();
        return Err(UnitGraphError::Cycle { unresolved });
    }
    Ok(order)
}

/// Compute the set of probe keys reached by at least one non-probe consumer
/// in `units`, transitively closing under probe-on-probe `inputs.requires`.
///
/// A probe's upstream `inputs.requires` is read from two sources, depending
/// on where the probe was registered:
///
/// * **Top-level probes** (`cook.probe(...)` called outside any recipe body)
///   live in `probes` — the [`RecipeUnits`] view drained from
///   `session_state.probes` at registration. Their `inputs.requires` lives
///   on the [`crate::ProbeUnit`] directly.
/// * **Body-scope probes** (`cook.probe(...)` called inside a recipe body)
///   are `WorkPayload::Probe` entries in `units` with their
///   `inputs.requires` mirrored onto the surrounding [`CapturedUnit`]'s
///   `probes` field. They do NOT appear in `probes` because the registry is
///   drained once before any body runs.
///
/// Both indexes are consulted during the transitive closure so a body-scope
/// consumer probe can legitimately pull its body-scope upstream into
/// `consumed` (without this, demand-driven pruning silently drops the
/// upstream and the executor later reports "requires upstream X which has no
/// fingerprint").
fn compute_consumed_probe_keys(
    units: &[CapturedUnit],
    probes: &[crate::ProbeUnit],
) -> BTreeSet<String> {
    // Top-level probes (ru.probes): inputs.requires lives on the ProbeUnit.
    let top_level_probe_by_key: BTreeMap<&str, &crate::ProbeUnit> =
        probes.iter().map(|p| (p.key.as_str(), p)).collect();

    // Body-scope probes (WorkPayload::Probe entries in ru.units): their
    // inputs.requires is carried on the CapturedUnit.probes field.
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

/// Plan the unit-level dependency graph for a topologically-sorted closure.
///
/// **Unified-call contract (SHI-222).** The caller passes _every_ reachable
/// recipe in a single invocation; cross-recipe edges (both coarse `deps` and
/// fine-grained `dep_edges`) are resolved intra-call against the running
/// leaves accumulator, so the caller is responsible for passing recipes in
/// topological order — [`toposort_recipes`] produces one.
///
/// A `dep_edges` entry that names a recipe absent from the slice is rejected
/// up front with [`UnitGraphError::DanglingDepEdge`] — this channel is not
/// analyzer-validated upstream the way `deps`/`requires` is, so the plan is
/// the last chance to catch it. A `deps` entry naming an unknown recipe is a
/// different story: the analyzer rejects it before any plan is attempted, so
/// an absent leaves lookup below is unreachable on the live path and is left
/// unchecked.
///
/// The wiring rules — barriers, groups, probes, demand-driven pruning,
/// synthesis, leaf pass-through — are stated in the module doc above; this
/// function is their single implementation.
pub fn plan(recipe_units: &[RecipeUnits]) -> Result<UnitGraph, UnitGraphError> {
    // ── Pre-walk: dep_edges closure validation ──────────────────────────────
    // Checked against the complete set of recipe names present in the call —
    // NOT against the leaves accumulator below, which is populated only as
    // the topo-ordered slice is walked. Checking leaves per-unit would
    // false-positive on any dep_edges target not yet reached in topo order;
    // the full name set up front correctly treats "present in the slice but
    // zero units" (a legitimate empty leaf set) as distinct from "absent
    // from the slice" (the genuine error).
    let known_recipe_names: BTreeSet<&str> =
        recipe_units.iter().map(|ru| ru.recipe_name.as_str()).collect();
    for ru in recipe_units {
        for (_, dep_name) in &ru.dep_edges {
            if !known_recipe_names.contains(dep_name.as_str()) {
                return Err(UnitGraphError::DanglingDepEdge {
                    referring_recipe: ru.recipe_name.clone(),
                    dep_name: dep_name.clone(),
                });
            }
        }
    }

    let mut nodes: Vec<UnitGraphNode> = Vec::new();

    // Map from recipe name -> its leaf node ids: normally its final barrier,
    // but forwarded from its own `deps`' leaves when that barrier is empty
    // (see "Leaf pass-through" in the module doc).
    let mut recipe_leaves: BTreeMap<String, Vec<usize>> = BTreeMap::new();

    for ru in recipe_units {
        // Per-recipe index of probe key → unit index, to wire probe→consumer
        // edges from CapturedUnit.probes (CS-0074 Bug 2).
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
        // probes that are NOT present in `ru.units` — SHI-222 Phase 8).
        // Body-scope probes appear in both, and `probe_unit_index_by_key`
        // takes precedence for those when wiring consumer edges so the
        // body-scope node is reused.
        let probe_meta_by_key: BTreeMap<&str, &crate::ProbeUnit> =
            ru.probes.iter().map(|p| (p.key.as_str(), p)).collect();

        // node_by_unit_idx: populated as each unit is added; resolves
        // probe-unit node ids when wiring CapturedUnit.probes edges.
        let mut node_by_unit_idx: BTreeMap<usize, usize> = BTreeMap::new();

        // Demand-driven probe scheduling (§22.5.7): probe units whose key is
        // not transitively consumed are pruned.
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

        // Coarse cross-recipe deps: the leaves of every prerequisite recipe.
        // Also inherited by synthesised probes as their root deps, so they
        // cannot run before prerequisite recipes finish.
        //
        // Asymmetry note: unlike `ru.dep_edges` (validated by the pre-walk
        // check above), a `ru.deps` entry naming a recipe absent from
        // `recipe_leaves` is not diagnosed here. `deps` lowers from
        // `requires`, which the analyzer already rejects when it names an
        // unknown recipe — on the live path this lookup missing is already
        // impossible, and a redundant check would only duplicate that
        // upstream validation.
        let mut cross_deps: Vec<usize> = Vec::new();
        for dep_name in &ru.deps {
            if let Some(leaves) = recipe_leaves.get(dep_name) {
                cross_deps.extend(leaves);
            }
        }

        // ── Synthesised top-level probes (SHI-222 Phase 8) ──────────────────
        // Pre-materialise nodes for consumer-referenced probe keys that have
        // no `WorkPayload::Probe` entry in `ru.units`: top-level probes
        // registered at register-block scope (e.g. through helpers like
        // `cook_cc.checks.has_header`). Same demand-driven pruning rule as
        // body-scope probes: only keys in `consumed` get a node.
        let mut synthesised_probe_ids: BTreeMap<String, usize> = BTreeMap::new();

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
        // practice — `inputs.requires` is validated at register time) fall
        // through and edges to the missing upstream are simply omitted.
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
        // Keys not popped (cycle — should be unreachable) are appended
        // anyway so they at least get nodes; edges to upstream synthesised
        // peers will be missing, mirroring the probe-declared-after-consumer
        // fallback below.
        for k in &synth_keys {
            if !ordered_synth.contains(k) {
                ordered_synth.push(k.clone());
            }
        }

        for key in &ordered_synth {
            let meta = probe_meta_by_key
                .get(key.as_str())
                .expect("synth_keys filtered on probe_meta_by_key membership");
            let mut deps: Vec<(usize, EdgeProvenance)> = cross_deps
                .iter()
                .map(|&d| (d, EdgeProvenance::Barrier))
                .collect();
            for upstream in &meta.inputs.requires {
                if let Some(&id) = synthesised_probe_ids.get(upstream) {
                    deps.push((id, EdgeProvenance::Probe));
                }
                // An upstream that resolves to a body-scope probe cannot be
                // wired here: that unit hasn't been added yet (synthesis
                // precedes the unit walk). Top-level → top-level is the only
                // direction supported, which matches the helper-emitted
                // shape.
            }
            let id = nodes.len();
            nodes.push(UnitGraphNode {
                origin: NodeOrigin::SynthProbe {
                    recipe: ru.recipe_name.clone(),
                    probe_key: key.clone(),
                },
                deps: dedup_pairs(deps),
            });
            synthesised_probe_ids.insert(key.clone(), id);
        }

        // ── The unit walk ───────────────────────────────────────────────────
        // Quick lookup: unit index -> (step_group, position within it).
        let mut unit_group_info: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        for (gi, group) in ru.step_groups.iter().enumerate() {
            for (pos, &unit_idx) in group.iter().enumerate() {
                unit_group_info.insert(unit_idx, (gi, pos));
            }
        }

        // Current barrier: the node ids the next sequential unit depends on.
        let mut barrier: Vec<usize> = Vec::new();

        // Node ids per step group, to form the barrier when the group ends.
        let mut group_ids: BTreeMap<usize, Vec<usize>> = BTreeMap::new();

        for (unit_idx, unit) in ru.units.iter().enumerate() {
            // Demand-driven prune (§22.5.7): the `barrier` carries forward
            // unchanged since we skip before any node push or barrier
            // mutation; the probe→consumer wiring below safely no-ops for
            // these probes since their ids never enter `node_by_unit_idx`.
            //
            // Load-bearing invariant: probes are always emitted with
            // `DepKind::Sequential` by `install_cook_probe`, so pruning them
            // does not desync step-group accounting — group bookkeeping
            // below only fires for the `DepKind::StepGroup` variant, never
            // for probes. A future change that broadens probes' `dep_kind`
            // MUST revisit this skip.
            if skip_indices.contains(&unit_idx) {
                continue;
            }

            // Probes are pure fact-gathering and do not participate in the
            // sequential barrier — they depend only on cross-recipe deps,
            // explicit `dep_edges`, and their upstream probes (wired below
            // via `unit.probes`). Without this, sibling probes chain
            // one-after-another because `install_cook_probe` emits all
            // probes with `DepKind::Sequential`.
            let is_probe = matches!(unit.payload, WorkPayload::Probe { .. });

            // Within-recipe dependencies: the current barrier. The recorded
            // provenance is the consumer's own dep_kind — a step-group
            // member depends on the prior barrier as a group entry,
            // everything else as a sequential barrier.
            let within_deps: Vec<usize> = if is_probe { Vec::new() } else { barrier.clone() };
            let within_kind = match &unit.dep_kind {
                DepKind::StepGroup(_) => EdgeProvenance::Group,
                DepKind::Sequential => EdgeProvenance::Serial,
            };

            // Coarse cross-recipe deps apply only to root units (no
            // within-recipe deps).
            let mut deps: Vec<(usize, EdgeProvenance)> = if within_deps.is_empty() {
                cross_deps
                    .iter()
                    .map(|&d| (d, EdgeProvenance::Barrier))
                    .collect()
            } else {
                within_deps
                    .into_iter()
                    .map(|d| (d, within_kind))
                    .collect()
            };

            // Fine-grained dep edges: the leaves of specific recipes, for
            // this exact unit, regardless of within-recipe deps.
            for (dep_unit_idx, dep_recipe_name) in &ru.dep_edges {
                if *dep_unit_idx == unit_idx {
                    if let Some(leaves) = recipe_leaves.get(dep_recipe_name) {
                        deps.extend(leaves.iter().map(|&d| (d, EdgeProvenance::DepOrder)));
                    }
                }
            }

            // Probe→consumer edges from CapturedUnit.probes (CS-0074 Bug 2).
            // Resolution order (SHI-222 Phase 8): body-scope probe units
            // first, synthesised top-level probes second; a key present in
            // both prefers the body-scope unit so sequencing relative to its
            // surrounding units is preserved.
            for req_key in &unit.probes {
                let mut wired = false;
                if let Some(&probe_unit_idx) = probe_unit_index_by_key.get(req_key) {
                    if let Some(&probe_id) = node_by_unit_idx.get(&probe_unit_idx) {
                        deps.push((probe_id, EdgeProvenance::Probe));
                        wired = true;
                    }
                    // If the probe id isn't known yet (probe declared after
                    // consumer), the edge is silently skipped. In practice
                    // this cannot happen: probe keys are validated at
                    // register time and probes are pushed into units when
                    // cook.probe is called, before cook.add_unit in the
                    // same register block.
                }
                if !wired {
                    if let Some(&probe_id) = synthesised_probe_ids.get(req_key) {
                        deps.push((probe_id, EdgeProvenance::Probe));
                    }
                }
            }

            let id = nodes.len();
            nodes.push(UnitGraphNode {
                origin: NodeOrigin::Unit {
                    recipe: ru.recipe_name.clone(),
                    unit_idx,
                },
                deps: dedup_pairs(deps),
            });
            node_by_unit_idx.insert(unit_idx, id);

            // Update barrier / group tracking. Probes never advance the
            // barrier: the pre-existing barrier flows through to the next
            // non-probe unit unchanged.
            if is_probe {
                continue;
            }
            // This match is exhaustive on purpose — `DepKind` is defined in
            // this crate, so a new variant fails compilation HERE, forcing
            // the wiring law to decide its semantics instead of silently
            // defaulting.
            match &unit.dep_kind {
                DepKind::Sequential => {
                    barrier = vec![id];
                }
                DepKind::StepGroup(gi) => {
                    group_ids.entry(*gi).or_default().push(id);
                    if let Some(&(_, pos)) = unit_group_info.get(&unit_idx) {
                        let group_size = ru.step_groups[*gi].len();
                        if pos + 1 == group_size {
                            // Last member processed: the group becomes the
                            // new barrier.
                            barrier = group_ids[gi].clone();
                        }
                    }
                }
            }
        }

        // Record this recipe's leaves: normally its final sequential
        // barrier, but when that barrier is EMPTY, forward `cross_deps`
        // instead — see "Leaf pass-through" in the module doc for the ways
        // a non-empty `units` still ends the loop with an empty barrier.
        // `cross_deps` is itself already a forwarded/transitive set by
        // induction (every recipe earlier in this topo-ordered slice applied
        // the same rule), so a chain of empty-barrier recipes forwards the
        // original producer's leaves the whole way down. When `cross_deps`
        // is also empty, empty forwards empty — there is nothing to forward.
        let leaves = if barrier.is_empty() { cross_deps } else { barrier };
        recipe_leaves.insert(ru.recipe_name.clone(), leaves);
    }

    Ok(UnitGraph { nodes })
}

/// Remove exact `(id, provenance)` duplicates, preserving first-occurrence
/// order. Exact duplicates arise legitimately — a diamond of pass-through
/// meta-targets forwards the same producer leaf down two dep entries — and
/// carry no information. The same id under two DISTINCT provenances is NOT a
/// duplicate and both survive (the additive rule).
fn dedup_pairs(deps: Vec<(usize, EdgeProvenance)>) -> Vec<(usize, EdgeProvenance)> {
    let mut seen: BTreeSet<(usize, EdgeProvenance)> = BTreeSet::new();
    deps.into_iter().filter(|p| seen.insert(*p)).collect()
}

#[cfg(test)]
#[path = "tests/unit_graph_tests.rs"]
mod tests;
