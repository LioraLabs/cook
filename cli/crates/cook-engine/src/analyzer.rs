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

/// Workspace descriptor — the minimal data cook-engine needs from the workspace
/// to perform namespace resolution. cook-cli builds this from the Workspace struct.
pub struct WorkspaceLayout {
    /// Canonical path to the root Cookfile's directory.
    pub root_dir: PathBuf,
    /// Recipes from the root Cookfile (name → deps, no prefix).
    pub root_recipes: Vec<(String, Vec<String>)>,
    /// Imported Cookfiles: (canonical_dir, recipes as name→deps).
    pub imported_recipes: Vec<(PathBuf, Vec<(String, Vec<String>)>)>,
    /// Namespace map: (parent_canonical, import_name, imported_canonical).
    pub namespace_map: Vec<NamespaceEntry>,
}

/// Build fully-namespaced recipe info from a workspace layout.
/// Root recipes keep their names; imported recipes get dotted prefixes.
/// Dependencies are resolved relative to each Cookfile's namespace.
pub fn build_workspace_recipe_info(
    layout: &WorkspaceLayout,
) -> BTreeMap<String, RecipeInfo> {
    let mut all = BTreeMap::new();

    // Root recipes
    for (name, deps) in &layout.root_recipes {
        let resolved_deps: Vec<String> = deps
            .iter()
            .map(|dep| resolve_dep_namespace(&layout.namespace_map, &layout.root_dir, &layout.root_dir, dep, ""))
            .collect();
        all.insert(
            name.clone(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: resolved_deps,
                orders: vec![],
            },
        );
    }

    // Imported recipes
    for (canonical_dir, recipes) in &layout.imported_recipes {
        let prefix = find_full_prefix(&layout.namespace_map, &layout.root_dir, canonical_dir);
        for (name, deps) in recipes {
            let resolved_deps: Vec<String> = deps
                .iter()
                .map(|dep| {
                    resolve_dep_namespace(&layout.namespace_map, &layout.root_dir, canonical_dir, dep, &prefix)
                })
                .collect();
            all.insert(
                format!("{prefix}.{name}"),
                RecipeInfo {
                    ingredients: vec![],
                    serves: vec![],
                    requires: resolved_deps,
                    orders: vec![],
                },
            );
        }
    }

    all
}

/// Resolve a dependency name to its fully namespaced form.
/// Checks if the dep references an import visible from `from_dir`.
/// Uses `root_dir` (the workspace root) for computing the absolute prefix,
/// ensuring consistent naming even when the same import is reached from
/// multiple parents (diamond dependencies).
fn resolve_dep_namespace(
    namespace_map: &[NamespaceEntry],
    root_dir: &Path,
    from_dir: &Path,
    dep: &str,
    current_prefix: &str,
) -> String {
    if let Some((target_dir, recipe_name)) =
        resolve_namespaced_dep(namespace_map, from_dir, dep)
    {
        let prefix = find_full_prefix(namespace_map, root_dir, &target_dir);
        format!("{prefix}.{recipe_name}")
    } else {
        if current_prefix.is_empty() {
            dep.to_string()
        } else {
            format!("{current_prefix}.{dep}")
        }
    }
}

/// Resolve "proto.generate" from a parent dir to (canonical_import_dir, recipe_name).
fn resolve_namespaced_dep(
    namespace_map: &[NamespaceEntry],
    parent_dir: &Path,
    dep: &str,
) -> Option<(PathBuf, String)> {
    let dot_pos = dep.find('.')?;
    let import_name = &dep[..dot_pos];
    let recipe_name = &dep[dot_pos + 1..];

    let parent_canonical =
        std::fs::canonicalize(parent_dir).unwrap_or_else(|_| parent_dir.to_path_buf());

    for (parent, name, target) in namespace_map {
        if parent == &parent_canonical && name == import_name {
            return Some((target.clone(), recipe_name.to_string()));
        }
    }
    None
}

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

// ---------------------------------------------------------------------------
// Workspace-wide recipe registration for `cook test`
// ---------------------------------------------------------------------------

