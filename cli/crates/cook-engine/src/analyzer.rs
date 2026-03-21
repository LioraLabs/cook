//! Recipe dependency resolution and topological sort.
//!
//! This module works with recipe names and dependency lists (strings) — it does
//! not depend on any AST types. The graph algorithms (topological sort,
//! dependency resolution) operate on `BTreeMap<String, RecipeInfo>`.

use std::collections::{BTreeMap, BTreeSet};

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
/// Dependencies come from two sources:
/// 1. Explicit `requires` declarations.
/// 2. Implicit file dependencies: if recipe A lists "lib.a" as an ingredient
///    and recipe B serves "lib.a", then A depends on B.
fn build_adjacency<'a>(
    recipes: &'a BTreeMap<String, RecipeInfo>,
) -> Result<BTreeMap<&'a str, BTreeSet<&'a str>>, GraphError> {
    // Build serves_map: file path -> recipe name that produces it
    let mut serves_map: BTreeMap<&str, &str> = BTreeMap::new();
    for (name, info) in recipes {
        for path in &info.serves {
            serves_map.insert(path.as_str(), name.as_str());
        }
    }

    let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (name, info) in recipes {
        let mut recipe_deps = BTreeSet::new();
        for req in &info.requires {
            if !recipes.contains_key(req.as_str()) {
                return Err(GraphError::UnknownRecipe(req.clone()));
            }
            recipe_deps.insert(req.as_str());
        }
        for ingredient in &info.ingredients {
            if let Some(&provider) = serves_map.get(ingredient.as_str()) {
                if provider != name.as_str() {
                    recipe_deps.insert(provider);
                }
            }
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
    fn test_implicit_file_dependency() {
        let mut recipes = BTreeMap::new();
        // "build" needs "lib.a" as ingredient; "compile" serves "lib.a"
        recipes.insert(
            "build".to_string(),
            info(vec!["lib.a"], vec!["app"], vec![]),
        );
        recipes.insert(
            "compile".to_string(),
            info(vec![], vec!["lib.a"], vec![]),
        );
        let order = topological_sort(&recipes, "build").unwrap();
        assert_eq!(order, vec!["compile", "build"]);
    }

    #[test]
    fn test_glob_pattern_does_not_trigger_implicit_dep() {
        // Glob patterns like "src/*.c" should NOT match exact serves paths
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            info(vec!["src/*.c"], vec![], vec![]),
        );
        recipes.insert(
            "gen".to_string(),
            info(vec![], vec!["src/gen.c"], vec![]),
        );
        let order = topological_sort(&recipes, "build").unwrap();
        // "gen" should NOT be included because "src/*.c" != "src/gen.c"
        assert_eq!(order, vec!["build"]);
    }

    #[test]
    fn test_mixed_implicit_and_explicit() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "deploy".to_string(),
            info(vec!["bin/app"], vec![], vec!["test"]),
        );
        recipes.insert(
            "build".to_string(),
            info(vec![], vec!["bin/app"], vec![]),
        );
        recipes.insert("test".to_string(), info(vec![], vec![], vec![]));
        let order = topological_sort(&recipes, "deploy").unwrap();
        // test and build must come before deploy
        assert!(
            order.iter().position(|x| x == "test").unwrap()
                < order.iter().position(|x| x == "deploy").unwrap()
        );
        assert!(
            order.iter().position(|x| x == "build").unwrap()
                < order.iter().position(|x| x == "deploy").unwrap()
        );
        assert_eq!(order.len(), 3);
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
        // Recipe has same dep via both requires and implicit
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
    fn test_dependency_edges_implicit_via_serves() {
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
        assert_eq!(edges.len(), 2);
        assert_eq!(edges["build"], vec!["compile"]);
        assert!(edges["compile"].is_empty());
    }

    #[test]
    fn test_dependency_edges_unknown_target() {
        let recipes: BTreeMap<String, RecipeInfo> = BTreeMap::new();
        let result = dependency_edges(&recipes, "missing");
        assert!(result.is_err());
    }
}
