use super::*;
use crate::registered_workspace::RegisteredWorkspace;
use cook_contracts::RecipeUnits;
use cook_register::{RecipeKind, RegisteredRecipePub, RegistrationSource};
use std::path::PathBuf;

fn make_name(name: &str, requires: &[&str]) -> RegisteredRecipePub {
    RegisteredRecipePub {
        name: name.to_string(),
        source: RegistrationSource::Static { line: 1 },
        kind: RecipeKind::Recipe,
        requires: requires.iter().map(|s| s.to_string()).collect(),
        params: Vec::new(),
        origin: None,
    }
}

fn empty_ws() -> RegisteredWorkspace {
    RegisteredWorkspace {
        warnings: Vec::new(),
        names: Vec::new(),
        units_by_recipe: BTreeMap::new(),
        probes: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix: BTreeMap::new(),
        terminal_outputs: BTreeMap::new(),
    }
}

#[test]
fn empty_workspace_yields_empty_map() {
    let ws = empty_ws();
    let infos = build_recipe_infos_from_registered(&ws);
    assert!(infos.is_empty());
}

#[test]
fn surface_recipe_populates_serves_and_requires() {
    let mut ws = empty_ws();
    ws.names.push(make_name("build", &["compile"]));
    ws.units_by_recipe.insert(
        "build".to_string(),
        RecipeUnits {
            recipe_name: "build".to_string(),
            deps: vec!["compile".to_string()],
            units: Vec::new(),
            step_groups: Vec::new(),
            working_dir: PathBuf::from("/tmp"),
            env_vars: BTreeMap::new(),
            terminal_outputs: vec!["build/app".to_string()],
            dep_edges: Vec::new(),
            probes: Vec::new(),
        },
    );

    let infos = build_recipe_infos_from_registered(&ws);
    let info = infos.get("build").expect("build present");
    assert_eq!(info.serves, vec!["build/app".to_string()]);
    assert_eq!(info.requires, vec!["compile".to_string()]);
    assert!(info.ingredients.is_empty());
}

#[test]
fn dynamic_recipe_with_no_units_has_empty_serves() {
    // Dynamic recipes (e.g. cook_cc.bin) register a name but may not
    // produce a RecipeUnits entry; build_recipe_infos_from_registered
    // must tolerate that and fall back to requires-only.
    let mut ws = empty_ws();
    ws.names.push(make_name("cc_bin", &["compile"]));
    let infos = build_recipe_infos_from_registered(&ws);
    let info = infos.get("cc_bin").expect("cc_bin present");
    assert!(info.serves.is_empty());
    assert_eq!(info.requires, vec!["compile".to_string()]);
}
