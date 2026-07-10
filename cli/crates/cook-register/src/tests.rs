use cook_contracts::{DepKind, RecipeUnits, WorkPayload};

use super::*;
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

fn make_registry(dir: &std::path::Path) -> RegisterSessionBuilder {
    RegisterSessionBuilder::new(dir.to_path_buf(), HashMap::new())
}

/// Helper: drive `register_cookfile` and return the `RecipeUnits` for a
/// named recipe. Migrated from the deleted `register_recipe` convenience
/// (SHI-222 Phase 6 Tasks 6.1+6.2). Panics if the named recipe is missing.
fn register_one(rt: RegisterSessionBuilder, lua_src: &str, name: &str) -> RecipeUnits {
    let registered = register_cookfile(rt, lua_src, None)
        .unwrap_or_else(|e| panic!("register_cookfile failed: {e:?}"));
    registered
        .units_by_recipe
        .get(name)
        .unwrap_or_else(|| {
            panic!(
                "recipe {name:?} not registered; got: {:?}",
                registered.units_by_recipe.keys().collect::<Vec<_>>()
            )
        })
        .clone()
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
    let result = register_one(rt, lua_src, "hello");
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
    let result = register_one(rt, lua_src, "multi");
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
    let result = register_one(rt, lua_src, "build");
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
    let result = register_one(rt, lua_src, "build");
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
    let result = register_one(rt, lua_src, "build");
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

    // Single register_cookfile pass invokes both bodies in topo order
    // (lib before app, since app requires lib), so the cook.export from
    // lib's body is visible by the time app's body calls cook.import.
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    let lib_result = registered.units_by_recipe.get("lib").expect("lib missing");
    assert_eq!(lib_result.units.len(), 0);

    let app_result = registered.units_by_recipe.get("app").expect("app missing");
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
    let result = register_one(rt, lua_src, "check");
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
        })
        cook.add_test({
            command = "./run_test_b",
            suite = "unit",
        })
    end)
end)
"#;
    let result = register_one(rt, lua_src, "tests");
    assert_eq!(result.units.len(), 2);
    match &result.units[0].payload {
        WorkPayload::Test { cmd, timeout, should_fail, suite_name, test_name, .. } => {
            assert_eq!(cmd, "./run_test_a");
            // CS-0135: cook.add_test no longer accepts timeout/should_fail/
            // name; WorkPayload::Test still carries these fields for the
            // engine executor, populated with their prior absent-defaults.
            assert_eq!(*timeout, u64::MAX); // CS-0135: no per-test time bound
            assert!(!should_fail);
            assert_eq!(suite_name, "unit");
            assert_eq!(test_name, "");
        }
        _ => panic!("expected Test payload"),
    }
    match &result.units[1].payload {
        WorkPayload::Test { should_fail, .. } => {
            assert!(!should_fail);
        }
        _ => panic!("expected Test payload"),
    }
}

/// CS-0061 §3.2: `suite` defaults to the enclosing recipe's qualified name
/// when the caller omits the field. Exercises the engine path (current_recipe
/// is set by `register_cookfile`'s per-body drain loop, not by the unit-level helper).
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
    let result = register_one(rt, lua_src, "my_tests");
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
    let rt = RegisterSessionBuilder::new(dir.path().to_path_buf(), HashMap::new())
        .with_shared_terminal_outputs(shared)
        .with_qualified_prefix("mylib".to_string());
    let result = register_one(rt, lua_src, "tests");
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
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "empty command must be rejected");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("command"), "error should mention 'command', got: {err}");
}

// CS-0135 §22.4: `cook.add_test` no longer accepts a `timeout` field, so
// the prior `test_add_test_rejects_zero_timeout_via_engine` register-time
// rejection test no longer has a live contract to cover (the field is
// silently ignored, not validated).

// -----------------------------------------------------------------------
// CS-0127: cook.add_unit typed-field sweep — every wrong-typed field is a
// register-phase hard error naming the field, never a silent coercion to
// its default. Mirrors the CS-0122 `command` precedent (unit_api.rs).
// -----------------------------------------------------------------------

/// Small helper shared by the CS-0127 field-typing tests below: register a
/// single-recipe Cookfile whose body is exactly one `cook.add_unit(spec)`
/// call, and return the register-time error string. Panics if registration
/// unexpectedly succeeds.
fn add_unit_reject(spec_body: &str) -> String {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = format!(
        r#"
cook.recipe("r", {{}}, function()
    cook.add_unit({{ {spec_body} }})
end)
"#
    );
    let result = register_cookfile(rt, &lua_src, None);
    assert!(
        result.is_err(),
        "expected register_cookfile to reject spec `{{ {spec_body} }}`, but it succeeded"
    );
    result.err().unwrap().to_string()
}

