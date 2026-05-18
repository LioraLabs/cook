//! Two-tier wave grouping from explicit and inferred dependency edges.
//!
//! - **Explicit deps** (`: dep`) create wave boundaries. If B explicitly depends
//!   on A, A must complete in an earlier wave before B starts.
//! - **Inferred deps** (`{dep}`) merge recipes into the same wave. If B
//!   references `{A}`, A and B belong to the same wave with A registered first.
//! - Transitive inferred deps collapse: if C->B->A (all inferred), all three
//!   land in a single wave.
//! - Inferred merging respects wave boundaries: if A is constrained to wave N
//!   by explicit deps, B using `{A}` joins wave N rather than pulling A earlier.
//!
//! **Historical note (SHI-222 Phase 4 Task 4.4).** This module was originally
//! in `cook-engine` and drove the per-wave register / DAG / execute loop. The
//! engine now walks a single unified work-unit DAG across every reachable
//! recipe (no waves), so this module is retained here purely as a viewer-side
//! presentation grouping: the TUI / JSON viewer groups recipes into "waves"
//! purely for display, with no runtime semantics. Phase 5 will likely replace
//! this with a topology projection derived directly from the unified DAG.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A wave: a set of recipes to register and execute together.
#[derive(Debug, Clone)]
pub struct Wave {
    /// Recipes in this wave, in toposorted order (dependencies first).
    pub recipes: Vec<String>,
}

