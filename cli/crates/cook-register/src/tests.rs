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
    let result = rt.register_recipe(lua_src, "hello", None).unwrap();
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
    let result = rt.register_recipe(lua_src, "multi", None).unwrap();
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
    let result = rt.register_recipe(lua_src, "build", None).unwrap();
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
    let result = rt.register_recipe(lua_src, "build", None).unwrap();
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
    let result = rt.register_recipe(lua_src, "build", None).unwrap();
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
    let lib_result = rt.register_recipe(lua_src, "lib", None).unwrap();
    assert_eq!(lib_result.units.len(), 0);

    // Register "app" — it imports
    let app_result = rt.register_recipe(lua_src, "app", None).unwrap();
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
    let result = rt.register_recipe(lua_src, "check", None).unwrap();
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
    let result = rt.register_recipe(lua_src, "tests", None).unwrap();
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

/// CS-0061 §3.2: `suite` defaults to the enclosing recipe's qualified name
/// when the caller omits the field. Exercises the engine path (current_recipe
/// is set by Registry::register_recipe, not by the unit-level helper).
#[test]
fn test_add_test_defaults_suite_to_recipe_name_via_engine() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("my_tests", {}, function()
    cook.add_test({
        command = "./run",
        name = "t",
    })
end)
"#;
    let result = rt.register_recipe(lua_src, "my_tests", None).unwrap();
    assert_eq!(result.units.len(), 1);
    match &result.units[0].payload {
        WorkPayload::Test { suite_name, .. } => {
            assert_eq!(suite_name, "my_tests",
                "suite should default to recipe name when omitted");
        }
        _ => panic!("expected Test payload"),
    }
}

/// CS-0061 §3.2: qualified prefix is included in the default suite name.
#[test]
fn test_add_test_defaults_suite_includes_qualified_prefix() {
    use crate::dep_output_api::SharedTerminalOutputs;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    let dir = TempDir::new().unwrap();
    let shared: SharedTerminalOutputs = Arc::new(Mutex::new(BTreeMap::new()));
    let lua_src = r#"
cook.recipe("tests", {}, function()
    cook.add_test({
        command = "./run",
        name = "t",
    })
end)
"#;
    let rt = Registry::new(dir.path().to_path_buf(), HashMap::new())
        .with_shared_terminal_outputs(shared)
        .with_qualified_prefix("mylib".to_string());
    let result = rt.register_recipe(lua_src, "tests", None).unwrap();
    match &result.units[0].payload {
        WorkPayload::Test { suite_name, .. } => {
            assert_eq!(suite_name, "mylib.tests",
                "suite default must include the qualified prefix");
        }
        _ => panic!("expected Test payload"),
    }
}

/// CS-0061 §3.2: empty command is rejected at register time.
#[test]
fn test_add_test_rejects_empty_command_via_engine() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("r", {}, function()
    cook.add_test({ command = "" })
end)
"#;
    let result = rt.register_recipe(lua_src, "r", None);
    assert!(result.is_err(), "empty command must be rejected");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("command"), "error should mention 'command', got: {err}");
}

/// CS-0061 §3.2: timeout = 0 is rejected at register time.
#[test]
fn test_add_test_rejects_zero_timeout_via_engine() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("r", {}, function()
    cook.add_test({ command = "true", timeout = 0 })
end)
"#;
    let result = rt.register_recipe(lua_src, "r", None);
    assert!(result.is_err(), "timeout = 0 must be rejected");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("timeout"), "error should mention 'timeout', got: {err}");
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

// -----------------------------------------------------------------------
// Config block dispatch tests
// -----------------------------------------------------------------------

#[test]
fn test_registry_runs_config_block_and_applies_env() {
    let mut initial_env = HashMap::new();
    initial_env.insert("CC".to_string(), "initial".to_string());

    let lua_source = r#"
function __cook_run_config_blocks(selected_name)
    env.CC = "from_base"
    if selected_name ~= nil then
        if selected_name == "release" then
            env.CXXFLAGS = "-O3"
        end
    end
end

cook.recipe("build", {}, function() end)
"#;

    let tmp = TempDir::new().unwrap();
    let registry = Registry::new(tmp.path().to_path_buf(), initial_env)
        .with_selected_config(Some("release".to_string()));

    let units = registry.register_recipe(lua_source, "build", None).unwrap();

    assert_eq!(units.env_vars.get("CC").map(|s| s.as_str()), Some("from_base"));
    assert_eq!(units.env_vars.get("CXXFLAGS").map(|s| s.as_str()), Some("-O3"));
}

