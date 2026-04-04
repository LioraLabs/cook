use cook_contracts::{DepKind, WorkPayload};

use super::*;
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

fn make_registry(dir: &std::path::Path) -> Registry {
    Registry::new(dir.to_path_buf(), HashMap::new())
}

// -----------------------------------------------------------------------
// Registration-mode tests
// -----------------------------------------------------------------------

#[test]
fn test_register_captures_shell_step() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("hello", {}, function()
    cook.exec("echo hello", 1)
end)
"#;
    let result = rt.register_recipe(lua_src, "hello").unwrap();
    assert_eq!(result.recipe_name, "hello");
    assert_eq!(result.units.len(), 1);
    match &result.units[0].payload {
        WorkPayload::Shell { cmd, line } => {
            assert_eq!(cmd, "echo hello");
            assert_eq!(*line, 1);
        }
        other => panic!("expected Shell payload, got: {:?}", other),
    }
    assert!(matches!(result.units[0].dep_kind, DepKind::Sequential));
}

#[test]
fn test_register_captures_multiple_shell_steps() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("multi", {}, function()
    cook.exec("echo first", 1)
    cook.exec("echo second", 2)
end)
"#;
    let result = rt.register_recipe(lua_src, "multi").unwrap();
    assert_eq!(result.units.len(), 2);
    assert!(matches!(result.units[0].dep_kind, DepKind::Sequential));
    assert!(matches!(result.units[1].dep_kind, DepKind::Sequential));
    match &result.units[0].payload {
        WorkPayload::Shell { cmd, .. } => {
            assert_eq!(cmd, "echo first");
        }
        other => panic!("expected Shell, got: {:?}", other),
    }
    match &result.units[1].payload {
        WorkPayload::Shell { cmd, .. } => {
            assert_eq!(cmd, "echo second");
        }
        other => panic!("expected Shell, got: {:?}", other),
    }
}

#[test]
fn test_register_returns_recipe_deps() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("build", {requires = {"clean"}}, function()
    cook.exec("echo building", 1)
end)
cook.recipe("clean", {}, function()
    cook.exec("echo cleaning", 1)
end)
"#;
    let result = rt.register_recipe(lua_src, "build").unwrap();
    assert_eq!(result.deps, vec!["clean"]);
}

#[test]
fn test_register_step_groups_with_add_unit() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.step_group(function()
        cook.add_unit({ command = "gcc -c a.c -o a.o", inputs = {"a.c"}, output = "a.o" })
        cook.add_unit({ command = "gcc -c b.c -o b.o", inputs = {"b.c"}, output = "b.o" })
    end)
end)
"#;
    let result = rt.register_recipe(lua_src, "build").unwrap();
    assert_eq!(result.units.len(), 2);
    assert_eq!(result.step_groups.len(), 1);
    assert_eq!(result.step_groups[0].len(), 2);
    assert!(result.step_groups[0].contains(&0));
    assert!(result.step_groups[0].contains(&1));
    assert!(matches!(result.units[0].dep_kind, DepKind::StepGroup(0)));
    assert!(matches!(result.units[1].dep_kind, DepKind::StepGroup(0)));
}