/// Compute wave assignments from explicit and inferred dep edges.
///
/// Returns waves in execution order (wave 0 first).
///
/// - `explicit_deps`: recipe -> recipes it explicitly depends on (wave boundaries).
/// - `inferred_deps`: recipe -> recipes it references via `{dep}` (same-wave merging).
/// - `all_recipes`: the full set of recipe names.
pub fn compute_waves(
    explicit_deps: &BTreeMap<String, Vec<String>>,
    inferred_deps: &BTreeMap<String, Vec<String>>,
    all_recipes: &BTreeSet<String>,
) -> Result<Vec<Wave>, String> {
    // Step 1: Build inferred-dep connected components via BFS following
    //         inferred edges in both directions.
    let groups = build_inferred_groups(inferred_deps, all_recipes);

    // Map each recipe to its group id.
    let mut recipe_to_group: BTreeMap<String, usize> = BTreeMap::new();
    for (group_id, members) in groups.iter().enumerate() {
        for recipe in members {
            recipe_to_group.insert(recipe.clone(), group_id);
        }
    }

    // Step 2: Build inter-group edges from explicit deps.
    let mut group_deps: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for gid in 0..groups.len() {
        group_deps.entry(gid).or_default();
    }

    for (recipe, deps) in explicit_deps {
        let Some(&from_group) = recipe_to_group.get(recipe) else {
            continue;
        };
        for dep in deps {
            let Some(&to_group) = recipe_to_group.get(dep) else {
                continue;
            };
            if from_group != to_group {
                group_deps.entry(from_group).or_default().insert(to_group);
            }
        }
    }

    // Step 3: Toposort groups and assign wave levels.
    //         Groups with no inter-group ordering constraints share a wave.
    //         A group's wave level = max(wave_level of its dependencies) + 1,
    //         or 0 if it has no dependencies.
    let group_order = toposort_groups(&group_deps, groups.len())?;

    let mut group_wave: BTreeMap<usize, usize> = BTreeMap::new();
    for &gid in &group_order {
        let level = group_deps
            .get(&gid)
            .map(|deps| {
                deps.iter()
                    .map(|d| group_wave.get(d).copied().unwrap_or(0) + 1)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        group_wave.insert(gid, level);
    }

    // Step 4: Collect groups by wave level, toposort recipes within each
    //         group, then concatenate all groups in a wave.
    let max_wave = group_wave.values().copied().max().unwrap_or(0);
    let mut waves = Vec::new();
    for wave_level in 0..=max_wave {
        let mut wave_recipes = Vec::new();
        // Iterate groups in toposorted order so output is deterministic.
        for &gid in &group_order {
            if group_wave.get(&gid).copied() != Some(wave_level) {
                continue;
            }
            let sorted = toposort_within_group(&groups[gid], inferred_deps)?;
            wave_recipes.extend(sorted);
        }
        if !wave_recipes.is_empty() {
            waves.push(Wave {
                recipes: wave_recipes,
            });
        }
    }

    Ok(waves)
}

/// Build connected components from inferred deps (both directions).
fn build_inferred_groups(
    inferred_deps: &BTreeMap<String, Vec<String>>,
    all_recipes: &BTreeSet<String>,
) -> Vec<BTreeSet<String>> {
    // Build undirected adjacency for inferred edges.
    let mut adj: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for recipe in all_recipes {
        adj.entry(recipe.clone()).or_default();
    }
    for (recipe, deps) in inferred_deps {
        for dep in deps {
            adj.entry(recipe.clone()).or_default().insert(dep.clone());
            adj.entry(dep.clone()).or_default().insert(recipe.clone());
        }
    }

    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut groups: Vec<BTreeSet<String>> = Vec::new();

    for recipe in all_recipes {
        if visited.contains(recipe) {
            continue;
        }
        let mut component = BTreeSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(recipe.clone());
        visited.insert(recipe.clone());

        while let Some(current) = queue.pop_front() {
            component.insert(current.clone());
            if let Some(neighbors) = adj.get(&current) {
                for neighbor in neighbors {
                    if !visited.contains(neighbor) {
                        visited.insert(neighbor.clone());
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }
        groups.push(component);
    }

    groups
}

/// Kahn's algorithm toposort on group-level DAG.
///
/// `group_deps` maps group_id -> set of group_ids it depends on.
fn toposort_groups(
    group_deps: &BTreeMap<usize, BTreeSet<usize>>,
    num_groups: usize,
) -> Result<Vec<usize>, String> {
    // in_degree[g] = number of groups that g depends on (i.e. edges into g
    // in the "must come after" sense).
    let mut in_degree: BTreeMap<usize, usize> = BTreeMap::new();
    for gid in 0..num_groups {
        in_degree.insert(gid, 0);
    }

    // forward[dep] = set of groups that depend on dep (dep -> dependent).
    let mut forward: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (&group, deps) in group_deps {
        for &dep in deps {
            forward.entry(dep).or_default().insert(group);
            *in_degree.entry(group).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<usize> = VecDeque::new();
    for (&gid, &deg) in &in_degree {
        if deg == 0 {
            queue.push_back(gid);
        }
    }

    let mut order = Vec::new();
    while let Some(gid) = queue.pop_front() {
        order.push(gid);
        if let Some(dependents) = forward.get(&gid) {
            for &dependent in dependents {
                let deg = in_degree.get_mut(&dependent).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(dependent);
                }
            }
        }
    }

    if order.len() != num_groups {
        return Err("cycle detected in explicit dependencies between wave groups".to_string());
    }

    Ok(order)
}

/// Toposort recipes within a single group using directed inferred edges.
///
/// If recipe A has inferred dep on B, B must come before A in the output.
/// Detects cycles and returns an error if found.
fn toposort_within_group(
    members: &BTreeSet<String>,
    inferred_deps: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>, String> {
    if members.len() == 1 {
        return Ok(members.iter().cloned().collect());
    }

    // Build directed edges within this group: recipe -> deps (within group).
    let mut deps_in_group: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for member in members {
        deps_in_group.entry(member.as_str()).or_default();
    }

    for member in members {
        if let Some(deps) = inferred_deps.get(member) {
            for dep in deps {
                if members.contains(dep) {
                    deps_in_group
                        .entry(member.as_str())
                        .or_default()
                        .insert(dep.as_str());
                }
            }
        }
    }

    // Kahn's algorithm.
    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
    for &member in deps_in_group.keys() {
        in_degree.insert(member, 0);
    }

    // Forward edges: dep -> dependents.
    let mut forward: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (&recipe, deps) in &deps_in_group {
        for &dep in deps {
            forward.entry(dep).or_default().insert(recipe);
            *in_degree.entry(recipe).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<&str> = VecDeque::new();
    for (&member, &deg) in &in_degree {
        if deg == 0 {
            queue.push_back(member);
        }
    }

    let mut order = Vec::new();
    while let Some(current) = queue.pop_front() {
        order.push(current.to_string());
        if let Some(nexts) = forward.get(current) {
            for &next in nexts {
                let deg = in_degree.get_mut(next).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(next);
                }
            }
        }
    }

    if order.len() != members.len() {
        let remaining: Vec<&str> = members
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !order.iter().any(|o| o == s))
            .collect();
        return Err(format!(
            "cycle detected in inferred dependencies among: {}",
            remaining.join(", ")
        ));
    }

    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_deps_single_wave() {
        let explicit = BTreeMap::new();
        let inferred = BTreeMap::new();
        let recipes = BTreeSet::from(["a".into(), "b".into(), "c".into()]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].recipes.len(), 3);
    }

    #[test]
    fn test_explicit_dep_creates_wave_boundary() {
        let mut explicit = BTreeMap::new();
        explicit.insert("run".to_string(), vec!["app".to_string()]);
        let inferred = BTreeMap::new();
        let recipes = BTreeSet::from(["app".into(), "run".into()]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 2);
        assert!(waves[0].recipes.contains(&"app".to_string()));
        assert!(waves[1].recipes.contains(&"run".to_string()));
    }

    #[test]
    fn test_inferred_dep_same_wave() {
        let explicit = BTreeMap::new();
        let mut inferred = BTreeMap::new();
        inferred.insert(
            "app".to_string(),
            vec!["libmath".to_string(), "libstr".to_string()],
        );
        let recipes = BTreeSet::from(["libmath".into(), "libstr".into(), "app".into()]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 1);
        let app_pos = waves[0].recipes.iter().position(|r| r == "app").unwrap();
        let math_pos = waves[0]
            .recipes
            .iter()
            .position(|r| r == "libmath")
            .unwrap();
        let str_pos = waves[0]
            .recipes
            .iter()
            .position(|r| r == "libstr")
            .unwrap();
        assert!(math_pos < app_pos);
        assert!(str_pos < app_pos);
    }

    #[test]
    fn test_transitive_inferred_deps_collapse() {
        let explicit = BTreeMap::new();
        let mut inferred = BTreeMap::new();
        inferred.insert("core".to_string(), vec!["protos".to_string()]);
        inferred.insert("server".to_string(), vec!["core".to_string()]);
        let recipes = BTreeSet::from(["protos".into(), "core".into(), "server".into()]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 1);
        let order: Vec<&str> = waves[0].recipes.iter().map(|s| s.as_str()).collect();
        assert!(
            order.iter().position(|&r| r == "protos").unwrap()
                < order.iter().position(|&r| r == "core").unwrap()
        );
        assert!(
            order.iter().position(|&r| r == "core").unwrap()
                < order.iter().position(|&r| r == "server").unwrap()
        );
    }

    #[test]
    fn test_mixed_explicit_and_inferred() {
        // libmath, libstr, app uses {libmath} {libstr}, run: app
        let mut explicit = BTreeMap::new();
        explicit.insert("run".to_string(), vec!["app".to_string()]);
        let mut inferred = BTreeMap::new();
        inferred.insert(
            "app".to_string(),
            vec!["libmath".to_string(), "libstr".to_string()],
        );
        let recipes = BTreeSet::from([
            "libmath".into(),
            "libstr".into(),
            "app".into(),
            "run".into(),
        ]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0].recipes.len(), 3); // libmath, libstr, app
        assert_eq!(waves[1].recipes, vec!["run".to_string()]);
    }

    #[test]
    fn test_inferred_cycle_detected() {
        let explicit = BTreeMap::new();
        let mut inferred = BTreeMap::new();
        inferred.insert("a".to_string(), vec!["b".to_string()]);
        inferred.insert("b".to_string(), vec!["a".to_string()]);
        let recipes = BTreeSet::from(["a".into(), "b".into()]);
        let result = compute_waves(&explicit, &inferred, &recipes);
        assert!(result.is_err());
    }

    #[test]
    fn test_inferred_respects_wave_boundaries() {
        // setup has no deps, libmath depends explicitly on setup,
        // app uses {libmath} (inferred)
        // Expected: wave 1 = setup, wave 2 = libmath + app
        let mut explicit = BTreeMap::new();
        explicit.insert("libmath".to_string(), vec!["setup".to_string()]);
        let mut inferred = BTreeMap::new();
        inferred.insert("app".to_string(), vec!["libmath".to_string()]);
        let recipes = BTreeSet::from(["setup".into(), "libmath".into(), "app".into()]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 2);
        assert!(waves[0].recipes.contains(&"setup".to_string()));
        assert!(waves[1].recipes.contains(&"libmath".to_string()));
        assert!(waves[1].recipes.contains(&"app".to_string()));
    }
}