#[test]
fn test_registry_config_block_unnamed_runs_when_no_selection() {
    let initial_env = HashMap::new();
    let lua_source = r#"
function __cook_run_config_blocks(selected_name)
    env.BASE = "applied"
    if selected_name ~= nil then
        if selected_name == "never_selected" then
            env.SHOULD_NOT_APPEAR = "1"
        end
    end
end

cook.recipe("build", {}, function() end)
"#;

    let tmp = TempDir::new().unwrap();
    let registry = Registry::new(tmp.path().to_path_buf(), initial_env);
    let units = registry.register_recipe(lua_source, "build", None).unwrap();

    assert_eq!(units.env_vars.get("BASE").map(|s| s.as_str()), Some("applied"));
    assert!(units.env_vars.get("SHOULD_NOT_APPEAR").is_none());
}

#[test]
fn test_registry_no_dispatcher_no_op() {
    let lua_source = r#"cook.recipe("build", {}, function() end)"#;
    let tmp = TempDir::new().unwrap();
    let registry = Registry::new(tmp.path().to_path_buf(), HashMap::new());
    let units = registry.register_recipe(lua_source, "build", None).unwrap();
    assert_eq!(units.recipe_name, "build");
}

#[test]
fn test_cli_overrides_win_over_config_block_defaults() {
    // Regression: a `config` block's `env.X = "default"` is last-write-wins
    // per Standard §3.6, but explicit CLI `--set X=Y` overrides MUST still
    // win. The engine reapplies cli_overrides on cook.env after the
    // dispatcher returns.
    let mut initial_env = HashMap::new();
    initial_env.insert("OPT_LEVEL".to_string(), "0".to_string());
    initial_env.insert("UNSET_KEY".to_string(), "from_env".to_string());

    let mut cli_overrides = HashMap::new();
    cli_overrides.insert("OPT_LEVEL".to_string(), "0".to_string());

    let lua_source = r#"
function __cook_run_config_blocks(selected_name)
    env.OPT_LEVEL = "3"
    env.GREETING = "hello"
end

cook.recipe("build", {}, function() end)
"#;

    let tmp = TempDir::new().unwrap();
    let registry = Registry::new(tmp.path().to_path_buf(), initial_env)
        .with_cli_overrides(cli_overrides);
    let units = registry.register_recipe(lua_source, "build", None).unwrap();

    // CLI override wins over the config block's `env.OPT_LEVEL = "3"`.
    assert_eq!(units.env_vars.get("OPT_LEVEL").map(|s| s.as_str()), Some("0"));
    // Keys not in cli_overrides keep the config-block value.
    assert_eq!(units.env_vars.get("GREETING").map(|s| s.as_str()), Some("hello"));
    // Process-env keys not touched by the config block (and not in
    // cli_overrides) flow through unchanged.
    assert_eq!(units.env_vars.get("UNSET_KEY").map(|s| s.as_str()), Some("from_env"));
}

#[test]
fn test_cli_override_for_undeclared_key_still_applied() {
    // CLI overrides apply unconditionally, even for keys the config block
    // doesn't declare. Whether `$<X>` resolution then accepts X is a
    // separate concern handled by env_keyset / require_env.
    let initial_env = HashMap::new();
    let mut cli_overrides = HashMap::new();
    cli_overrides.insert("ARBITRARY".to_string(), "42".to_string());

    let lua_source = r#"
function __cook_run_config_blocks(selected_name)
    env.DECLARED = "yes"
end

cook.recipe("build", {}, function() end)
"#;

    let tmp = TempDir::new().unwrap();
    let registry = Registry::new(tmp.path().to_path_buf(), initial_env)
        .with_cli_overrides(cli_overrides);
    let units = registry.register_recipe(lua_source, "build", None).unwrap();

    assert_eq!(units.env_vars.get("ARBITRARY").map(|s| s.as_str()), Some("42"));
    assert_eq!(units.env_vars.get("DECLARED").map(|s| s.as_str()), Some("yes"));
}

