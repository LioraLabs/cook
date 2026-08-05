//! Recipe dependency resolution and topological sort.
//!
//! This module works with recipe names and dependency lists (strings) — it does
//! not depend on any AST types. The graph algorithms (topological sort,
//! dependency resolution) operate on `BTreeMap<String, RecipeInfo>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use thiserror::Error;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum GraphError {
    #[error("dependency cycle detected involving: {0}")]
    CycleDetected(String),
    #[error("unknown recipe: {0}")]
    UnknownRecipe(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    Parse(String),
}

// ---------------------------------------------------------------------------
// RecipeInfo
// ---------------------------------------------------------------------------

/// Metadata about a recipe used for dependency resolution.
///
/// `ingredients` are file paths consumed by the recipe.
/// `serves` are file paths produced by the recipe.
/// `requires` are explicit named dependencies on other recipes.
/// `orders` are names reached only through fine-grained per-unit references
/// (`dep_edges`: `$<B>`, `cook.dep_output`, `cook.dep_order`). They join the
/// build closure and participate in cycle detection exactly as `requires` does,
/// but they never become a whole-recipe barrier — `run` keeps the coarse
/// `RecipeUnits.deps` restricted to the recipe's own declared `requires`.
pub struct RecipeInfo {
    pub ingredients: Vec<String>,
    pub serves: Vec<String>,
    pub requires: Vec<String>,
    pub orders: Vec<String>,
}

// ---------------------------------------------------------------------------
// Graph algorithms
// ---------------------------------------------------------------------------

/// Build an adjacency map: for each recipe, the set of recipes it depends on.
///
/// Edges come from explicit `requires` declarations and from `orders` —
/// names a recipe reaches only through fine-grained per-unit references. Both
/// establish closure membership and are cycle-checked; only `requires` also
/// becomes a coarse whole-recipe barrier (see `RecipeInfo`). Path-string equality between an ingredient
/// and another recipe's cook-output is opaque and does NOT produce an
/// edge — see Cook Standard §10.6 and rationale App. C.16.1.
fn build_adjacency<'a>(
    recipes: &'a BTreeMap<String, RecipeInfo>,
) -> Result<BTreeMap<&'a str, BTreeSet<&'a str>>, GraphError> {
    let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (name, info) in recipes {
        let mut recipe_deps = BTreeSet::new();
        for req in info.requires.iter().chain(info.orders.iter()) {
            if !recipes.contains_key(req.as_str()) {
                return Err(GraphError::UnknownRecipe(req.clone()));
            }
            recipe_deps.insert(req.as_str());
        }
        deps.insert(name.as_str(), recipe_deps);
    }
    Ok(deps)
}

/// Compute `recipe_name -> [dependency_names]` for all recipes reachable from
/// `target`. Returns a `BTreeMap` for deterministic output.
pub fn dependency_edges(
    recipes: &BTreeMap<String, RecipeInfo>,
    target: &str,
) -> Result<BTreeMap<String, Vec<String>>, GraphError> {
    let reachable = topological_sort(recipes, target)?;
    let adjacency = build_adjacency(recipes)?;

    let reachable_set: BTreeSet<&str> = reachable.iter().map(|s| s.as_str()).collect();
    let mut result = BTreeMap::new();
    for name in &reachable {
        let deps = adjacency
            .get(name.as_str())
            .map(|s| {
                let mut v: Vec<String> = s
                    .iter()
                    .filter(|d| reachable_set.contains(**d))
                    .map(|d| d.to_string())
                    .collect();
                v.sort();
                v
            })
            .unwrap_or_default();
        result.insert(name.clone(), deps);
    }
    Ok(result)
}

/// Compute `recipe_name -> [dependency_names]` for all recipes reachable from
/// any of the given `targets`. Merges the dependency graphs of each target.
pub fn dependency_edges_multi(
    recipes: &BTreeMap<String, RecipeInfo>,
    targets: &[String],
) -> Result<BTreeMap<String, Vec<String>>, GraphError> {
    let mut merged = BTreeMap::new();
    for target in targets {
        let edges = dependency_edges(recipes, target)?;
        for (name, deps) in edges {
            let entry = merged.entry(name).or_insert_with(Vec::new);
            for dep in deps {
                if !entry.contains(&dep) {
                    entry.push(dep);
                }
            }
        }
    }
    // Sort deps for deterministic output
    for deps in merged.values_mut() {
        deps.sort();
    }
    Ok(merged)
}

