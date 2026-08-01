use super::*;

#[test]
fn anchor_globs_joins_relative_and_keeps_absolute() {
    let dir = std::path::Path::new("/ws/apps/rust");
    let got = anchor_globs(
        vec!["src/*.c".to_string(), "/abs/x/*.h".to_string()],
        dir,
    );
    assert_eq!(got, vec!["/ws/apps/rust/src/*.c".to_string(), "/abs/x/*.h".to_string()]);
}

/// COOK-407: the watch set came from the entry Cookfile's AST, so it saw only
/// recipes written as `recipe NAME` blocks in that one file. A
/// module-registered unit (`cook_cc.bin` mints its units through
/// `cook.add_unit` and has no AST recipe) and every imported member
/// contributed nothing, and `cook serve` reported itself as watching while
/// watching none of a C++ project's sources.
#[test]
fn watch_set_comes_from_registered_units_including_module_and_imported_ones() {
    use cook_contracts::cache::{CacheMeta, DeclaredInput};
    use cook_contracts::{CapturedUnit, DepKind, RecipeUnits, WorkPayload};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn unit(inputs: Vec<DeclaredInput>) -> CapturedUnit {
        CapturedUnit {
            payload: WorkPayload::Shell {
                cmd: "cc -c".into(),
                line: 1,
            },
            cache_meta: Some(CacheMeta {
                recipe_name: "r".into(),
                project_id: String::new(),
                cookfile_path: String::new(),
                cache_key: String::new(),
                inputs,
                ..Default::default()
            }),
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: BTreeMap::new(),
            member: None,
            output_paths: vec![],
            test_name: None,
        }
    }

    fn recipe(name: &str, wd: &str, units: Vec<CapturedUnit>) -> RecipeUnits {
        RecipeUnits {
            recipe_name: name.into(),
            deps: vec![],
            units,
            step_groups: vec![],
            working_dir: PathBuf::from(wd),
            env_vars: BTreeMap::new(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        }
    }

    let mut units_by_recipe = BTreeMap::new();
    // A module-registered unit in the root: no AST recipe would exist for it.
    units_by_recipe.insert(
        "app".to_string(),
        recipe(
            "app",
            "/ws",
            vec![unit(vec![
                DeclaredInput::path("src/main.c"),
                DeclaredInput::pattern("src/*.h"),
            ])],
        ),
    );
    // An imported member: relative to ITS directory, not the entry Cookfile's.
    units_by_recipe.insert(
        "lib.build".to_string(),
        recipe(
            "lib.build",
            "/ws/vendor/lib",
            vec![unit(vec![DeclaredInput::path("lib.c")])],
        ),
    );
    // Not in the requested order: must not be watched.
    units_by_recipe.insert(
        "unrelated".to_string(),
        recipe(
            "unrelated",
            "/ws",
            vec![unit(vec![DeclaredInput::path("nope.c")])],
        ),
    );

    let registered = cook_engine::RegisteredWorkspace {
        units_by_recipe,
        ..Default::default()
    };

    let globs = crate::watcher::CookWatcher::collect_globs_for_recipes(
        &registered,
        &["app".to_string(), "lib.build".to_string()],
    );

    assert!(globs.contains(&"/ws/src/main.c".to_string()), "{globs:?}");
    assert!(globs.contains(&"/ws/src/*.h".to_string()), "{globs:?}");
    assert!(
        globs.contains(&"/ws/vendor/lib/lib.c".to_string()),
        "an imported member's input anchors to its own working_dir: {globs:?}"
    );
    assert!(
        !globs.iter().any(|g| g.contains("nope.c")),
        "only recipes in the requested chain are watched: {globs:?}"
    );
}
