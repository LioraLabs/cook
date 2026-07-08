use super::compute_affected;
use crate::RegisteredWorkspace;
use cook_contracts::{CacheMeta, CapturedUnit, DepKind, RecipeUnits, WorkPayload};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Build a workspace where the recipe has a single `Shell` unit and its
/// declared inputs live on `cache_meta.input_paths` (the post-register-phase
/// landing site for recipe-level `inputs = {...}` propagated to shell steps).
fn workspace_with_shell(recipe: &str, inputs: &[&str]) -> RegisteredWorkspace {
    let cache_meta = CacheMeta {
        recipe_name: recipe.to_string(),
        project_id: String::new(),
        cookfile_path: String::new(),
        cache_key: String::new(),
        input_paths: inputs.iter().map(|s| s.to_string()).collect(),
        output_paths: vec![],
        command_hash: 0,
        env_contribution: 0,
        consulted_env: BTreeMap::new(),
        discovered_inputs: None,
        seal_keys: Default::default(),
            sharing: Default::default(),
        record: false,
    };
    let unit = CapturedUnit {
        payload: WorkPayload::Shell { cmd: "echo build".into(), line: 1 },
        cache_meta: Some(cache_meta),
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: BTreeMap::new(),
        member: None,
        output_paths: Vec::new(),
    };
    let mut units_by_recipe = BTreeMap::new();
    units_by_recipe.insert(
        recipe.to_string(),
        RecipeUnits {
            recipe_name: recipe.to_string(),
            deps: vec![],
            units: vec![unit],
            step_groups: vec![],
            working_dir: PathBuf::from("."),
            env_vars: BTreeMap::new(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        },
    );
    RegisteredWorkspace {
        names: vec![],
        units_by_recipe,
        probes: BTreeMap::new(),
        final_env_by_cookfile: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix: BTreeMap::new(),
        terminal_outputs: BTreeMap::new(),
    }
}

/// Build a `RegisteredWorkspace` containing one recipe per entry, where each
/// recipe has a single `LuaChunk` unit carrying the given inputs.
fn workspace_with(recipes: &[(&str, &[&str])]) -> RegisteredWorkspace {
    let mut units_by_recipe = BTreeMap::new();
    for (name, inputs) in recipes {
        let unit = CapturedUnit {
            payload: WorkPayload::LuaChunk {
                code: String::new(),
                inputs: inputs.iter().map(|s| s.to_string()).collect(),
                outputs: vec![],
                ingredient_groups: vec![],
                step_kind: cook_contracts::StepKind::Cook,
                is_chore: false,
                line: 0,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: BTreeMap::new(),
            member: None,
            output_paths: Vec::new(),
        };
        units_by_recipe.insert(
            name.to_string(),
            RecipeUnits {
                recipe_name: name.to_string(),
                deps: vec![],
                units: vec![unit],
                step_groups: vec![],
                working_dir: PathBuf::from("."),
                env_vars: BTreeMap::new(),
                terminal_outputs: vec![],
                dep_edges: vec![],
                probes: vec![],
            },
        );
    }
    RegisteredWorkspace {
        names: vec![],
        units_by_recipe,
        probes: BTreeMap::new(),
        final_env_by_cookfile: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix: BTreeMap::new(),
        terminal_outputs: BTreeMap::new(),
    }
}

fn paths(ps: &[&str]) -> BTreeSet<PathBuf> {
    ps.iter().map(PathBuf::from).collect()
}

fn names(ns: &[&str]) -> BTreeSet<String> {
    ns.iter().map(|s| s.to_string()).collect()
}

fn edges_from(spec: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    spec.iter()
        .map(|(k, ds)| (k.to_string(), ds.iter().map(|s| s.to_string()).collect()))
        .collect()
}

#[test]
fn empty_changed_paths_returns_empty() {
    let ws = workspace_with(&[("build", &["src/main.rs"])]);
    let edges = edges_from(&[("build", &[])]);
    let closure = names(&["build"]);
    let got = compute_affected(&paths(&[]), &ws, &edges, &closure);
    assert!(got.is_empty());
}

#[test]
fn empty_closure_returns_empty() {
    let ws = workspace_with(&[("build", &["src/main.rs"])]);
    let edges = edges_from(&[]);
    let closure = names(&[]);
    let got = compute_affected(&paths(&["src/main.rs"]), &ws, &edges, &closure);
    assert!(got.is_empty());
}

#[test]
fn single_recipe_matching_path_returned() {
    let ws = workspace_with(&[("build", &["src/main.rs"])]);
    let edges = edges_from(&[("build", &[])]);
    let closure = names(&["build"]);
    let got = compute_affected(&paths(&["src/main.rs"]), &ws, &edges, &closure);
    assert_eq!(got, names(&["build"]));
}

#[test]
fn single_recipe_non_matching_path_not_returned() {
    let ws = workspace_with(&[("build", &["src/main.rs"])]);
    let edges = edges_from(&[("build", &[])]);
    let closure = names(&["build"]);
    let got = compute_affected(&paths(&["docs/readme.md"]), &ws, &edges, &closure);
    assert!(got.is_empty());
}

#[test]
fn empty_inputs_no_consumers_never_returned() {
    let ws = workspace_with(&[("noop", &[])]);
    let edges = edges_from(&[("noop", &[])]);
    let closure = names(&["noop"]);
    let got = compute_affected(&paths(&["anything.rs"]), &ws, &edges, &closure);
    assert!(got.is_empty());
}

#[test]
fn empty_inputs_with_downstream_consumer_returned_transitively() {
    // `pure` declares no inputs but `app` depends on it; `lib` is the actual
    // source-bearing leaf. Touch `lib/foo.rs` → both `lib` and `pure` and
    // `app` should run (lib is direct hit, app depends on pure which depends
    // on lib).
    let ws = workspace_with(&[
        ("lib", &["lib/foo.rs"]),
        ("pure", &[]),
        ("app", &[]),
    ]);
    let edges = edges_from(&[
        ("app", &["pure"]),
        ("pure", &["lib"]),
        ("lib", &[]),
    ]);
    let closure = names(&["app", "pure", "lib"]);
    let got = compute_affected(&paths(&["lib/foo.rs"]), &ws, &edges, &closure);
    assert_eq!(got, names(&["app", "pure", "lib"]));
}

#[test]
fn diamond_only_one_side_affected() {
    let ws = workspace_with(&[
        ("utils", &["utils/src/x.rs"]),
        ("shared", &["shared/src/y.rs"]),
        ("app", &[]),
    ]);
    let edges = edges_from(&[
        ("app", &["utils", "shared"]),
        ("utils", &[]),
        ("shared", &[]),
    ]);
    let closure = names(&["app", "utils", "shared"]);
    let got = compute_affected(&paths(&["utils/src/x.rs"]), &ws, &edges, &closure);
    assert_eq!(got, names(&["app", "utils"]));
}

#[test]
fn affected_outside_closure_not_returned() {
    let ws = workspace_with(&[
        ("build", &["src/main.rs"]),
        ("lint", &["src/lint.rs"]),
    ]);
    let edges = edges_from(&[("build", &[]), ("lint", &[])]);
    let closure = names(&["build"]);
    let got = compute_affected(&paths(&["src/lint.rs"]), &ws, &edges, &closure);
    assert!(got.is_empty());
}

#[test]
fn same_input_in_multiple_recipes_all_returned() {
    let ws = workspace_with(&[
        ("a", &["shared/x.rs"]),
        ("b", &["shared/x.rs"]),
    ]);
    let edges = edges_from(&[("a", &[]), ("b", &[])]);
    let closure = names(&["a", "b"]);
    let got = compute_affected(&paths(&["shared/x.rs"]), &ws, &edges, &closure);
    assert_eq!(got, names(&["a", "b"]));
}

#[test]
fn recipe_with_multiple_inputs_one_hit_enough() {
    let ws = workspace_with(&[("build", &["src/a.rs", "src/b.rs", "src/c.rs"])]);
    let edges = edges_from(&[("build", &[])]);
    let closure = names(&["build"]);
    let got = compute_affected(&paths(&["src/b.rs"]), &ws, &edges, &closure);
    assert_eq!(got, names(&["build"]));
}

#[test]
fn changed_path_unrelated_to_any_recipe_returns_empty() {
    let ws = workspace_with(&[
        ("build", &["src/main.rs"]),
        ("test", &["tests/test_main.rs"]),
    ]);
    let edges = edges_from(&[("build", &[]), ("test", &[])]);
    let closure = names(&["build", "test"]);
    let got = compute_affected(&paths(&["random.txt"]), &ws, &edges, &closure);
    assert!(got.is_empty());
}

#[test]
fn long_chain_transitive_downstream() {
    let ws = workspace_with(&[
        ("a", &["a.rs"]),
        ("b", &[]),
        ("c", &[]),
        ("d", &[]),
    ]);
    let edges = edges_from(&[
        ("d", &["c"]),
        ("c", &["b"]),
        ("b", &["a"]),
        ("a", &[]),
    ]);
    let closure = names(&["a", "b", "c", "d"]);
    let got = compute_affected(&paths(&["a.rs"]), &ws, &edges, &closure);
    assert_eq!(got, names(&["a", "b", "c", "d"]));
}

#[test]
fn shell_payload_with_cache_meta_inputs_is_a_direct_hit() {
    // A recipe whose only unit is a `Shell` payload with the recipe-level
    // inputs propagated onto `cache_meta.input_paths` (the post-register
    // landing site for `inputs = {...}` on non-Lua-chunk units). Must hit.
    let ws = workspace_with_shell("build", &["src/main.rs"]);
    let edges = edges_from(&[("build", &[])]);
    let closure = names(&["build"]);
    let got = compute_affected(&paths(&["src/main.rs"]), &ws, &edges, &closure);
    assert_eq!(got, names(&["build"]));
}

#[test]
fn shell_payload_with_cache_meta_inputs_unrelated_path_misses() {
    let ws = workspace_with_shell("build", &["src/main.rs"]);
    let edges = edges_from(&[("build", &[])]);
    let closure = names(&["build"]);
    let got = compute_affected(&paths(&["docs/x.md"]), &ws, &edges, &closure);
    assert!(got.is_empty());
}