#[test]
fn test_recipe_shadows_env_var_does_not_panic() {
    // Standard §5.2.3: when a recipe and a declared env var share a name,
    // the recipe wins and a warning is emitted to stderr. We can't easily
    // capture stderr from the integration-shaped test here, so we just
    // confirm that registering succeeds (no panic, no error returned) when
    // a recipe shadows an env var. The end-to-end smoke test in the CLI
    // covers the actual diagnostic text. The dedup of the warning across
    // multiple recipe-register calls is also exercised end-to-end.
    let lua_source = r#"
function __cook_run_config_blocks(selected_name)
    env.foo = "shadowed"
    env.bar = "also_shadowed"
    env.kept = "no_recipe_with_this_name"
end

cook.recipe("foo", {}, function() end)
cook.recipe("bar", {}, function() end)
cook.recipe("standalone", {}, function() end)
"#;

    let tmp = TempDir::new().unwrap();
    let registry = Registry::new(tmp.path().to_path_buf(), HashMap::new());
    // Multiple register calls (would have emitted duplicate warnings before
    // the dedup fix); just assert each succeeds.
    registry.register_recipe(lua_source, "foo", None).expect("foo register");
    registry.register_recipe(lua_source, "bar", None).expect("bar register");
    registry.register_recipe(lua_source, "standalone", None).expect("standalone register");
}

// -----------------------------------------------------------------------
// Chore registration tests (CS-0020)
// -----------------------------------------------------------------------

#[test]
fn test_chore_registers_as_recipe_with_interactive_and_no_cache() {
    // A chore compiled by cook-luagen must register units with
    // interactive = true and cache = false.
    let tmp = TempDir::new().unwrap();
    let rt = make_registry(tmp.path());

    // Simulate what compile_chore emits.
    let lua_src = r#"
cook.recipe("clean", {}, function()
    cook._enter_chore()
    cook.add_unit({command = [[rm -rf build]], interactive = true, line = 2, cache = false})
    cook._exit_chore()
end)
"#;
    let result = rt.register_recipe(lua_src, "clean", None).unwrap();
    assert_eq!(result.units.len(), 1);
    // cache_meta must be None (cache = false).
    assert!(result.units[0].cache_meta.is_none(), "chore unit must have no cache_meta");
    // Payload must be Interactive.
    match &result.units[0].payload {
        WorkPayload::Interactive { cmd, .. } => {
            assert_eq!(cmd, "rm -rf build");
        }
        other => panic!("expected Interactive payload, got: {:?}", other),
    }
}

#[test]
fn test_chore_cache_true_rejected_while_chore_active() {
    // §{chores.no-caching}: cook.add_unit({cache = true}) MUST raise a Lua
    // error while cook._enter_chore() is active.
    let tmp = TempDir::new().unwrap();
    let rt = make_registry(tmp.path());

    let lua_src = r#"
cook.recipe("evil", {}, function()
    cook._enter_chore()
    cook.add_unit({command = "true", cache = true})
    cook._exit_chore()
end)
"#;
    let result = rt.register_recipe(lua_src, "evil", None);
    assert!(
        result.is_err(),
        "cache = true inside a chore must raise an error, but succeeded"
    );
    let err = result.err().unwrap().to_string();
    assert!(
        err.contains("cache") || err.contains("chore"),
        "error message should mention cache/chore, got: {err}"
    );
}

#[test]
fn test_chore_cache_true_allowed_outside_chore() {
    // After _exit_chore(), cache = true must be allowed again.
    let tmp = TempDir::new().unwrap();
    let rt = make_registry(tmp.path());

    let lua_src = r#"
cook.recipe("chore_then_recipe", {}, function()
    cook._enter_chore()
    cook.add_unit({command = "echo in chore", cache = false})
    cook._exit_chore()
    -- After exiting chore context, cache = true is permitted.
    cook.add_unit({command = "echo normal", cache = true})
end)
"#;
    let result = rt.register_recipe(lua_src, "chore_then_recipe", None);
    assert!(
        result.is_ok(),
        "cache = true after _exit_chore should be allowed, got: {:?}",
        result.err()
    );
    let units = result.unwrap();
    assert_eq!(units.units.len(), 2);
    assert!(units.units[0].cache_meta.is_none());   // chore unit: no cache
    assert!(units.units[1].cache_meta.is_some());   // normal unit: cached
}

