use super::*;

fn ns(edges: &[(&str, &str, &str)]) -> Vec<NamespaceEntry> {
    edges
        .iter()
        .map(|(p, a, t)| (PathBuf::from(p), a.to_string(), PathBuf::from(t)))
        .collect()
}

#[test]
fn prefix_linear_chain() {
    let map = ns(&[("/r", "backend", "/r/backend"), ("/r/backend", "proto", "/r/proto")]);
    assert_eq!(find_full_prefix(&map, Path::new("/r"), Path::new("/r/proto")), "backend.proto");
    assert_eq!(find_full_prefix(&map, Path::new("/r"), Path::new("/r/backend")), "backend");
}

#[test]
fn prefix_diamond_prefers_root_direct_alias() {
    // Root imports a directly AND b, where b also imports a. The root's own
    // alias must name a, regardless of declaration order (§20.2: the entry
    // Cookfile's imports remain addressable under their declared aliases).
    let direct_first = ns(&[
        ("/r", "a", "/r/a"),
        ("/r", "b", "/r/b"),
        ("/r/b", "a", "/r/a"),
    ]);
    assert_eq!(find_full_prefix(&direct_first, Path::new("/r"), Path::new("/r/a")), "a");

    let direct_last = ns(&[
        ("/r", "b", "/r/b"),
        ("/r/b", "a", "/r/a"),
        ("/r", "a", "/r/a"),
    ]);
    assert_eq!(find_full_prefix(&direct_last, Path::new("/r"), Path::new("/r/a")), "a");
}

#[test]
fn prefix_diamond_equal_depth_breaks_ties_by_declaration_order() {
    // lib is reachable via x and y at the same depth; x is declared first.
    let map = ns(&[
        ("/r", "x", "/r/x"),
        ("/r", "y", "/r/y"),
        ("/r/x", "lib", "/r/lib"),
        ("/r/y", "lib", "/r/lib"),
    ]);
    assert_eq!(find_full_prefix(&map, Path::new("/r"), Path::new("/r/lib")), "x.lib");
}

#[test]
fn prefix_diamond_shortest_chain_wins_over_deeper() {
    // lib is reachable at depth 1 via root's own alias and at depth 2 via b;
    // the shorter chain must win even though b's edge is processed later in
    // a child-first walk.
    let map = ns(&[
        ("/r", "b", "/r/b"),
        ("/r/b", "mid", "/r/mid"),
        ("/r/mid", "lib", "/r/lib"),
        ("/r", "direct", "/r/lib"),
    ]);
    assert_eq!(find_full_prefix(&map, Path::new("/r"), Path::new("/r/lib")), "direct");
}

fn info(
    ingredients: Vec<&str>,
    serves: Vec<&str>,
    requires: Vec<&str>,
) -> RecipeInfo {
    RecipeInfo {
        ingredients: ingredients.into_iter().map(String::from).collect(),
        serves: serves.into_iter().map(String::from).collect(),
        requires: requires.into_iter().map(String::from).collect(),
        orders: vec![],
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
    // Path-match implicit-dep has been removed (see §10.6 / App. C.16.1).
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