/// Topological sort starting from `target`. Returns recipes in execution order.
/// Only includes recipes reachable from target.
pub fn topological_sort(
    recipes: &BTreeMap<String, RecipeInfo>,
    target: &str,
) -> Result<Vec<String>, GraphError> {
    if !recipes.contains_key(target) {
        return Err(GraphError::UnknownRecipe(target.to_string()));
    }

    let deps = build_adjacency(recipes)?;

    // DFS topological sort
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Unvisited,
        Visiting,
        Visited,
    }

    let mut states: BTreeMap<&str, State> = recipes
        .keys()
        .map(|k| (k.as_str(), State::Unvisited))
        .collect();
    let mut order = Vec::new();

    fn visit<'a>(
        node: &'a str,
        deps: &BTreeMap<&'a str, BTreeSet<&'a str>>,
        states: &mut BTreeMap<&'a str, State>,
        order: &mut Vec<String>,
    ) -> Result<(), GraphError> {
        match states.get(node) {
            Some(State::Visited) => return Ok(()),
            Some(State::Visiting) => return Err(GraphError::CycleDetected(node.to_string())),
            _ => {}
        }
        states.insert(node, State::Visiting);
        if let Some(node_deps) = deps.get(node) {
            for &dep in node_deps {
                visit(dep, deps, states, order)?;
            }
        }
        states.insert(node, State::Visited);
        order.push(node.to_string());
        Ok(())
    }

    visit(target, &deps, &mut states, &mut order)?;
    Ok(order)
}

// ---------------------------------------------------------------------------
// Workspace namespace resolution
// ---------------------------------------------------------------------------

/// An entry in the workspace namespace map: (parent_dir, import_name, imported_dir).
/// All paths are canonical.
pub type NamespaceEntry = (PathBuf, String, PathBuf);

/// Find the full dotted prefix for a canonical import path.
/// E.g., root→backend→proto = "backend.proto".
///
/// When the same directory is reachable through more than one import chain
/// (§11.5 diamond dedup), the canonical prefix is the **shortest alias chain
/// from the workspace root**, with ties broken by declaration order. This is a
/// breadth-first walk from the root, so a directory the root imports directly
/// is always named by the root's own alias — which §20.2 requires to stay
/// addressable — and the name cannot change because an unrelated import was
/// added or reordered elsewhere in the workspace (CS-0147; previously the last
/// edge to insert into a reverse map won, making names declaration-order
/// dependent).
pub fn find_full_prefix(
    namespace_map: &[NamespaceEntry],
    root_dir: &Path,
    canonical_path: &Path,
) -> String {
    let root_canonical =
        std::fs::canonicalize(root_dir).unwrap_or_else(|_| root_dir.to_path_buf());
    if canonical_path == root_canonical {
        return String::new();
    }

    let mut named: BTreeMap<&Path, String> = BTreeMap::new();
    let mut frontier: Vec<(&Path, String)> =
        vec![(root_canonical.as_path(), String::new())];
    while !frontier.is_empty() {
        let mut next: Vec<(&Path, String)> = Vec::new();
        for (dir, prefix) in &frontier {
            for (parent, alias, target) in namespace_map {
                if parent.as_path() != *dir
                    || target.as_path() == root_canonical.as_path()
                    || named.contains_key(target.as_path())
                {
                    continue;
                }
                let child_prefix = if prefix.is_empty() {
                    alias.clone()
                } else {
                    format!("{prefix}.{alias}")
                };
                named.insert(target.as_path(), child_prefix.clone());
                next.push((target.as_path(), child_prefix));
            }
        }
        frontier = next;
    }

    named.get(canonical_path).cloned().unwrap_or_default()
}

#[cfg(test)]
#[path = "tests/analyzer_tests.rs"]
mod tests;
