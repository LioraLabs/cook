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

/// Build a `Dag<WorkNode>` from a topologically-sorted list of `RecipeUnits`.
///
/// Performs plan-time validation that no two non-dep-related recipes declare
/// the same canonical output path. If two recipes with no recipe-level
/// dependency edge between them (in either direction) both claim the same
/// `working_dir.join(output_path)`, this returns
/// [`EngineError::OutputCollision`] before any work is dispatched. This
/// prevents silent races under `--jobs > 1` where two recipes write the same
/// artifact concurrently with no enforced ordering.
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

        // dag_id_by_unit_idx: populated as each unit is added; lets us resolve
        // probe-unit dag IDs when wiring CapturedUnit.probes edges.
        let mut dag_id_by_unit_idx: BTreeMap<usize, usize> = BTreeMap::new();
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
            for req_key in &unit.probes {
                if let Some(&probe_unit_idx) = probe_unit_index_by_key.get(req_key) {
                    if let Some(&probe_dag_id) = dag_id_by_unit_idx.get(&probe_unit_idx) {
                        if !all_deps.contains(&probe_dag_id) {
                            all_deps.push(probe_dag_id);
                        }
                    }
                    // If the probe dag_id isn't known yet (probe declared after consumer
                    // in units), the edge is silently skipped. In practice this cannot
                    // happen: engine.rs validates all probe keys exist as registered
                    // probes, and probes are pushed into units when cook.probe is called
                    // (before cook.add_unit in the same register block).
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