/// Register every recipe in every imported Cookfile in the workspace,
/// regardless of whether it is reachable from any target.
///
/// Used by `cook test` to discover all test_step units across the
/// workspace.
pub fn register_workspace_for_test(
    project_root: &Path,
) -> Result<BTreeMap<String, RecipeInfo>, GraphError> {
    let synthetic_targets = collect_all_recipe_names_in_workspace(project_root)?;
    // Build a RecipeInfo map for every recipe discovered.  We populate
    // `requires` from the AST `deps` field so cross-recipe ordering is
    // preserved for the slice computation in Phase 4, but `ingredients`
    // and `serves` are left empty — the test runner only needs the names
    // and dependency graph, not file-path metadata.
    build_recipe_info_for_targets(project_root, &synthetic_targets)
}

/// Collect every fully-qualified recipe name reachable from `project_root/Cookfile`,
/// walking imports recursively (BFS, dedup by canonical path).
///
/// Tree-relative imports (`./path`) resolve relative to the importing directory.
/// Sigil-anchored imports (`//path`) resolve relative to `project_root`.
fn collect_all_recipe_names_in_workspace(
    project_root: &Path,
) -> Result<Vec<String>, GraphError> {
    let root_canon = std::fs::canonicalize(project_root)
        .map_err(|e| GraphError::Io(format!("cannot resolve project root: {e}")))?;

    let mut all_targets: Vec<String> = Vec::new();
    // Stack entries: (dotted prefix for this Cookfile, canonical dir path)
    let mut to_visit: Vec<(String, PathBuf)> = vec![(String::new(), root_canon.clone())];
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();

    while let Some((prefix, dir)) = to_visit.pop() {
        if !visited.insert(dir.clone()) {
            continue;
        }
        let cookfile_path = dir.join("Cookfile");
        if !cookfile_path.exists() {
            continue;
        }
        let source = std::fs::read_to_string(&cookfile_path)
            .map_err(|e| GraphError::Io(format!("cannot read {}: {e}", cookfile_path.display())))?;
        let parsed = cook_lang::parse(&source)
            .map_err(|e| GraphError::Parse(format!("{}: {e}", cookfile_path.display())))?;

        // Collect recipe names with the current prefix.
        for recipe in &parsed.recipes {
            let qualified = if prefix.is_empty() {
                recipe.name.clone()
            } else {
                format!("{}.{}", prefix, recipe.name)
            };
            all_targets.push(qualified);
        }

        // Enqueue imports.
        for imp in &parsed.imports {
            let import_dir = match &imp.path {
                cook_lang::ast::ImportPath::Tree(p) => dir.join(p),
                cook_lang::ast::ImportPath::Sigil(p) => root_canon.join(p),
            };
            let import_canon = match std::fs::canonicalize(&import_dir) {
                Ok(c) => c,
                Err(_) => continue, // best-effort; missing dirs are ignored
            };
            let import_prefix = if prefix.is_empty() {
                imp.name.clone()
            } else {
                format!("{}.{}", prefix, imp.name)
            };
            to_visit.push((import_prefix, import_canon));
        }
    }

    Ok(all_targets)
}