#[test]
fn test_module_loads_and_adds_units() {
    let dir = TempDir::new().unwrap();

    // Create a module
    let modules_dir = dir.path().join("cook_modules");
    fs::create_dir_all(&modules_dir).unwrap();
    fs::write(modules_dir.join("test_mod.lua"), r#"
        local m = {}
        function m.add_steps()
            cook.step_group(function()
                cook.add_unit({
                    inputs = { "a.txt" },
                    output = "b.txt",
                    command = "cp a.txt b.txt",
                })
                cook.add_unit({
                    inputs = { "c.txt" },
                    output = "d.txt",
                    command = "cp c.txt d.txt",
                })
            end)
        end
        return m
    "#).unwrap();

    let rt = make_registry(dir.path());
    let lua_src = r#"
local test_mod = cook.load_module("test_mod")
cook.recipe("build", {}, function()
    test_mod.add_steps()
end)
"#;
    let result = rt.register_recipe(lua_src, "build").unwrap();
    assert_eq!(result.units.len(), 2);
    assert!(matches!(result.units[0].dep_kind, DepKind::StepGroup(0)));
    assert!(matches!(result.units[1].dep_kind, DepKind::StepGroup(0)));
    assert!(result.units[0].cache_meta.is_some());
}

#[test]
fn test_export_import_across_recipes() {
    let dir = TempDir::new().unwrap();

    let modules_dir = dir.path().join("cook_modules");
    fs::create_dir_all(&modules_dir).unwrap();
    fs::write(modules_dir.join("test_mod.lua"), r#"
        local m = {}
        function m.export_lib()
            cook.export("mylib", { lib_path = "build/libmylib.a" })
        end
        function m.use_lib()
            local info = cook.import("mylib")
            cook.add_unit({
                inputs = { info.lib_path },
                output = "bin/app",
                command = "gcc " .. info.lib_path .. " -o bin/app",
            })
        end
        return m
    "#).unwrap();

    let rt = make_registry(dir.path());

    let lua_src = r#"
local test_mod = cook.load_module("test_mod")
cook.recipe("lib", {}, function()
    test_mod.export_lib()
end)
cook.recipe("app", {requires = {"lib"}}, function()
    test_mod.use_lib()
end)
"#;

    // Register "lib" first — it exports
    let lib_result = rt.register_recipe(lua_src, "lib").unwrap();
    assert_eq!(lib_result.units.len(), 0);

    // Register "app" — it imports
    let app_result = rt.register_recipe(lua_src, "app").unwrap();
    assert_eq!(app_result.units.len(), 1);
    match &app_result.units[0].payload {
        WorkPayload::Shell { cmd, .. } => {
            assert!(cmd.contains("libmylib.a"));
        }
        other => panic!("expected Shell, got: {:?}", other),
    }
}

#[test]
fn test_platform_available_in_module() {
    let dir = TempDir::new().unwrap();
    let modules_dir = dir.path().join("cook_modules");
    fs::create_dir_all(&modules_dir).unwrap();
    fs::write(modules_dir.join("test_mod.lua"), r#"
        local m = {}
        m.detected_os = cook.platform.os
        return m
    "#).unwrap();

    let rt = make_registry(dir.path());
    let lua_src = r#"
local test_mod = cook.load_module("test_mod")
cook.recipe("check", {}, function()
    cook.add_unit({ command = test_mod.detected_os, cache = false })
end)
"#;
    let result = rt.register_recipe(lua_src, "check").unwrap();
    match &result.units[0].payload {
        WorkPayload::Shell { cmd, .. } => {
            assert_eq!(cmd, std::env::consts::OS);
        }
        other => panic!("expected Shell, got: {:?}", other),
    }
}

#[test]
fn test_add_test_captures_test_unit() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("tests", {}, function()
    cook.step_group(function()
        cook.add_test({
            command = "./run_test_a",
            suite = "unit",
            name = "test_a",
            timeout = 60,
        })
        cook.add_test({
            command = "./run_test_b",
            suite = "unit",
            name = "test_b",
            should_fail = true,
        })
    end)
end)
"#;
    let result = rt.register_recipe(lua_src, "tests").unwrap();
    assert_eq!(result.units.len(), 2);
    match &result.units[0].payload {
        WorkPayload::Test { cmd, timeout, should_fail, suite_name, test_name, .. } => {
            assert_eq!(cmd, "./run_test_a");
            assert_eq!(*timeout, 60);
            assert!(!should_fail);
            assert_eq!(suite_name, "unit");
            assert_eq!(test_name, "test_a");
        }
        _ => panic!("expected Test payload"),
    }
    match &result.units[1].payload {
        WorkPayload::Test { should_fail, .. } => {
            assert!(*should_fail);
        }
        _ => panic!("expected Test payload"),
    }
}

#[test]
fn test_resolve_ingredients_api() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.c"), "").unwrap();
    fs::write(dir.path().join("b.c"), "").unwrap();
    fs::write(dir.path().join("skip.c"), "").unwrap();

    let lua = mlua::Lua::new();
    // Must create the cook table first — register_resolve_ingredients adds to it
    let cook = lua.create_table().unwrap();
    lua.globals().set("cook", cook).unwrap();

    crate::context::register_resolve_ingredients(&lua, dir.path()).unwrap();

    let result: Vec<String> = lua
        .load(r#"return cook.resolve_ingredients({"*.c"}, {"skip.c"})"#)
        .eval::<mlua::Table>()
        .unwrap()
        .sequence_values::<String>()
        .filter_map(|v| v.ok())
        .collect();

    assert!(result.contains(&"a.c".to_string()));
    assert!(result.contains(&"b.c".to_string()));
    assert!(!result.contains(&"skip.c".to_string()));
    assert_eq!(result.len(), 2);
}
