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

use std::collections::BTreeMap;

use cook_contracts::{CapturedUnit, DepKind, RecipeUnits, WorkPayload};
use cook_dag::Dag;

use crate::WorkNode;

/// Build a `Dag<WorkNode>` from a topologically-sorted list of `RecipeUnits`.
pub fn build_dag(recipe_units: Vec<RecipeUnits>) -> Dag<WorkNode> {
    let mut dag = Dag::new();

    // Map from recipe name -> its final barrier (leaf node ids).
    let mut recipe_leaves: BTreeMap<String, Vec<usize>> = BTreeMap::new();

    for ru in &recipe_units {
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
            }
        }

        // Record this recipe's final barrier as its leaves.
        recipe_leaves.insert(ru.recipe_name.clone(), barrier);
    }

    dag
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
                },
                CapturedUnit {
                    payload: shell("echo b"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                },
            ],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
        };
        let dag = build_dag(vec![units]);
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
                },
                CapturedUnit {
                    payload: shell("gcc -c b.c"),
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                },
                CapturedUnit {
                    payload: shell("ar rcs lib.a"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                },
            ],
            step_groups: vec![vec![0, 1]],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
        };
        let dag = build_dag(vec![units]);
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
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
        };
        let build = RecipeUnits {
            recipe_name: "build".into(),
            deps: vec!["setup".into()],
            units: vec![CapturedUnit {
                payload: shell("gcc main.c"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
        };
        let dag = build_dag(vec![setup, build]);
        assert_eq!(dag.len(), 2);
        // build's unit should depend on setup's unit
        assert_eq!(dag.node(1).remaining_deps(), 1);
    }

    #[test]
    fn test_build_empty() {
        let dag = build_dag(vec![]);
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
                },
                CapturedUnit {
                    payload: shell("gcc -c mul.c"),
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                },
                CapturedUnit {
                    payload: shell("ar rcs libmath.a"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                },
            ],
            step_groups: vec![vec![0, 1]],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec!["libmath.a".into()],
            dep_edges: vec![],
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
                },
                CapturedUnit {
                    payload: shell("gcc -o app main.o libmath.a"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                },
            ],
            step_groups: vec![vec![0]],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec!["app".into()],
            dep_edges: vec![(1, "libmath".into())], // unit 1 (link) depends on libmath
        };

        let dag = build_dag(vec![libmath, app]);
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
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
        };
        let build = RecipeUnits {
            recipe_name: "build".into(),
            deps: vec!["setup".into()],
            units: vec![CapturedUnit {
                payload: shell("gcc main.c"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
            }],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
        };
        let dag = build_dag(vec![setup, build]);
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
                },
                CapturedUnit {
                    payload: shell("echo real work"),
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                },
            ],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
        };
        let dag = build_dag(vec![units]);
        assert_eq!(dag.len(), 2);
        // First node is presatisfied (no payload)
        assert!(dag.node(0).payload().payload.is_none());
        // Second node has payload
        assert!(dag.node(1).payload().payload.is_some());
    }
}
