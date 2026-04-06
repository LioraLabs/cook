//! Build a `Dag<WorkNode>` from a topologically-sorted list of `RecipeUnits`.
//!
//! Within-recipe wiring:
//! - `DepKind::Sequential` units depend on the current barrier (the set of
//!   nodes that must finish before the next sequential unit can start).
//! - `DepKind::StepGroup(idx)` units all share the same barrier (the one
//!   active when the group started). When the last member of a group is
//!   processed, all group members become the new barrier.
//!
//! Cross-recipe wiring:
//! - A recipe's root units (those with no within-recipe deps) additionally
//!   depend on the leaf barrier of every recipe listed in `deps`.

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
            // Cross-recipe deps only apply to root units (units with no
            // within-recipe deps).
            let all_deps = if within_deps.is_empty() {
                cross_deps.clone()
            } else {
                within_deps
            };

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

            let dag_id = dag.add_node(work_node, &all_deps);

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
    use std::sync::atomic::Ordering;

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
        assert_eq!(dag.node(0).remaining_deps.load(Ordering::Relaxed), 0);
        assert_eq!(dag.node(1).remaining_deps.load(Ordering::Relaxed), 1);
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
        assert_eq!(dag.node(0).remaining_deps.load(Ordering::Relaxed), 0);
        assert_eq!(dag.node(1).remaining_deps.load(Ordering::Relaxed), 0);
        // Sequential unit after group depends on both group members
        assert_eq!(dag.node(2).remaining_deps.load(Ordering::Relaxed), 2);
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
        assert_eq!(dag.node(1).remaining_deps.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_build_empty() {
        let dag = build_dag(vec![]);
        assert!(dag.is_empty());
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
        assert!(dag.node(0).payload.payload.is_none());
        // Second node has payload
        assert!(dag.node(1).payload.payload.is_some());
    }
}