#[test]
fn test_add_unit_rejects_non_string_lua_code() {
    let err = add_unit_reject(r#"lua_code = 42"#);
    assert!(err.contains("lua_code"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_boolean_interactive() {
    let err = add_unit_reject(r#"command = "true", interactive = "yes""#);
    assert!(err.contains("interactive"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_integer_line() {
    let err = add_unit_reject(r#"command = "true", line = "7""#);
    assert!(err.contains("line"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_boolean_cache() {
    let err = add_unit_reject(r#"command = "true", cache = "no""#);
    assert!(err.contains("cache"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_table_inputs() {
    let err = add_unit_reject(r#"command = "true", inputs = "src/a.c""#);
    assert!(err.contains("inputs"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_string_element_inputs() {
    let err = add_unit_reject(r#"command = "true", inputs = {1, 2}"#);
    assert!(err.contains("inputs"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_string_output() {
    let err = add_unit_reject(r#"command = "true", output = {}"#);
    assert!(err.contains("output"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_table_outputs() {
    let err = add_unit_reject(r#"command = "true", outputs = "x""#);
    assert!(err.contains("outputs"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_string_element_outputs() {
    let err = add_unit_reject(r#"command = "true", outputs = {true}"#);
    assert!(err.contains("outputs"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_table_ingredient_groups() {
    let err = add_unit_reject(r#"command = "true", ingredient_groups = "x""#);
    assert!(err.contains("ingredient_groups"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_string_element_ingredient_groups() {
    let err = add_unit_reject(r#"command = "true", ingredient_groups = {{1}}"#);
    assert!(err.contains("ingredient_groups"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_unknown_step_kind_value() {
    let err = add_unit_reject(r#"command = "true", step_kind = "banana""#);
    assert!(err.contains("step_kind"), "got: {err}");
    assert!(err.contains("banana"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_string_step_kind() {
    let err = add_unit_reject(r#"command = "true", step_kind = 3"#);
    assert!(err.contains("step_kind"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_string_member() {
    let err = add_unit_reject(r#"command = "true", member = {}"#);
    assert!(err.contains("member"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_unknown_sharing_value() {
    let err = add_unit_reject(r#"command = "true", sharing = "wide""#);
    assert!(err.contains("sharing"), "got: {err}");
    assert!(err.contains("wide"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_string_sharing() {
    let err = add_unit_reject(r#"command = "true", sharing = 1"#);
    assert!(err.contains("sharing"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_table_env() {
    let err = add_unit_reject(r#"command = "true", env = "FOO=1""#);
    assert!(err.contains("env"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_string_env_value() {
    // A numeric value must NOT be silently coerced to "1" (mlua's
    // String: FromLua would otherwise do so) — env is a string→string map.
    let err = add_unit_reject(r#"command = "true", env = { FOO = 1 }"#);
    assert!(err.contains("env"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_table_file_refs() {
    let err = add_unit_reject(r#"command = "true", file_refs = "src/*.c""#);
    assert!(err.contains("file_refs"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_non_table_consulted_env_keys() {
    let err = add_unit_reject(r#"command = "true", consulted_env_keys = 42"#);
    assert!(err.contains("consulted_env_keys"), "got: {err}");
}

#[test]
fn test_add_unit_rejects_unknown_string_consulted_env_keys() {
    let err = add_unit_reject(r#"command = "true", consulted_env_keys = "FOO""#);
    assert!(err.contains("consulted_env_keys"), "got: {err}");
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
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), initial_env)
        .with_selected_config(Some("release".to_string()));

    let units = register_one(registry, lua_source, "build");

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
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), initial_env);
    let units = register_one(registry, lua_source, "build");

    assert_eq!(units.env_vars.get("BASE").map(|s| s.as_str()), Some("applied"));
    assert!(units.env_vars.get("SHOULD_NOT_APPEAR").is_none());
}

#[test]
fn test_registry_no_dispatcher_no_op() {
    let lua_source = r#"cook.recipe("build", {}, function() end)"#;
    let tmp = TempDir::new().unwrap();
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), HashMap::new());
    let units = register_one(registry, lua_source, "build");
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
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), initial_env)
        .with_cli_overrides(cli_overrides);
    let units = register_one(registry, lua_source, "build");

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
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), initial_env)
        .with_cli_overrides(cli_overrides);
    let units = register_one(registry, lua_source, "build");

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
    // covers the actual diagnostic text.
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
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), HashMap::new());
    // A single register_cookfile pass invokes every body and emits the
    // shadowing warning at the config-block dispatch step. We only assert
    // it does not error.
    let registered = register_cookfile(registry, lua_source, None)
        .expect("register_cookfile must succeed despite shadowing");
    // All three recipes must have been registered.
    assert!(registered.units_by_recipe.contains_key("foo"));
    assert!(registered.units_by_recipe.contains_key("bar"));
    assert!(registered.units_by_recipe.contains_key("standalone"));
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
    let result = register_one(rt, lua_src, "clean");
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
    let result = register_cookfile(rt, lua_src, None);
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
    let result = register_cookfile(rt, lua_src, None);
    assert!(
        result.is_ok(),
        "cache = true after _exit_chore should be allowed, got: {:?}",
        result.err()
    );
    let registered = result.unwrap();
    let units = registered
        .units_by_recipe
        .get("chore_then_recipe")
        .expect("chore_then_recipe missing");
    assert_eq!(units.units.len(), 2);
    assert!(units.units[0].cache_meta.is_none());   // chore unit: no cache
    assert!(units.units[1].cache_meta.is_some());   // normal unit: cached
}

#[test]
fn test_compile_chore_and_register_integration() {
    // Full parse → compile_chore → register_cookfile pipeline.
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
        compile_chore(&cookfile.chores[0], &[], &std::collections::BTreeSet::new())
    );

    let result = register_cookfile(rt, &lua, None);
    assert!(
        result.is_ok(),
        "chore registration should succeed, got: {:?}",
        result.err()
    );
    let registered = result.unwrap();
    let units = registered
        .units_by_recipe
        .get("clean")
        .expect("clean missing");
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
fn test_register_cookfile_inserts_with_qualified_prefix() {
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
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), HashMap::new())
        .with_shared_terminal_outputs(shared.clone())
        .with_qualified_prefix("lib".to_string());

    register_cookfile(registry, lua_src, None).unwrap();

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
fn test_register_cookfile_empty_prefix_uses_bare_name() {
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
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), HashMap::new())
        .with_shared_terminal_outputs(shared.clone())
        .with_qualified_prefix(String::new());

    register_cookfile(registry, lua_src, None).unwrap();

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

// -----------------------------------------------------------------------
// member_outputs recording tests (COOK-96 Task 7)
// -----------------------------------------------------------------------

/// COOK-96: `register_cookfile` must populate the shared `member_outputs` map
/// for any recipe that calls `cook.add_unit` with a `member` field. Mirrors
/// the terminal_outputs recording test above; uses a fan-out-style unit
/// (`member = "s1"`) so the engine can serve `$<recipe[s1]>` cross-recipe joins.
#[test]
fn test_register_cookfile_populates_member_outputs() {
    use crate::dep_output_api::SharedMemberOutputs;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    let tmp = tempfile::TempDir::new().unwrap();

    let shared: SharedMemberOutputs = Arc::new(Mutex::new(BTreeMap::new()));
    let lua_src = r#"
cook.recipe("render", {}, function()
    cook.step_group(function()
        cook.add_unit({ command = "ffmpeg -i a.mp4 out/s1.mp4", output = "out/s1.mp4", member = "s1" })
        cook.add_unit({ command = "ffmpeg -i b.mp4 out/s2.mp4", output = "out/s2.mp4", member = "s2" })
    end)
end)
"#;
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), HashMap::new())
        .with_shared_member_outputs(shared.clone());

    register_cookfile(registry, lua_src, None).unwrap();

    let map = shared.lock().unwrap();
    assert!(
        map.contains_key("render"),
        "expected 'render' entry in member_outputs, got: {:?}",
        map.keys().collect::<Vec<_>>()
    );
    let render = map.get("render").unwrap();
    assert_eq!(
        render.get("s1").map(|v| v.as_slice()),
        Some(&["out/s1.mp4".to_string()][..]),
        "member 's1' must map to out/s1.mp4"
    );
    assert_eq!(
        render.get("s2").map(|v| v.as_slice()),
        Some(&["out/s2.mp4".to_string()][..]),
        "member 's2' must map to out/s2.mp4"
    );
}

/// COOK-96: qualified prefix is applied to the member_outputs key, mirroring
/// the terminal_outputs qualified-prefix test.
#[test]
fn test_register_cookfile_member_outputs_with_qualified_prefix() {
    use crate::dep_output_api::SharedMemberOutputs;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    let tmp = tempfile::TempDir::new().unwrap();

    let shared: SharedMemberOutputs = Arc::new(Mutex::new(BTreeMap::new()));
    let lua_src = r#"
cook.recipe("encode", {}, function()
    cook.step_group(function()
        cook.add_unit({ command = "enc a.wav out/a.opus", output = "out/a.opus", member = "a" })
    end)
end)
"#;
    let registry = RegisterSessionBuilder::new(tmp.path().to_path_buf(), HashMap::new())
        .with_shared_member_outputs(shared.clone())
        .with_qualified_prefix("audio".to_string());

    register_cookfile(registry, lua_src, None).unwrap();

    let map = shared.lock().unwrap();
    assert!(
        map.contains_key("audio.encode"),
        "expected 'audio.encode' (qualified), got: {:?}",
        map.keys().collect::<Vec<_>>()
    );
    assert!(
        !map.contains_key("encode"),
        "bare 'encode' must not appear when prefix is set, got: {:?}",
        map.keys().collect::<Vec<_>>()
    );
}

// -----------------------------------------------------------------------
// Probe-unit registration tests (CS-0074 §22.5)
// -----------------------------------------------------------------------

#[test]
fn registry_collects_probe_declarations_into_recipe_units() {
    // Under the unified `register_cookfile` design, `cook.probe` calls inside
    // a recipe body land as `WorkPayload::Probe` capture units on `RecipeUnits.units`
    // (so the DAG builder can schedule them as in-recipe work). The session-scoped
    // `RecipeUnits.probes` view is reserved for top-level probes (spec §7).
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.probe("cc:zlib", {
        inputs = { tools = {"pkg-config"} },
        produce = "return { found = true }",
    })
    cook.exec("echo hello", 1)
end)
"#;

    let result = register_one(rt, lua_src, "build");
    // Body-scoped probe → CapturedUnit on `units`.
    let probe_units: Vec<_> = result
        .units
        .iter()
        .filter_map(|u| match &u.payload {
            WorkPayload::Probe { key, produce, .. } => Some((key.as_str(), produce.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(probe_units.len(), 1, "expected 1 probe unit, got: {:?}", probe_units);
    assert_eq!(probe_units[0], ("cc:zlib", "return { found = true }"));
}

#[test]
fn registry_collects_multiple_probes() {
    // See `registry_collects_probe_declarations_into_recipe_units` — body-scoped
    // probes appear as CapturedUnit entries in `units` under register_cookfile.
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.probe("cc:zlib",   { inputs = {}, produce = "return 1" })
    cook.probe("cc:openssl", { inputs = {}, produce = "return 2" })
end)
"#;

    let result = register_one(rt, lua_src, "build");
    let keys: Vec<&str> = result
        .units
        .iter()
        .filter_map(|u| match &u.payload {
            WorkPayload::Probe { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&"cc:zlib"), "expected cc:zlib in probes");
    assert!(keys.contains(&"cc:openssl"), "expected cc:openssl in probes");
}

#[test]
fn registry_duplicate_probe_key_propagates_error() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.probe("cc:zlib", { inputs = {}, produce = "return 1" })
    cook.probe("cc:zlib", { inputs = {}, produce = "return 2" })
end)
"#;

    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "duplicate probe key must fail register_cookfile");
    let err = result.err().unwrap().to_string();
    assert!(
        err.contains("cc:zlib"),
        "error must name the duplicate key; got: {err}"
    );
}

#[test]
fn registry_probe_without_probes_has_empty_vec() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.exec("echo hello", 1)
end)
"#;

    let result = register_one(rt, lua_src, "build");
    assert!(
        result.probes.is_empty(),
        "recipe with no cook.probe calls must have empty probes vec"
    );
}

#[test]
fn add_unit_with_probes_captured_in_recipe_units() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    // cook.probe must be registered first so the §22.5.5 probes-resolution
    // check at end-of-pass can resolve "cc:zlib".
    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.probe("cc:zlib", { inputs = {}, produce = "return 1" })
    cook.add_unit({
        command = "echo building",
        probes = { "cc:zlib" },
        cache = false,
    })
end)
"#;

    let result = register_one(rt, lua_src, "build");
    // After CS-0074 Bug 1 fix: probe units also appear in units vec.
    // 1 probe unit + 1 consumer unit = 2 total.
    assert_eq!(result.units.len(), 2, "expected probe unit + consumer unit");
    let consumer = result.units.iter().find(|u| matches!(u.payload, WorkPayload::Shell { .. }))
        .expect("expected a consumer Shell unit");
    assert_eq!(consumer.probes, vec!["cc:zlib"]);
}

// -----------------------------------------------------------------------
// C4: Probes resolution against probe registry (§22.5.5)
// -----------------------------------------------------------------------

#[test]
fn unresolved_probes_key_errors() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.add_unit({
        command = "true",
        outputs = {"build/myapp.o"},
        probes = {"cc:nonexistent"},
        cache = false,
    })
end)
"#;

    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "unknown probes key must fail");
    let err = result.err().unwrap().to_string();
    assert!(
        err.contains("lists probe key 'cc:nonexistent' in `probes` but no such probe was declared"),
        "got: {err}"
    );
}

#[test]
fn resolved_probes_key_succeeds() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.probe("cc:zlib", { inputs = {}, produce = "return 1" })
    cook.add_unit({
        command = "true",
        outputs = {"build/myapp.o"},
        probes = {"cc:zlib"},
        cache = false,
    })
end)
"#;

    let result = register_one(rt, lua_src, "build");
    // After CS-0074 Bug 1 fix: first unit is the probe, second is the consumer.
    let u = result.units.iter().find(|u| matches!(u.payload, WorkPayload::Shell { .. })).unwrap();
    assert_eq!(u.probes, vec!["cc:zlib"]);
}

#[test]
fn add_unit_legacy_requires_field_rejected() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.probe("cc:zlib", { inputs = {}, produce = "return 1" })
    cook.add_unit({
        name = "u",
        inputs = {},
        outputs = {"build/o"},
        cache = false,
        requires = {"cc:zlib"},
        command = "true",
    })
end)
"#;

    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "legacy `requires` field must be rejected");
    let err = result.err().unwrap().to_string();
    assert!(
        err.contains("rename to `probes`"),
        "diagnostic must direct to `probes`; got: {err}"
    );
}

// -----------------------------------------------------------------------
// C5: Probe requires cycle detection (§22.5.8)
// -----------------------------------------------------------------------

#[test]
fn probe_cycle_a_b_a_errors() {
    // Probe cycle detection runs against the session probe set — declared
    // probes at top level (or inside body but visible after the body loop)
    // form a deterministic graph that `register_cookfile` must reject.
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    let lua_src = r#"
cook.probe("cc:a", {
    inputs = { requires = {"cc:b"} },
    produce = "return 1",
})
cook.probe("cc:b", {
    inputs = { requires = {"cc:a"} },
    produce = "return 2",
})
cook.recipe("build", {}, function() end)
"#;

    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "probe cycle must fail register_cookfile");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("probe cycle detected"), "got: {err}");
    assert!(err.contains("cc:a") && err.contains("cc:b"), "got: {err}");
}

#[test]
fn probe_no_cycle_succeeds() {
    // Top-level (session-scoped) probes — declared before the recipe body
    // runs, surface on `RegisteredCookfile.probes`.
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    let lua_src = r#"
cook.probe("cc:a", { inputs = {}, produce = "return 1" })
cook.probe("cc:b", { inputs = { requires = {"cc:a"} }, produce = "return 2" })
cook.recipe("build", {}, function() end)
"#;

    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(registered.probes.len(), 2);
}

// -----------------------------------------------------------------------
// CS-0074 regression tests: Bug 1 — cook.probe creates a CapturedUnit
// -----------------------------------------------------------------------

#[test]
fn cook_probe_creates_a_capturedunit_with_workpayload_probe() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());

    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.probe("test:k", {
        inputs = {},
        produce = "return 42",
    })
    cook.exec("true", 1)
end)
"#;

    let result = register_one(rt, lua_src, "build");
    let probe_unit = result.units.iter().find(|u| matches!(u.payload, WorkPayload::Probe { .. }));
    assert!(
        probe_unit.is_some(),
        "expected a CapturedUnit with WorkPayload::Probe; units: {:?}",
        result.units.iter().map(|u| format!("{:?}", u.payload)).collect::<Vec<_>>()
    );
    if let WorkPayload::Probe { key, produce, .. } = &probe_unit.unwrap().payload {
        assert_eq!(key, "test:k");
        assert_eq!(produce, "return 42");
    }
}

// -----------------------------------------------------------------------
// SHI-222 Phase 2 Task 2.2 — register_cookfile entry point
// -----------------------------------------------------------------------

#[test]
fn register_cookfile_invokes_each_body_once_in_topo_order() {
    use crate::{register_cookfile, RegisterSessionBuilder};

    // Three recipes chosen so alphabetical order (a, m, z) differs from
    // topological order (z, m, a). This distinguishes "register_cookfile
    // walks the DAG in topo order" from "it happens to iterate the
    // recipe map alphabetically".
    //
    // Recipe "z" uses cook.add_unit (which produces Some(CacheMeta)) so we
    // can verify the per-body drain loop patches cache_meta.recipe_name
    // from the qualified name. The installer closures in
    // register_unit_api/install_cook_api capture "" at API-install time
    // (register_cookfile has no single recipe at install); the per-recipe
    // drain loop in register_cookfile must overwrite that with the actual
    // recipe name. cook.exec keeps cache_meta == None, so the other two
    // recipes still validate the topo-order command-payload story.
    let lua_src = r#"
        cook.recipe("z", {requires = {}}, function()
            cook.add_unit({ command = "touch z.txt", inputs = {"src/z.c"}, output = "z.txt" })
        end)
        cook.recipe("m", {requires = {"z"}}, function()
            cook.exec("touch m.txt", 2)
        end)
        cook.recipe("a", {requires = {"m"}}, function()
            cook.exec("touch a.txt", 3)
        end)
    "#;

    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let registered = register_cookfile(builder, lua_src, None).unwrap();

    // Topo order: z -> m -> a (alphabetical would be a, m, z).
    assert_eq!(registered.names.len(), 3);
    assert_eq!(registered.names[0].name, "z");
    assert_eq!(registered.names[1].name, "m");
    assert_eq!(registered.names[2].name, "a");

    // Each body must have been invoked exactly once. Asserting on the
    // captured payload (rather than just unit count) catches a body that
    // no-ops on a second invocation but would still leave units.len() == 1.
    let z_units = registered
        .units_by_recipe
        .get("z")
        .expect("missing units_by_recipe entry for z");
    assert_eq!(z_units.units.len(), 1, "recipe z should have produced exactly one unit");
    match &z_units.units[0].payload {
        WorkPayload::Shell { cmd, .. } => {
            assert_eq!(cmd, "touch z.txt", "recipe z captured wrong command");
        }
        other => panic!("recipe z produced unexpected payload: {other:?}"),
    }
    // Per-body drain loop must have patched cache_meta.recipe_name from
    // the qualified name (here just "z" — no qualified_prefix in tests).
    // Without the patch this would be "" because install_cook_api was
    // called with recipe_name: "" in register_cookfile.
    let z_meta = z_units.units[0]
        .cache_meta
        .as_ref()
        .expect("cook.add_unit must produce Some(CacheMeta)");
    assert_eq!(
        z_meta.recipe_name, "z",
        "drain loop must patch cache_meta.recipe_name from qualified name"
    );

    for (name, expected_cmd) in [("m", "touch m.txt"), ("a", "touch a.txt")] {
        let recipe_units = registered
            .units_by_recipe
            .get(name)
            .unwrap_or_else(|| panic!("missing units_by_recipe entry for {name}"));
        assert_eq!(
            recipe_units.units.len(),
            1,
            "recipe {name} should have produced exactly one unit"
        );
        match &recipe_units.units[0].payload {
            WorkPayload::Shell { cmd, .. } => {
                assert_eq!(cmd, expected_cmd, "recipe {name} captured wrong command");
            }
            other => panic!("recipe {name} produced unexpected payload: {other:?}"),
        }
    }
}

#[test]
fn register_cookfile_rejects_duplicate_dynamic_registration() {
    use crate::{register_cookfile, RegisterError, RegisterSessionBuilder};

    let lua_src = r#"
        cook.recipe("build", {requires = {}}, function() end)
        cook.recipe("build", {requires = {}}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let err = register_cookfile(builder, lua_src, None).unwrap_err();

    match err {
        RegisterError::RecipeCollision { name, sites } => {
            assert_eq!(name, "build");
            assert_eq!(sites.len(), 2);
        }
        other => panic!("expected RecipeCollision, got {other:?}"),
    }
}

#[test]
fn register_cookfile_captures_all_sites_for_triple_collision() {
    use crate::{register_cookfile, RegisterError, RegisterSessionBuilder};

    // Three cook.recipe calls for the same name; all three sites must be
    // captured in registration order so users can see every offender.
    let lua_src = r#"
        cook.recipe("build", {requires = {}}, function() end)
        cook.recipe("build", {requires = {}}, function() end)
        cook.recipe("build", {requires = {}}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let err = register_cookfile(builder, lua_src, None).unwrap_err();

    match err {
        RegisterError::RecipeCollision { name, sites } => {
            assert_eq!(name, "build");
            assert_eq!(sites.len(), 3, "all three sites must be captured");
        }
        other => panic!("expected RecipeCollision, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// SHI-222 Phase 2 Task 2.4 — list_names entry point
// -----------------------------------------------------------------------

#[test]
fn list_names_returns_registrations_without_invoking_bodies() {
    use crate::{list_names, RegisterSessionBuilder};

    // Body of recipe "a" would error if invoked — list_names must NOT run
    // bodies, so the call must succeed and surface both "a" and "b" with
    // their declared `requires` metadata.
    let lua_src = r#"
        cook.recipe("a", {requires = {}}, function()
            error("body must not run during list_names")
        end)
        cook.recipe("b", {requires = {"a"}}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let names = list_names(builder, lua_src).unwrap();
    let by_name: std::collections::BTreeMap<_, _> = names.iter()
        .map(|r| (r.name.clone(), r))
        .collect();
    assert!(by_name.contains_key("a"));
    assert!(by_name.contains_key("b"));
    assert_eq!(by_name["b"].requires, vec!["a".to_string()]);

    // Lock down that the Task 2.2 chunk-naming wiring still flows through
    // list_names: `cook.recipe` Lua calls are tagged Dynamic with a non-zero
    // source line. A regression that breaks the call-stack walk would land
    // line == 0.
    assert!(matches!(
        by_name["a"].source,
        crate::RegistrationSource::Dynamic { line } if line > 0
    ));
    // RecipeKind is hard-coded to Recipe until Phase 3 codegen distinguishes
    // chores. Locking this down catches an accidental kind flip during the
    // upcoming codegen split.
    assert_eq!(by_name["b"].kind, crate::RecipeKind::Recipe);
}

#[test]
fn register_cookfile_accepts_top_level_probe() {
    use crate::{register_cookfile, RegisterSessionBuilder};

    // Probe at top level (outside any recipe body) — per spec §6 step 4 /
    // §7 this is the session-scoped form and must succeed: the probe
    // appears in RegisteredCookfile.probes but is NOT scheduled as a
    // CapturedUnit inside any recipe body. Before the fix, the body push
    // in install_cook_probe was unconditional, so the top-level call
    // aborted lua.load(...).exec() with "called outside a recipe body".
    let lua_src = r#"
        cook.probe("os.kernel", {
            inputs = { env = {"PATH"} },
            produce = "return { found = true }",
        })
        cook.recipe("hello", {requires = {}}, function()
            cook.exec("echo hi", 0)
        end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let registered = register_cookfile(builder, lua_src, None).unwrap();

    assert!(
        registered.probes.contains_key("os.kernel"),
        "top-level probe must land in RegisteredCookfile.probes; got keys: {:?}",
        registered.probes.keys().collect::<Vec<_>>()
    );
    assert_eq!(registered.names.len(), 1);
    assert_eq!(registered.names[0].name, "hello");

    // Body-scoped CapturedUnit emission is *only* for probes inside a
    // recipe body. The top-level probe must NOT show up in the recipe's
    // units vector — locking this down catches a regression that would
    // re-introduce the unconditional push.
    let hello_units = registered
        .units_by_recipe
        .get("hello")
        .expect("missing units_by_recipe entry for hello");
    let probe_units: Vec<_> = hello_units
        .units
        .iter()
        .filter(|u| matches!(u.payload, WorkPayload::Probe { .. }))
        .collect();
    assert!(
        probe_units.is_empty(),
        "top-level probe must not be pushed as a CapturedUnit inside the recipe body; \
         got {} probe-payload unit(s) in hello.units",
        probe_units.len()
    );
}

#[test]
fn register_cookfile_surfaces_wrapper_registered_recipe() {
    use crate::{register_cookfile, RegisterSessionBuilder, RegistrationSource};

    // A wrapper module that internally calls cook.recipe. Inline so the
    // test is self-contained.
    let lua_src = r#"
        local mod = {}
        function mod.bin(name)
            cook.recipe(name, {requires = {}}, function()
                cook.exec("touch " .. name, 0)
            end)
        end

        mod.bin("game")
    "#;

    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let registered = register_cookfile(builder, lua_src, None).unwrap();

    assert_eq!(registered.names.len(), 1);
    assert_eq!(registered.names[0].name, "game");
    match registered.names[0].source {
        RegistrationSource::Dynamic { line } => {
            // Line should point at the `mod.bin("game")` call site, not at the
            // `cook.recipe(...)` line inside mod.bin. Don't pin the exact number
            // (depends on how Lua maps stack frames here) — just assert it's
            // greater than zero and within the source range.
            assert!(line > 0 && line <= 8, "line was {line}");
        }
        other => panic!("expected Dynamic, got {other:?}"),
    }
    assert!(registered.units_by_recipe.contains_key("game"));
}

// -----------------------------------------------------------------------
// SHI-222 Phase 3 Task 3.1 (review polish I3) — direct capture-layer
// tests for the codegen-private cook.__register_surface[_chore] closures.
//
// These closures are otherwise only exercised through codegen-driven
// integration tests, so a hand-written Lua call here pins the contract
// in isolation: (name, kind, source line) end-to-end through
// register_cookfile. The explicit `__line = N` assertion also locks down
// the M1 silent-coercion class — any future change that breaks `__line`
// parsing would fail at `line == 0` instead of `line == N`.
// -----------------------------------------------------------------------

#[test]
fn register_cookfile_records_static_surface_recipe_with_line() {
    use crate::{register_cookfile, RecipeKind, RegisterSessionBuilder, RegistrationSource};

    // Hand-written codegen-style source: call the codegen-private
    // `cook.__register_surface` helper directly with a populated meta
    // table (the exact shape `cook-luagen` emits for `recipe NAME`).
    let lua_src = r#"
        cook.__register_surface("build",
            {ingredients = {}, excludes = {}, requires = {}, __line = 7},
            function()
                cook.exec("touch ok.txt", 0)
            end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder =
        RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let registered = register_cookfile(builder, lua_src, None).unwrap();

    assert_eq!(registered.names.len(), 1);
    assert_eq!(registered.names[0].name, "build");
    assert_eq!(registered.names[0].kind, RecipeKind::Recipe);
    match registered.names[0].source {
        RegistrationSource::Static { line } => assert_eq!(line, 7),
        other => panic!("expected Static {{ line = 7 }}, got {other:?}"),
    }
    assert!(registered.units_by_recipe.contains_key("build"));
}

#[test]
fn register_cookfile_records_static_surface_chore_with_kind_chore() {
    use crate::{register_cookfile, RecipeKind, RegisterSessionBuilder, RegistrationSource};

    // Chores have no ingredients/excludes (parser-enforced), so omit
    // those fields to verify the defaults in `parse_meta_lists` work.
    let lua_src = r#"
        cook.__register_surface_chore("clean",
            {requires = {}, __line = 12},
            function()
                cook.exec("rm -rf build", 0)
            end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder =
        RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let registered = register_cookfile(builder, lua_src, None).unwrap();

    assert_eq!(registered.names.len(), 1);
    assert_eq!(registered.names[0].name, "clean");
    assert_eq!(registered.names[0].kind, RecipeKind::Chore);
    match registered.names[0].source {
        RegistrationSource::Static { line } => assert_eq!(line, 12),
        other => panic!("expected Static {{ line = 12 }}, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// SHI-222 Phase 3 Task 3.2 — surface-vs-dynamic collision is now
// observable end-to-end.
//
// This is the headline SHI-222 collision case: a codegen-emitted surface
// declaration (`recipe NAME`) colliding with a `cook.recipe(NAME, ...)`
// call from a register block, top-level module call, or wrapper. Before
// Task 3.1 split the registration paths, both sites tagged `Dynamic` and
// the diagnostic could not name which side was the surface declaration.
//
// The hand-written Lua here matches the shape `cook-luagen` emits for
// `recipe "build"` (cf. `codegen_emits_register_surface_for_surface_recipes`),
// so the test pins the integration between the codegen surface emission
// (Task 3.1, locked down in `cook-luagen::tests`) and the collision
// diagnostic in `detect_collisions` (Phase 2 Task 2.3).
// -----------------------------------------------------------------------

#[test]
fn register_cookfile_rejects_surface_vs_dynamic_collision() {
    use crate::{register_cookfile, RegisterError, RegisterSessionBuilder};

    // Simulate codegen output directly: surface recipe + register block
    // both registering "build". The surface site lowers via
    // `cook.__register_surface` (codegen-private); the dynamic site is
    // a plain `cook.recipe(...)` call as a register block would emit.
    let lua_src = r#"
        cook.__register_surface("build",
            {ingredients = {}, excludes = {}, requires = {}, __line = 3},
            function() end)
        cook.recipe("build", {requires = {}}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let err = register_cookfile(builder, lua_src, None).unwrap_err();
    match err {
        RegisterError::RecipeCollision { name, sites } => {
            assert_eq!(name, "build");
            assert_eq!(sites.len(), 2);
            assert!(
                sites.iter().any(|s| matches!(s.kind, crate::RegistrationSiteKind::SurfaceRecipe)),
                "expected one site tagged SurfaceRecipe, got sites: {sites:?}"
            );
            assert!(
                sites.iter().any(|s| matches!(s.kind, crate::RegistrationSiteKind::Dynamic)),
                "expected one site tagged Dynamic, got sites: {sites:?}"
            );
        }
        other => panic!("expected RecipeCollision, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// COOK-64 §22.5.9 — ingredients <probe> register pre-pass
// -----------------------------------------------------------------------

/// Drive the full surface pipeline (parse → codegen → register) so the
/// codegen-emitted `__for_each` surface metadata and the fan-out body are
/// exercised exactly as a real Cookfile would produce them.
fn register_surface(
    dir: &std::path::Path,
    cookfile: &str,
) -> Result<RegisteredCookfile, RegisterError> {
    let parsed = cook_lang::parse(cookfile).expect("fixture must parse");
    // Mirror the real engine parse stage (cook-engine pipeline/parse.rs): build
    // the recipe-name set so cross-recipe refs like `$<render[]>` (COOK-96) and
    // `{NAME.ACCESSOR}` resolve as recipe refs rather than env vars.
    let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&parsed);
    let lua_src = cook_luagen::generate_with_names_checked(&parsed, &recipe_names)
        .expect("fixture must lower");
    register_cookfile(make_registry(dir), &lua_src, None)
}

#[test]
fn for_each_probe_prepass_fans_out_units() {
    let dir = TempDir::new().unwrap();
    // A self-contained probe (no file inputs) returns a 3-element array; the
    // pre-pass must resolve it so the `ingredients <probe>` body fans out one
    // unit per card. Before COOK-64 the body errored on `cook.cache.get` returning nil.
    let cookfile = r#"
register
    cook.probe("cards", {
        inputs = {},
        produce = [[ return { {id="1", name="ace"}, {id="2", name="king"}, {id="3", name="queen"} } ]],
    })

recipe deal
    ingredients cards
    cook "build/$<in.id>.txt" {
        mkdir -p build
        printf '%s\n' "$<in.name>" > $<out>
    }
"#;
    let registered = register_surface(dir.path(), cookfile).expect("register");
    let units = registered
        .units_by_recipe
        .get("deal")
        .expect("deal recipe registered");
    assert_eq!(
        units.units.len(),
        3,
        "expected one fanned-out unit per card, got {}",
        units.units.len()
    );

    // §17.1 observable #5: per-member fingerprint — each unit's command_hash
    // must differ (the member is folded in by unit_api), so editing one card
    // re-runs only its unit.
    let hashes: std::collections::BTreeSet<u64> = units
        .units
        .iter()
        .filter_map(|u| u.cache_meta.as_ref().map(|m| m.command_hash))
        .collect();
    assert_eq!(
        hashes.len(),
        3,
        "each member's unit must have a distinct command_hash"
    );
}

#[test]
fn for_each_probe_field_selector_indexes_named_array() {
    let dir = TempDir::new().unwrap();
    // `ingredients catalog:items` iterates the array at the probe value's `items`
    // field — the pre-pass stores the whole record; the body indexes `[items]`.
    let cookfile = r#"
register
    cook.probe("catalog", {
        inputs = {},
        produce = [[ return { items = { {id="a"}, {id="b"} } } ]],
    })

recipe build_catalog
    ingredients catalog:items
    cook "build/$<in.id>.json" {
        mkdir -p build
        printf '%s\n' '$<in>' > $<out>
    }
"#;
    let registered = register_surface(dir.path(), cookfile).expect("register");
    let units = registered.units_by_recipe.get("build_catalog").unwrap();
    assert_eq!(units.units.len(), 2, "two items in the field array");
}

#[test]
fn for_each_two_segment_probe_key_fans_out() {
    let dir = TempDir::new().unwrap();
    // COOK-190: `ns:name` is the canonical probe naming; a two-segment key
    // in ingredients position must resolve to the declared probe, not
    // truncate to probe `cards` + field selector `list`.
    let cookfile = r#"
register
    cook.probe("cards:list", {
        inputs = {},
        produce = [[ return { "alpha", "beta", "gamma" } ]],
    })

recipe stamps
    ingredients cards:list
    cook "out/$<in>.stamp" {
        mkdir -p out
        printf '%s' "$<in>" > $<out>
    }
"#;
    let registered = register_surface(dir.path(), cookfile).expect("register");
    let units = registered.units_by_recipe.get("stamps").expect("stamps registered");
    assert_eq!(units.units.len(), 3, "one unit per member of probe 'cards:list'");
}

#[test]
fn for_each_two_segment_key_with_field_selector_fans_out() {
    let dir = TempDir::new().unwrap();
    // §22.5.10 three-segment form: two-segment probe key `ns:cards` + one
    // trailing `:items` field selector. The pre-pass resolves via the final
    // colon (no probe `ns:cards:items` is declared) and stashes the selected
    // array under the verbatim ref.
    let cookfile = r#"
register
    cook.probe("ns:cards", {
        inputs = {},
        produce = [[ return { items = { {id="a"}, {id="b"} } } ]],
    })

recipe build_cards
    ingredients ns:cards:items
    cook "out/$<in.id>.json" {
        mkdir -p out
        printf '%s' '$<in>' > $<out>
    }
"#;
    let registered = register_surface(dir.path(), cookfile).expect("register");
    let units = registered.units_by_recipe.get("build_cards").unwrap();
    assert_eq!(units.units.len(), 2, "two items via the ns:cards:items selector");
}

#[test]
fn for_each_exact_probe_key_wins_over_field_selector_split() {
    let dir = TempDir::new().unwrap();
    // Both `cards` (an object with a `list` field) and `cards:list` (an
    // array) are declared. §22.5.10 resolution: the exact key match wins.
    let cookfile = r#"
register
    cook.probe("cards", {
        inputs = {},
        produce = [[ return { list = { "x" } } ]],
    })
    cook.probe("cards:list", {
        inputs = {},
        produce = [[ return { "a", "b" } ]],
    })

recipe stamps
    ingredients cards:list
    cook "out/$<in>.stamp" {
        mkdir -p out
        printf '%s' "$<in>" > $<out>
    }
"#;
    let registered = register_surface(dir.path(), cookfile).expect("register");
    let units = registered.units_by_recipe.get("stamps").unwrap();
    assert_eq!(units.units.len(), 2, "exact key 'cards:list' wins over cards[list]");
}

#[test]
fn for_each_undeclared_two_segment_ref_error_names_full_ref() {
    let dir = TempDir::new().unwrap();
    let cookfile = r#"
recipe stamps
    ingredients nope:list
    cook "out/$<in>.stamp" {
        printf '%s' "$<in>" > $<out>
    }
"#;
    let err = register_surface(dir.path(), cookfile).expect_err("must reject");
    assert!(
        matches!(err, RegisterError::ForEachProbeUndeclared { ref key, .. } if key == "nope:list"),
        "error must name the full source ref, got {err:?}"
    );
}

#[test]
fn for_each_undeclared_probe_rejected() {
    let dir = TempDir::new().unwrap();
    // `ingredients nope` names a probe that was never declared.
    let cookfile = r#"
recipe deal
    ingredients nope
    cook "build/$<in.id>.txt" {
        printf '%s\n' "$<in.id>" > $<out>
    }
"#;
    let err = register_surface(dir.path(), cookfile).expect_err("must reject");
    assert!(
        matches!(err, RegisterError::ForEachProbeUndeclared { ref key, .. } if key == "nope"),
        "expected ForEachProbeUndeclared, got {err:?}"
    );
}

#[test]
fn for_each_non_array_probe_rejected() {
    let dir = TempDir::new().unwrap();
    // The probe resolves to a record, not an array — §22.5.9 requires a
    // sequence to iterate.
    let cookfile = r#"
register
    cook.probe("cards", {
        inputs = {},
        produce = [[ return { name = "not-an-array" } ]],
    })

recipe deal
    ingredients cards
    cook "build/$<in.id>.txt" {
        printf '%s\n' "$<in.id>" > $<out>
    }
"#;
    let err = register_surface(dir.path(), cookfile).expect_err("must reject");
    assert!(
        matches!(err, RegisterError::ForEachNotArray { ref selector, .. } if selector == "cards"),
        "expected ForEachNotArray for 'cards', got {err:?}"
    );
}

#[test]
fn for_each_probe_depending_on_build_artifact_rejected() {
    let dir = TempDir::new().unwrap();
    // `gen.json` exists (so the pre-pass produce succeeds) but is also the
    // declared output of recipe `gen` — i.e. a build artifact. An
    // ingredients <probe> source must be statically evaluable, so the
    // dependency is rejected.
    fs::write(dir.path().join("gen.json"), r#"[{"id":"x"}]"#).unwrap();
    let cookfile = r#"
register
    cook.probe("data", {
        inputs  = { files = {"gen.json"} },
        produce = [[ return cook.json_decode(cook.sh("cat gen.json")) ]],
    })

recipe gen
    cook "gen.json" {
        echo '[{"id":"x"}]' > $<out>
    }

recipe consume
    ingredients data
    cook "build/$<in.id>.txt" {
        mkdir -p build
        printf '%s' "$<in.id>" > $<out>
    }
"#;
    let err = register_surface(dir.path(), cookfile).expect_err("must reject");
    assert!(
        matches!(err, RegisterError::ForEachProbeArtifactDep { ref key, ref path } if key == "data" && path == "gen.json"),
        "expected ForEachProbeArtifactDep for probe 'data' on 'gen.json', got {err:?}"
    );
}

#[test]
fn for_each_unreachable_broken_probe_does_not_block_target() {
    let dir = TempDir::new().unwrap();
    // §22.5.9 demand-driven: building target `a` must NOT evaluate the probe of
    // the unrelated `ingredients <probe>` recipe `b` — even though `b`'s probe
    // resolves to a non-array (which would otherwise be a register error). `b`
    // registers with no units; `a` builds normally.
    let cookfile = r#"
register
    cook.probe("bad", { inputs = {}, produce = [[ return { not_an = "array" } ]] })

recipe a
    cook "build/a.txt" {
        mkdir -p build
        echo a > $<out>
    }

recipe b
    ingredients bad
    cook "build/$<in.id>.txt" {
        printf '%s' "$<in.id>" > $<out>
    }
"#;
    let parsed = cook_lang::parse(cookfile).expect("parse");
    let lua_src = cook_luagen::generate(&parsed);
    let rt = RegisterSessionBuilder::new(dir.path().to_path_buf(), HashMap::new())
        .with_target_argv("a".to_string(), vec![]);
    let registered =
        register_cookfile(rt, &lua_src, None).expect("building 'a' must not touch b's probe");
    assert_eq!(
        registered.units_by_recipe.get("a").unwrap().units.len(),
        1,
        "target 'a' fans out its single unit"
    );
    assert!(
        registered.units_by_recipe.get("b").unwrap().units.is_empty(),
        "unreachable for_each recipe 'b' registers with no units"
    );
}

// -----------------------------------------------------------------------
// COOK-96 Task 8 — `$<recipe[]>` per-member recipe-output accessor:
// end-to-end join + per-member fingerprint isolation.
// -----------------------------------------------------------------------

/// Build the `mux` fixture: three fan-out recipes over the same `sceneprobe`
/// (two records, ids `s1`/`s2`). `render` and `tts` each produce one artifact
/// per member; `mux` joins both producers FOR THE CURRENT MEMBER via
/// `$<render[]>` / `$<tts[]>`. Driving the full surface pipeline (parse →
/// codegen → register) exercises the real `cook.dep_output_member` lowering
/// and the topo-ordered producer→consumer member-output handoff.
const MUX_FIXTURE: &str = r#"
register
    cook.probe("sceneprobe", {
        inputs = {},
        produce = [[ return { {id="s1"}, {id="s2"} } ]],
    })

recipe render
    ingredients sceneprobe
    cook "build/$<in.id>.silent.mp4" {
        mkdir -p build
        echo "$<in.id>" > $<out>
    }

recipe tts
    ingredients sceneprobe
    cook "build/$<in.id>.wav" {
        mkdir -p build
        echo "$<in.id>" > $<out>
    }

recipe mux
    ingredients sceneprobe
    cook "build/$<in.id>.mp4" {
        bin/mux --video $<render[]> --audio $<tts[]> --out $<out>
    }
"#;

/// Return the mux unit whose Shell command mentions the given member token
/// (e.g. `s1`). Fan-out shell bodies bake the member into the command, so the
/// member's own `$<out>` path (`build/<id>.mp4`) is the reliable selector.
/// Single source of the member-selection logic shared by all mux tests.
fn mux_unit_for_member<'a>(units: &'a RecipeUnits, id: &str) -> &'a CapturedUnit {
    let needle = format!("build/{id}.mp4");
    units
        .units
        .iter()
        .find(|u| match &u.payload {
            WorkPayload::Shell { cmd, .. } => cmd.contains(&needle),
            _ => false,
        })
        .unwrap_or_else(|| panic!("no mux shell unit produces build/{id}.mp4"))
}

/// Convenience over [`mux_unit_for_member`] returning the unit's Shell command.
fn mux_cmd_for_member<'a>(units: &'a RecipeUnits, id: &str) -> &'a str {
    match &mux_unit_for_member(units, id).payload {
        WorkPayload::Shell { cmd, .. } => cmd.as_str(),
        _ => unreachable!("mux_unit_for_member only matches Shell units"),
    }
}

/// (join) Each mux member unit's command joins THIS member's render + tts
/// outputs: s1 ⇒ s1.silent.mp4 + s1.wav; s2 ⇒ s2.silent.mp4 + s2.wav.
#[test]
fn mux_two_upstream_per_member_join() {
    let dir = TempDir::new().unwrap();
    let registered = register_surface(dir.path(), MUX_FIXTURE).expect("register mux fixture");
    let mux = registered
        .units_by_recipe
        .get("mux")
        .expect("mux registered");

    let shell_units: Vec<_> = mux
        .units
        .iter()
        .filter(|u| matches!(u.payload, WorkPayload::Shell { .. }))
        .collect();
    assert_eq!(
        shell_units.len(),
        2,
        "mux fans out one unit per member (s1, s2); got {}",
        shell_units.len()
    );

    let s1 = mux_cmd_for_member(mux, "s1");
    assert!(
        s1.contains("build/s1.silent.mp4") && s1.contains("build/s1.wav"),
        "s1 mux unit must join s1's render+tts outputs; got: {s1}"
    );
    assert!(
        !s1.contains("s2"),
        "s1 mux unit must not reference any s2 path; got: {s1}"
    );

    let s2 = mux_cmd_for_member(mux, "s2");
    assert!(
        s2.contains("build/s2.silent.mp4") && s2.contains("build/s2.wav"),
        "s2 mux unit must join s2's render+tts outputs; got: {s2}"
    );
    assert!(
        !s2.contains("s1"),
        "s2 mux unit must not reference any s1 path; got: {s2}"
    );
}

/// (edge) Every mux unit carries recipe-level dep edges to BOTH producers.
#[test]
fn mux_per_member_records_dep_edges_to_both_producers() {
    let dir = TempDir::new().unwrap();
    let registered = register_surface(dir.path(), MUX_FIXTURE).expect("register mux fixture");
    let mux = registered
        .units_by_recipe
        .get("mux")
        .expect("mux registered");

    // dep_edges entries are (unit_idx, dep_recipe_name). Each mux shell unit
    // must have edges to both `render` and `tts`.
    for (idx, unit) in mux.units.iter().enumerate() {
        if !matches!(unit.payload, WorkPayload::Shell { .. }) {
            continue;
        }
        let deps_for_unit: std::collections::BTreeSet<&str> = mux
            .dep_edges
            .iter()
            .filter(|(u, _)| *u == idx)
            .map(|(_, name)| name.as_str())
            .collect();
        assert!(
            deps_for_unit.contains("render"),
            "mux unit {idx} must carry a dep edge to render; this unit's edges: {:?}",
            deps_for_unit
        );
        assert!(
            deps_for_unit.contains("tts"),
            "mux unit {idx} must carry a dep edge to tts; this unit's edges: {:?}",
            deps_for_unit
        );
    }
}

/// (isolation) The crux: each mux member unit's `cache_meta.input_paths`
/// contains ONLY its own member's upstream paths. The s1 unit must NOT carry
/// s2's paths and vice-versa. Before the per-member attribution fix the
/// step-group-wide accumulator leaked s1's paths into s2's fingerprint
/// (over-invalidation).
#[test]
fn mux_per_member_fingerprint_isolation() {
    let dir = TempDir::new().unwrap();
    let registered = register_surface(dir.path(), MUX_FIXTURE).expect("register mux fixture");
    let mux = registered
        .units_by_recipe
        .get("mux")
        .expect("mux registered");

    // Locate the two shell units by the member's own output path (shared helper).
    let s1 = mux_unit_for_member(mux, "s1");
    let s2 = mux_unit_for_member(mux, "s2");

    let s1_meta = s1.cache_meta.as_ref().expect("s1 mux unit has cache_meta");
    let s2_meta = s2.cache_meta.as_ref().expect("s2 mux unit has cache_meta");

    assert!(
        s1_meta.input_paths.contains(&"build/s1.silent.mp4".to_string())
            && s1_meta.input_paths.contains(&"build/s1.wav".to_string()),
        "s1 mux unit input_paths must contain s1's render+tts outputs; got: {:?}",
        s1_meta.input_paths
    );
    assert!(
        !s1_meta.input_paths.iter().any(|p| p.contains("s2")),
        "ISOLATION VIOLATION: s1 mux unit input_paths leaked an s2 path; got: {:?}",
        s1_meta.input_paths
    );

    assert!(
        s2_meta.input_paths.contains(&"build/s2.silent.mp4".to_string())
            && s2_meta.input_paths.contains(&"build/s2.wav".to_string()),
        "s2 mux unit input_paths must contain s2's render+tts outputs; got: {:?}",
        s2_meta.input_paths
    );
    assert!(
        !s2_meta.input_paths.iter().any(|p| p.contains("s1")),
        "ISOLATION VIOLATION: s2 mux unit input_paths leaked an s1 path; got: {:?}",
        s2_meta.input_paths
    );
}

// -----------------------------------------------------------------------
// CS-0101: cook.file_ref — $<file:PATH> register-phase resolution
// -----------------------------------------------------------------------

#[test]
fn file_ref_api_resolves_literal_and_glob() {
    use std::fs;
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("tokens.css"), "x").unwrap();
    fs::create_dir_all(dir.path().join("templates")).unwrap();
    fs::write(dir.path().join("templates/a.html"), "a").unwrap();
    fs::write(dir.path().join("templates/b.html"), "b").unwrap();
    let lua = mlua::Lua::new();
    let cook = lua.create_table().unwrap();
    lua.globals().set("cook", cook).unwrap();
    crate::file_ref::register_file_ref(&lua, dir.path()).unwrap();
    let lit: String = lua
        .load(r#"return cook.file_ref("tokens.css")"#)
        .eval()
        .unwrap();
    assert_eq!(lit, "tokens.css");
    let glob: String = lua
        .load(r#"return cook.file_ref("templates/*.html")"#)
        .eval()
        .unwrap();
    assert_eq!(glob, "templates/a.html templates/b.html"); // sorted
}

#[test]
fn file_ref_api_missing_literal_is_error() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let lua = mlua::Lua::new();
    let cook = lua.create_table().unwrap();
    lua.globals().set("cook", cook).unwrap();
    crate::file_ref::register_file_ref(&lua, dir.path()).unwrap();
    let err = lua
        .load(r#"return cook.file_ref("missing.css")"#)
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("missing.css"),
        "error must name the missing file; got: {err}"
    );
}

#[test]
fn file_ref_api_empty_glob_is_error() {
    use std::fs;
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("templates")).unwrap();
    let lua = mlua::Lua::new();
    let cook = lua.create_table().unwrap();
    lua.globals().set("cook", cook).unwrap();
    crate::file_ref::register_file_ref(&lua, dir.path()).unwrap();
    let err = lua
        .load(r#"return cook.file_ref("templates/*.html")"#)
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("matched no files"),
        "error must say the glob matched nothing; got: {err}"
    );
}

/// Test scaffold for CS-0101 add_unit `file_refs` plumbing: a Lua VM with
/// the full unit API registered against a caller-supplied working dir
/// (the file_refs field resolves patterns against real files).
fn make_unit_api_lua(working_dir: &std::path::Path) -> (mlua::Lua, SharedBodySlot) {
    use std::sync::{Arc, Mutex};
    let lua = mlua::Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    let body_slot: SharedBodySlot =
        std::rc::Rc::new(std::cell::RefCell::new(Some(BodyCaptureState::new())));
    let terminal_outputs: crate::dep_output_api::SharedTerminalOutputs =
        Arc::new(Mutex::new(std::collections::BTreeMap::new()));
    crate::unit_api::register_unit_api(
        &lua,
        body_slot.clone(),
        "recipe",
        terminal_outputs,
        working_dir.to_path_buf(),
    )
    .unwrap();
    (lua, body_slot)
}

#[test]
fn add_unit_file_refs_fold_into_cache_input_paths() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("in.md"), "m").unwrap();
    fs::write(dir.path().join("tokens.css"), "x").unwrap();

    let (lua, body_slot) = make_unit_api_lua(dir.path());
    lua.load(
        r#"
        cook.add_unit({
            inputs = {"in.md"},
            output = "out.html",
            command = "render",
            file_refs = {"tokens.css"},
        })
    "#,
    )
    .exec()
    .unwrap();

    let slot = body_slot.borrow();
    let body = slot.as_ref().expect("body slot populated");
    assert_eq!(body.units.len(), 1);
    let meta = body.units[0]
        .cache_meta
        .as_ref()
        .expect("cached unit has cache_meta");
    assert_eq!(
        meta.input_paths,
        vec!["in.md".to_string(), "tokens.css".to_string()],
        "file_refs matches must fold into cache_meta.input_paths after inputs"
    );
}

#[test]
fn add_unit_file_refs_missing_file_is_register_error() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("in.md"), "m").unwrap();

    let (lua, _body_slot) = make_unit_api_lua(dir.path());
    let err = lua
        .load(
            r#"
            cook.add_unit({
                inputs = {"in.md"},
                output = "out.html",
                command = "render",
                file_refs = {"missing.css"},
            })
        "#,
        )
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("missing.css"),
        "error must name the missing file; got: {err}"
    );
    assert!(err.contains("file not found"), "error must name the file-ref failure; got: {err}");
}
