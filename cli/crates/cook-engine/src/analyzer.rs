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
pub struct RecipeInfo {
    pub ingredients: Vec<String>,
    pub serves: Vec<String>,
    pub requires: Vec<String>,
}

// ---------------------------------------------------------------------------
// Graph algorithms
// ---------------------------------------------------------------------------

/// Build an adjacency map: for each recipe, the set of recipes it depends on.
///
/// Edges come from explicit `requires` declarations only. Name-reference
/// edges (`{lib}` / `{lib.accessor}`) are produced during codegen by
/// `cook-luagen` and composed into the DAG separately; they do not flow
/// through this adjacency map. Path-string equality between an ingredient
/// and another recipe's cook-output is opaque and does NOT produce an
/// edge — see Cook Standard § 5.6 and rationale B.5.N.
fn build_adjacency<'a>(
    recipes: &'a BTreeMap<String, RecipeInfo>,
) -> Result<BTreeMap<&'a str, BTreeSet<&'a str>>, GraphError> {
    let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (name, info) in recipes {
        let mut recipe_deps = BTreeSet::new();
        for req in &info.requires {
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

/// Find the full dotted prefix for a canonical import path by walking
/// the namespace chain back to root. E.g., root→backend→proto = "backend.proto".
pub fn find_full_prefix(
    namespace_map: &[NamespaceEntry],
    root_dir: &Path,
    canonical_path: &Path,
) -> String {
    let root_canonical =
        std::fs::canonicalize(root_dir).unwrap_or_else(|_| root_dir.to_path_buf());

    // Build reverse map: child_canonical → (parent_canonical, name)
    let mut parent_map: BTreeMap<&Path, (&Path, &str)> = BTreeMap::new();
    for (parent, name, target) in namespace_map {
        parent_map.insert(target.as_path(), (parent.as_path(), name.as_str()));
    }

    let mut segments = Vec::new();
    let mut current = canonical_path;
    loop {
        if current == root_canonical {
            break;
        }
        match parent_map.get(current) {
            Some(&(parent, name)) => {
                segments.push(name.to_string());
                current = parent;
            }
            None => break,
        }
    }

    segments.reverse();
    segments.join(".")
}

// ---------------------------------------------------------------------------
// Workspace-wide recipe registration for `cook test`
// ---------------------------------------------------------------------------

/// Register every recipe in every imported Cookfile in the workspace,
/// regardless of whether it is reachable from any target.
///
/// Used by `cook test` to discover all test_step units across the
/// workspace per docs/superpowers/specs/2026-05-07-test-runner-design.md §4.1.
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
mod workspace_tests {
    use super::*;

    #[test]
    fn register_workspace_for_test_includes_all_recipes_across_imports() {
        use std::collections::BTreeSet;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::write(root.join("Cookfile"), "\
import sub ./sub\n\
recipe build\n\
    cook \"build/r.txt\" { echo > $<out> }\n\
").unwrap();

        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/Cookfile"), "\
recipe inner\n\
    cook \"build/i.txt\" { echo > $<out> }\n\
recipe test_only\n\
    test { true } timeout 5\n\
").unwrap();

        let result = register_workspace_for_test(root).expect("must succeed");
        let names: BTreeSet<_> = result.keys().cloned().collect();
        assert!(names.contains("build"), "root recipe must be present");
        assert!(names.contains("sub.inner"), "imported recipe must be present");
        assert!(
            names.contains("sub.test_only"),
            "test_only is not referenced by any target but must still be registered; got: {names:?}"
        );
    }

    #[test]
    fn register_workspace_for_test_root_only_no_imports() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cookfile"), "recipe alpha\nrecipe beta\n").unwrap();
        let result = register_workspace_for_test(root).expect("must succeed");
        assert!(result.contains_key("alpha"));
        assert!(result.contains_key("beta"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(
        ingredients: Vec<&str>,
        serves: Vec<&str>,
        requires: Vec<&str>,
    ) -> RecipeInfo {
        RecipeInfo {
            ingredients: ingredients.into_iter().map(String::from).collect(),
            serves: serves.into_iter().map(String::from).collect(),
            requires: requires.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn test_single_recipe_no_deps() {
        let mut recipes = BTreeMap::new();
        recipes.insert("build".to_string(), info(vec![], vec![], vec![]));
        let order = topological_sort(&recipes, "build").unwrap();
        assert_eq!(order, vec!["build"]);
    }

    #[test]
    fn test_explicit_requires() {
        let mut recipes = BTreeMap::new();
        recipes.insert("build".to_string(), info(vec![], vec![], vec!["clean"]));
        recipes.insert("clean".to_string(), info(vec![], vec![], vec![]));
        let order = topological_sort(&recipes, "build").unwrap();
        assert_eq!(order, vec!["clean", "build"]);
    }

    #[test]
    fn test_ingredient_serves_string_match_is_opaque() {
        // Historical rule (removed): ingredient-serves string match implied a dep.
        // New rule: only `requires` and name references (outside this module)
        // create cross-recipe edges. This test pins the removal.
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            info(vec!["lib.a"], vec!["app"], vec![]),
        );
        recipes.insert(
            "compile".to_string(),
            info(vec![], vec!["lib.a"], vec![]),
        );
        let order = topological_sort(&recipes, "build").unwrap();
        assert_eq!(order, vec!["build"]);
    }

    #[test]
    fn test_path_match_does_not_imply_dep() {
        // Under the new rule, string equality between an ingredient path and a
        // cook-output path is NOT a cross-recipe edge. Only explicit `: dep` and
        // name references (handled in codegen) create edges.
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            info(vec!["lib.a"], vec!["app"], vec![]),
        );
        recipes.insert(
            "compile".to_string(),
            info(vec![], vec!["lib.a"], vec![]),
        );
        // `build` lists "lib.a" as ingredient; `compile` serves "lib.a".
        // After the rule removal, `compile` MUST NOT be pulled in as a dep
        // of `build`.
        let order = topological_sort(&recipes, "build").unwrap();
        assert_eq!(order, vec!["build"],
            "path-match must not imply dep; got {:?}", order);
    }

    #[test]
    fn test_cycle_detection() {
        let mut recipes = BTreeMap::new();
        recipes.insert("a".to_string(), info(vec![], vec![], vec!["b"]));
        recipes.insert("b".to_string(), info(vec![], vec![], vec!["a"]));
        let result = topological_sort(&recipes, "a");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GraphError::CycleDetected(_)));
    }

    #[test]
    fn test_self_dependency() {
        let mut recipes = BTreeMap::new();
        recipes.insert("loop".to_string(), info(vec![], vec![], vec!["loop"]));
        let result = topological_sort(&recipes, "loop");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GraphError::CycleDetected(_)));
    }

    #[test]
    fn test_unknown_recipe_in_requires() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            info(vec![], vec![], vec!["nonexistent"]),
        );
        let result = topological_sort(&recipes, "build");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            GraphError::UnknownRecipe(name) => assert_eq!(name, "nonexistent"),
            other => panic!("expected UnknownRecipe, got: {other}"),
        }
    }

    #[test]
    fn test_unknown_target_recipe() {
        let recipes: BTreeMap<String, RecipeInfo> = BTreeMap::new();
        let result = topological_sort(&recipes, "missing");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            GraphError::UnknownRecipe(name) => assert_eq!(name, "missing"),
            other => panic!("expected UnknownRecipe, got: {other}"),
        }
    }

    #[test]
    fn test_diamond_dependency() {
        // A depends on B and C; both B and C depend on D
        let mut recipes = BTreeMap::new();
        recipes.insert("a".to_string(), info(vec![], vec![], vec!["b", "c"]));
        recipes.insert("b".to_string(), info(vec![], vec![], vec!["d"]));
        recipes.insert("c".to_string(), info(vec![], vec![], vec!["d"]));
        recipes.insert("d".to_string(), info(vec![], vec![], vec![]));
        let order = topological_sort(&recipes, "a").unwrap();
        assert_eq!(order.len(), 4);
        // d must come before b and c; b and c must come before a
        let pos_d = order.iter().position(|x| x == "d").unwrap();
        let pos_b = order.iter().position(|x| x == "b").unwrap();
        let pos_c = order.iter().position(|x| x == "c").unwrap();
        let pos_a = order.iter().position(|x| x == "a").unwrap();
        assert!(pos_d < pos_b);
        assert!(pos_d < pos_c);
        assert!(pos_b < pos_a);
        assert!(pos_c < pos_a);
    }

    #[test]
    fn test_only_needed_recipes_included() {
        let mut recipes = BTreeMap::new();
        recipes.insert("a".to_string(), info(vec![], vec![], vec!["b"]));
        recipes.insert("b".to_string(), info(vec![], vec![], vec![]));
        recipes.insert("c".to_string(), info(vec![], vec![], vec![]));
        let order = topological_sort(&recipes, "a").unwrap();
        assert_eq!(order, vec!["b", "a"]);
        // "c" should not be included
        assert!(!order.contains(&"c".to_string()));
    }

    #[test]
    fn test_duplicate_edges_are_harmless() {
        // Explicit `requires` is the only source of edges here. The path-match
        // rule is gone (see `test_ingredient_serves_string_match_is_opaque`),
        // so the ingredient/serves overlap below contributes nothing.
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            info(vec!["lib.a"], vec![], vec!["compile"]),
        );
        recipes.insert(
            "compile".to_string(),
            info(vec![], vec!["lib.a"], vec![]),
        );
        let order = topological_sort(&recipes, "build").unwrap();
        assert_eq!(order, vec!["compile", "build"]);
    }

    #[test]
    fn test_namespaced_deps() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "all".to_string(),
            info(vec![], vec![], vec!["backend.build"]),
        );
        recipes.insert(
            "backend.build".to_string(),
            info(vec![], vec![], vec!["backend.proto.generate"]),
        );
        recipes.insert(
            "backend.proto.generate".to_string(),
            info(vec![], vec![], vec![]),
        );
        let order = topological_sort(&recipes, "all").unwrap();
        assert_eq!(
            order,
            vec![
                "backend.proto.generate".to_string(),
                "backend.build".to_string(),
                "all".to_string(),
            ]
        );
    }

    #[test]
    fn test_dependency_edges_single_recipe() {
        let mut recipes = BTreeMap::new();
        recipes.insert("build".to_string(), info(vec![], vec![], vec![]));
        let edges = dependency_edges(&recipes, "build").unwrap();
        assert_eq!(edges.len(), 1);
        assert!(edges["build"].is_empty());
    }

    #[test]
    fn test_dependency_edges_linear_chain() {
        let mut recipes = BTreeMap::new();
        recipes.insert("build".to_string(), info(vec![], vec![], vec!["clean"]));
        recipes.insert("clean".to_string(), info(vec![], vec![], vec![]));
        let edges = dependency_edges(&recipes, "build").unwrap();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges["build"], vec!["clean"]);
        assert!(edges["clean"].is_empty());
    }

    #[test]
    fn test_dependency_edges_diamond() {
        let mut recipes = BTreeMap::new();
        recipes.insert("a".to_string(), info(vec![], vec![], vec!["b", "c"]));
        recipes.insert("b".to_string(), info(vec![], vec![], vec!["d"]));
        recipes.insert("c".to_string(), info(vec![], vec![], vec!["d"]));
        recipes.insert("d".to_string(), info(vec![], vec![], vec![]));
        let edges = dependency_edges(&recipes, "a").unwrap();
        assert_eq!(edges.len(), 4);
        let mut a_deps = edges["a"].clone();
        a_deps.sort();
        assert_eq!(a_deps, vec!["b", "c"]);
        assert_eq!(edges["b"], vec!["d"]);
        assert_eq!(edges["c"], vec!["d"]);
        assert!(edges["d"].is_empty());
    }

    #[test]
    fn test_dependency_edges_excludes_unreachable() {
        let mut recipes = BTreeMap::new();
        recipes.insert("a".to_string(), info(vec![], vec![], vec!["b"]));
        recipes.insert("b".to_string(), info(vec![], vec![], vec![]));
        recipes.insert("c".to_string(), info(vec![], vec![], vec![]));
        let edges = dependency_edges(&recipes, "a").unwrap();
        assert_eq!(edges.len(), 2);
        assert!(!edges.contains_key("c"));
    }

    #[test]
    fn test_dependency_edges_no_implicit_via_serves() {
        // Path-match implicit-dep has been removed (see § 5.6 / B.5.N).
        // Ingredient/serves string overlap MUST NOT produce an edge through
        // `dependency_edges`; unreachable recipes MUST NOT appear in the map.
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            info(vec!["lib.a"], vec!["app"], vec![]),
        );
        recipes.insert(
            "compile".to_string(),
            info(vec![], vec!["lib.a"], vec![]),
        );
        let edges = dependency_edges(&recipes, "build").unwrap();
        assert_eq!(edges.len(), 1);
        assert!(edges["build"].is_empty());
        assert!(!edges.contains_key("compile"));
    }

    #[test]
    fn test_dependency_edges_unknown_target() {
        let recipes: BTreeMap<String, RecipeInfo> = BTreeMap::new();
        let result = dependency_edges(&recipes, "missing");
        assert!(result.is_err());
    }
}
