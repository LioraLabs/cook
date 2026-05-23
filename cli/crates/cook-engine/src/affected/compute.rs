//! Pure intersection logic for `cook affected`.
//!
//! Given a set of changed paths, a registered workspace, the recipe-level
//! edge map, and the user's requested closure, returns the subset of the
//! closure whose declared file inputs (or any transitive downstream
//! consumer's inputs) intersect the changed set.

use crate::RegisteredWorkspace;
use cook_contracts::WorkPayload;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

/// Compute the set of recipes from `closure` that should run given the
/// set of file paths that changed since some git reference.
///
/// Strict subset of `closure` — affected recipes outside it are not returned.
pub fn compute_affected(
    changed_paths: &BTreeSet<PathBuf>,
    registered: &RegisteredWorkspace,
    edges: &BTreeMap<String, Vec<String>>,
    closure: &BTreeSet<String>,
) -> BTreeSet<String> {
    if closure.is_empty() || changed_paths.is_empty() {
        return BTreeSet::new();
    }

    // Phase 1: direct hits — recipes in `closure` whose declared file inputs
    // intersect `changed_paths`. Inputs flow through two places after register-
    // phase: `LuaChunk.inputs` (for chunks born inside a `cook` step body) and
    // `CapturedUnit.cache_meta.input_paths` (for every unit, including raw
    // `Shell` payloads, where the recipe-level `inputs = {...}` declaration
    // is propagated for cache-key derivation). Union both so a recipe with
    // only shell steps is still affected when its declared inputs change.
    let changed_strs: BTreeSet<&str> =
        changed_paths.iter().filter_map(|p| p.to_str()).collect();

    let mut direct_hits: BTreeSet<String> = BTreeSet::new();
    for name in closure {
        let Some(units) = registered.units_by_recipe.get(name) else {
            continue;
        };
        'recipe: for unit in &units.units {
            if let WorkPayload::LuaChunk { inputs, .. } = &unit.payload {
                if inputs.iter().any(|i| changed_strs.contains(i.as_str())) {
                    direct_hits.insert(name.clone());
                    break 'recipe;
                }
            }
            if let Some(cm) = &unit.cache_meta {
                if cm.input_paths.iter().any(|i| changed_strs.contains(i.as_str())) {
                    direct_hits.insert(name.clone());
                    break 'recipe;
                }
            }
        }
    }

    if direct_hits.is_empty() {
        return BTreeSet::new();
    }

    // Phase 2: reverse-DAG BFS. Invert `edges` so we can walk forward from
    // a hit recipe to every consumer.
    let mut rev_edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (recipe, deps) in edges {
        for dep in deps {
            rev_edges.entry(dep.as_str()).or_default().push(recipe.as_str());
        }
    }

    let mut affected: BTreeSet<String> = direct_hits.clone();
    let mut queue: VecDeque<String> = direct_hits.into_iter().collect();
    while let Some(node) = queue.pop_front() {
        if let Some(consumers) = rev_edges.get(node.as_str()) {
            for consumer in consumers {
                if affected.insert(consumer.to_string()) {
                    queue.push_back(consumer.to_string());
                }
            }
        }
    }

    // Phase 3: strict-subset of closure.
    affected.intersection(closure).cloned().collect()
}
