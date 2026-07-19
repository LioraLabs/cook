//! Pure intersection logic for `cook affected`.
//!
//! Given a set of changed paths, a registered workspace, the recipe-level
//! edge map, and the user's requested closure, returns the subset of the
//! closure whose declared file inputs (or any transitive downstream
//! consumer's inputs) intersect the changed set.

use crate::RegisteredWorkspace;
use cook_contracts::WorkPayload;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

/// Compute the set of recipes from `closure` that should run given the
/// set of file paths that changed since some git reference.
///
/// `project_root` is the workspace root the changed paths are relative to.
///
/// Strict subset of `closure` — affected recipes outside it are not returned.
pub fn compute_affected(
    changed_paths: &BTreeSet<PathBuf>,
    registered: &RegisteredWorkspace,
    edges: &BTreeMap<String, Vec<String>>,
    closure: &BTreeSet<String>,
    project_root: &Path,
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

    // Owner-directory normalisation (COOK-274). An imported recipe's declared
    // inputs are recorded relative to its own Cookfile's directory, while
    // changed paths are workspace-root-relative — exact comparison never
    // matches for any imported recipe. Resolve the recipe's qualified prefix
    // (`web.build` → `web`, root recipes → `""`) to its directory relative to
    // the workspace root and compare the re-rooted spelling too. Recipe names
    // are dot-free at declaration sites, so everything before the last `.` is
    // the import prefix.
    let owner_rel_dir = |name: &str| -> Option<PathBuf> {
        let prefix = name.rsplit_once('.').map_or("", |(p, _)| p);
        let dir = registered.working_dir_by_prefix.get(prefix)?;
        let rel = match dir.strip_prefix(project_root) {
            Ok(r) => r.to_path_buf(),
            // Symlink-divergent spellings (e.g. a canonicalised member dir
            // under a non-canonical root) defeat a plain strip; retry with
            // both sides canonicalised.
            Err(_) => {
                let canon_root = std::fs::canonicalize(project_root).ok()?;
                let canon_dir = std::fs::canonicalize(dir).ok()?;
                canon_dir.strip_prefix(&canon_root).ok()?.to_path_buf()
            }
        };
        if rel.as_os_str().is_empty() {
            None // root Cookfile: inputs are already root-relative
        } else {
            Some(rel)
        }
    };

    let input_matches = |input: &str, rel_dir: Option<&PathBuf>| -> bool {
        let ip = Path::new(input);
        if ip.is_absolute() {
            return ip
                .strip_prefix(project_root)
                .ok()
                .and_then(|s| s.to_str())
                .is_some_and(|s| changed_strs.contains(s));
        }
        match rel_dir {
            // Imported recipe: its relative inputs are owner-relative, so the
            // re-rooted spelling is the only correct comparison — a raw match
            // would false-positive against a same-named root-level file.
            Some(rd) => rd
                .join(input)
                .to_str()
                .is_some_and(|joined| changed_strs.contains(joined)),
            // Root recipe (or unknown prefix): inputs are already
            // root-relative; compare as-is.
            None => changed_strs.contains(input),
        }
    };

    let mut direct_hits: BTreeSet<String> = BTreeSet::new();
    for name in closure {
        let Some(units) = registered.units_by_recipe.get(name) else {
            continue;
        };
        let rel_dir = owner_rel_dir(name);
        'recipe: for unit in &units.units {
            if let WorkPayload::LuaChunk { inputs, .. } = &unit.payload {
                if inputs.iter().any(|i| input_matches(i, rel_dir.as_ref())) {
                    direct_hits.insert(name.clone());
                    break 'recipe;
                }
            }
            if let Some(cm) = &unit.cache_meta {
                if cm.input_paths.iter().any(|i| input_matches(i, rel_dir.as_ref())) {
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
