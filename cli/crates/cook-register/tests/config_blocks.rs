//! End-to-end test: parse → codegen → register-phase execution with a
//! `config NAME` block that mutates env and asserts the mutation
//! reaches RecipeUnits.env_vars.

use cook_contracts::RecipeUnits;
use cook_lang::parse;
use cook_luagen::generate;
use cook_register::engine::{register_cookfile, RegisterSessionBuilder};
use std::collections::HashMap;

fn compile_and_run(source: &str, selected: Option<&str>) -> RecipeUnits {
    let cookfile = parse(source).expect("parse");
    let lua_source = generate(&cookfile);
    let tmp = tempfile::TempDir::new().unwrap();
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), HashMap::new())
        .with_selected_config(selected.map(|s| s.to_string()));
    // First recipe in the file is what we'll inspect.
    let name = cookfile.recipes[0].name.clone();
    let registered = register_cookfile(registry, &lua_source, None).expect("register");
    registered
        .units_by_recipe
        .get(&name)
        .unwrap_or_else(|| panic!("recipe {name:?} missing from units_by_recipe"))
        .clone()
}

#[test]
fn unnamed_config_applies_to_default_build() {
    let source = "\
config
    env.GREETING = \"hello\"

recipe build
";
    let units = compile_and_run(source, None);
    assert_eq!(units.env_vars.get("GREETING").map(|s| s.as_str()), Some("hello"));
}

#[test]
fn named_config_overlays_base() {
    let source = "\
config
    env.MODE = \"base\"
    env.OPT = \"-O0\"

config release
    env.MODE = \"release\"
    env.OPT = \"-O3\"

recipe build
";
    let units_rel = compile_and_run(source, Some("release"));
    assert_eq!(units_rel.env_vars.get("MODE").map(|s| s.as_str()), Some("release"));
    assert_eq!(units_rel.env_vars.get("OPT").map(|s| s.as_str()), Some("-O3"));

    let units_base = compile_and_run(source, None);
    assert_eq!(units_base.env_vars.get("MODE").map(|s| s.as_str()), Some("base"));
    assert_eq!(units_base.env_vars.get("OPT").map(|s| s.as_str()), Some("-O0"));
}

#[test]
fn no_config_blocks_still_builds() {
    let source = "recipe build\n";
    let units = compile_and_run(source, None);
    assert!(units.env_vars.get("GREETING").is_none());
}