#[test]
fn test_compile_chore_and_register_integration() {
    // Full parse → compile_chore → register_recipe pipeline.
    use cook_lang::parse;
    use cook_luagen::compile_chore;

    let tmp = TempDir::new().unwrap();
    let rt = make_registry(tmp.path());

    // Note: no trailing `end` — chore uses implicit termination (next top-level
    // keyword or EOF closes the body). `end` at column-0 is just Content("end")
    // and would be parsed as a third shell step.
    let source = "chore clean\n    rm -rf .scratch\n    mkdir -p .scratch\n";
    let cookfile = parse(source).expect("parse should succeed");
    assert_eq!(cookfile.chores.len(), 1);

    // Generate Lua for just the chore using compile_chore.
    let lua = format!(
        "-- Generated by Cook\n{}",
        compile_chore(&cookfile.chores[0], &[])
    );

    let result = rt.register_recipe(&lua, "clean", None);
    assert!(
        result.is_ok(),
        "chore registration should succeed, got: {:?}",
        result.err()
    );
    let units = result.unwrap();
    // Two shell steps → two interactive units.
    assert_eq!(units.units.len(), 2, "expected 2 units, got: {:#?}", units.units);
    for unit in &units.units {
        assert!(
            unit.cache_meta.is_none(),
            "chore unit must have no cache_meta, got: {:#?}",
            unit.cache_meta
        );
        assert!(
            matches!(&unit.payload, WorkPayload::Interactive { .. }),
            "chore unit must be Interactive, got: {:#?}",
            unit.payload
        );
    }
}

// -----------------------------------------------------------------------
// qualified_prefix tests (Phase 6)
// -----------------------------------------------------------------------

#[test]
fn test_register_recipe_inserts_with_qualified_prefix() {
    use crate::dep_output_api::SharedTerminalOutputs;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    let tmp = tempfile::TempDir::new().unwrap();

    let shared: SharedTerminalOutputs = Arc::new(Mutex::new(BTreeMap::new()));
    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.step_group(function()
        cook.add_unit({ command = "echo foo", output = "build/foo.o" })
    end)
end)
"#;
    let registry = Registry::new(tmp.path().to_path_buf(), HashMap::new())
        .with_shared_terminal_outputs(shared.clone())
        .with_qualified_prefix("lib".to_string());

    registry.register_recipe(lua_src, "build", None).unwrap();

    let map = shared.lock().unwrap();
    assert!(
        map.contains_key("lib.build"),
        "expected 'lib.build', got: {:?}",
        map.keys().collect::<Vec<_>>()
    );
    assert!(
        !map.contains_key("build"),
        "should NOT contain bare 'build', got: {:?}",
        map.keys().collect::<Vec<_>>()
    );
}

#[test]
fn test_register_recipe_empty_prefix_uses_bare_name() {
    use crate::dep_output_api::SharedTerminalOutputs;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    let tmp = tempfile::TempDir::new().unwrap();

    let shared: SharedTerminalOutputs = Arc::new(Mutex::new(BTreeMap::new()));
    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.step_group(function()
        cook.add_unit({ command = "echo foo", output = "build/foo.o" })
    end)
end)
"#;
    let registry = Registry::new(tmp.path().to_path_buf(), HashMap::new())
        .with_shared_terminal_outputs(shared.clone())
        .with_qualified_prefix(String::new());

    registry.register_recipe(lua_src, "build", None).unwrap();

    let map = shared.lock().unwrap();
    assert!(
        map.contains_key("build"),
        "expected bare 'build' for root registry, got: {:?}",
        map.keys().collect::<Vec<_>>()
    );
    assert!(
        !map.contains_key(".build"),
        "should NOT contain '.build' (dot-prefixed empty prefix), got: {:?}",
        map.keys().collect::<Vec<_>>()
    );
}