/// Build a `BTreeMap<String, RecipeInfo>` for every qualified name in
/// `targets`.  Dependencies are resolved from the AST so that the
/// topological sort / slice computation in Phase 4 works correctly.
///
/// This is a second, focused pass: we re-walk the import graph to find
/// each recipe's raw deps, then namespace-qualify them the same way
/// `build_workspace_recipe_info` does.
fn build_recipe_info_for_targets(
    project_root: &Path,
    targets: &[String],
) -> Result<BTreeMap<String, RecipeInfo>, GraphError> {
    // Collect all recipe infos (name → deps) from the full import walk.
    let root_canon = std::fs::canonicalize(project_root)
        .map_err(|e| GraphError::Io(format!("cannot resolve project root: {e}")))?;

    let mut result: BTreeMap<String, RecipeInfo> = BTreeMap::new();
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    // namespace_map: (parent_canonical, import_name, imported_canonical)
    let mut namespace_map: Vec<(PathBuf, String, PathBuf)> = Vec::new();

    // First pass: collect namespace_map and raw recipe deps.
    let mut raw_entries: Vec<(String, Vec<String>)> = Vec::new(); // (qualified_name, raw_deps)

    let mut stack: Vec<(String, PathBuf)> = vec![(String::new(), root_canon.clone())];
    while let Some((prefix, dir)) = stack.pop() {
        if !visited.insert(dir.clone()) {
            continue;
        }
        let cookfile_path = dir.join("Cookfile");
        if !cookfile_path.exists() {
            continue;
        }
        let source = std::fs::read_to_string(&cookfile_path)
            .map_err(|e| GraphError::Io(format!("cannot read {}: {e}", cookfile_path.display())))?;
        let parsed = cook_lang::parse(&source)
            .map_err(|e| GraphError::Parse(format!("{}: {e}", cookfile_path.display())))?;

        for recipe in &parsed.recipes {
            let qualified = if prefix.is_empty() {
                recipe.name.clone()
            } else {
                format!("{}.{}", prefix, recipe.name)
            };
            raw_entries.push((qualified, recipe.deps.clone()));
        }

        for imp in &parsed.imports {
            let import_dir = match &imp.path {
                cook_lang::ast::ImportPath::Tree(p) => dir.join(p),
                cook_lang::ast::ImportPath::Sigil(p) => root_canon.join(p),
            };
            let import_canon = match std::fs::canonicalize(&import_dir) {
                Ok(c) => c,
                Err(_) => continue,
            };
            namespace_map.push((dir.clone(), imp.name.clone(), import_canon.clone()));
            let import_prefix = if prefix.is_empty() {
                imp.name.clone()
            } else {
                format!("{}.{}", prefix, imp.name)
            };
            stack.push((import_prefix, import_canon));
        }
    }

    // Build the set of requested targets for fast lookup.
    let target_set: BTreeSet<&str> = targets.iter().map(|s| s.as_str()).collect();

    // Second pass: for each (qualified_name, raw_deps), resolve the raw deps
    // into fully-qualified names.  We use `build_workspace_recipe_info`'s
    // approach: walk namespace_map to resolve alias.recipe references.
    for (qualified, raw_deps) in &raw_entries {
        if !target_set.contains(qualified.as_str()) {
            continue;
        }
        // Determine the dir that owns this recipe so we can namespace-resolve
        // its raw deps correctly.
        let prefix = if let Some(dot) = qualified.rfind('.') {
            &qualified[..dot]
        } else {
            ""
        };
        let owner_dir = find_dir_for_prefix(&namespace_map, &root_canon, prefix);

        let resolved_deps: Vec<String> = raw_deps
            .iter()
            .map(|dep| {
                resolve_dep_namespace(&namespace_map, &root_canon, &owner_dir, dep, prefix)
            })
            .collect();

        result.insert(
            qualified.clone(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: resolved_deps,
                orders: vec![],
            },
        );
    }

    Ok(result)
}

/// Find the canonical directory for a given dotted prefix by walking the
/// namespace map from the root.  Returns `root_canon` for the empty prefix.
fn find_dir_for_prefix(
    namespace_map: &[(PathBuf, String, PathBuf)],
    root_canon: &Path,
    prefix: &str,
) -> PathBuf {
    if prefix.is_empty() {
        return root_canon.to_path_buf();
    }
    let segments: Vec<&str> = prefix.split('.').collect();
    let mut current = root_canon.to_path_buf();
    for seg in &segments {
        let mut found = false;
        for (parent, name, target) in namespace_map {
            if parent == &current && name.as_str() == *seg {
                current = target.clone();
                found = true;
                break;
            }
        }
        if !found {
            // Prefix not found in namespace_map — return current best guess.
            break;
        }
    }
    current
}

#[cfg(test)]
#[path = "tests/workspace_tests.rs"]
mod workspace_tests;

#[cfg(test)]
#[path = "tests/analyzer_tests.rs"]
mod tests;
