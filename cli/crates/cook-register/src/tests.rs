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

// CS-0153 §22.1: `step_kind = "test"` is no longer an accepted value on
// `cook.add_unit` — a test work unit is registrable only through
// `cook.add_test` (§22.4). Silently accepting `step_kind = "test"` here
// would build a unit invisible to `cook test` (WorkPayload::Test carries no
// StepKind; only `cook.add_test` constructs that payload), letting a test
// gate go green over a failing check.
#[test]
fn test_add_unit_rejects_step_kind_test() {
    let err = add_unit_reject(r#"command = "true", step_kind = "test""#);
    assert!(err.contains("step_kind"), "got: {err}");
    assert!(err.contains("cook.add_test"), "got: {err}");
}

// CS-0153 §22.1: the surviving accepted values — `"cook"` and `"chore"` —
// still register successfully, and the parsed kind lands on the LuaChunk
// payload. Guards the accept arms so the `"test"` rejection above can't
// silently widen.
#[test]
fn test_add_unit_accepts_step_kind_cook_and_chore() {
    for (kind, expected) in [
        ("cook", cook_contracts::StepKind::Cook),
        ("chore", cook_contracts::StepKind::Chore),
    ] {
        let dir = TempDir::new().unwrap();
        let rt = make_registry(dir.path());
        let lua_src = format!(
            r#"
cook.recipe("r", {{}}, function()
    cook.add_unit({{ lua_code = "return 1", cache = false, step_kind = "{kind}" }})
end)
"#
        );
        let result = register_one(rt, &lua_src, "r");
        assert_eq!(result.units.len(), 1);
        match &result.units[0].payload {
            WorkPayload::LuaChunk { step_kind, .. } => assert_eq!(*step_kind, expected),
            other => panic!("expected LuaChunk payload for {kind:?}, got: {other:?}"),
        }
    }
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

    crate::context::register_resolve_ingredients(&lua, dir.path(), dir.path()).unwrap();

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

// -----------------------------------------------------------------------
// CS-0143 — `cook.recipe` records an `origin` annotation.
// -----------------------------------------------------------------------

#[test]
fn list_names_surfaces_origin_when_present() {
    use crate::{list_names, RegisterSessionBuilder};

    let lua_src = r#"
        cook.recipe("workspace", {requires = {}, origin = "cook_pnpm.workspace"}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let names = list_names(builder, lua_src).unwrap();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].origin, Some("cook_pnpm.workspace".to_string()));
}

#[test]
fn list_names_surfaces_none_origin_when_absent() {
    use crate::{list_names, RegisterSessionBuilder};

    let lua_src = r#"
        cook.recipe("build", {requires = {}}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let names = list_names(builder, lua_src).unwrap();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].origin, None);
}

#[test]
fn list_names_surface_recipe_never_carries_origin() {
    use crate::{list_names, RegisterSessionBuilder};

    // A surface `recipe NAME` block lowers via `cook.__register_surface`,
    // never `cook.recipe` — it structurally cannot carry an origin. This
    // pins that guarantee: `parse_origin_meta` is only ever called from the
    // `cook.recipe` closure, never from `parse_meta_lists` (shared with the
    // surface paths).
    let lua_src = r#"
        cook.__register_surface("build",
            {ingredients = {}, excludes = {}, requires = {}, __line = 7},
            function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let names = list_names(builder, lua_src).unwrap();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].origin, None);
}

#[test]
fn list_names_surface_chore_never_carries_origin() {
    use crate::{list_names, RegisterSessionBuilder};

    // Same guarantee as the surface-recipe case, for the third registration
    // path: `cook.__register_surface_chore` also shares `parse_meta_lists`
    // and must never pick up an origin. Passing one explicitly proves the
    // field is ignored on this path rather than merely absent from the
    // fixture.
    let lua_src = r#"
        cook.__register_surface_chore("release",
            {ingredients = {}, excludes = {}, requires = {}, params = {},
             origin = "cook_pnpm.workspace", __line = 3},
            function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let names = list_names(builder, lua_src).unwrap();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].kind, crate::RecipeKind::Chore);
    assert_eq!(names[0].origin, None);
}

#[test]
fn list_names_rejects_non_string_origin() {
    use crate::{list_names, RegisterSessionBuilder};

    let lua_src = r#"
        cook.recipe("build", {requires = {}, origin = 42}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let err = list_names(builder, lua_src).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("origin"), "error must name the field: {msg}");
    // The offending Lua type must be named, not merely the field. This also
    // pins that `42` is rejected outright rather than silently coerced to
    // "42" by mlua's `String: FromLua` impl — the CS-0127 hazard that
    // `parse_origin_meta` matches on `LuaValue` specifically to avoid.
    assert!(
        msg.contains("integer"),
        "error must name the offending type: {msg}"
    );
    assert!(
        msg.contains("must be a string"),
        "error must state the expected type: {msg}"
    );
}

#[test]
fn list_names_rejects_empty_string_origin() {
    use crate::{list_names, RegisterSessionBuilder};

    // An empty string is a wrong-typed-adjacent authoring mistake: reject
    // it with the same error class as a non-string origin (an origin that
    // would render as `(from )` is worse than none at all).
    let lua_src = r#"
        cook.recipe("build", {requires = {}, origin = ""}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let err = list_names(builder, lua_src).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("origin"), "error must name the field: {msg}");
    assert!(
        msg.contains("non-empty string"),
        "error must state the expected type: {msg}"
    );
}

#[test]
fn register_cookfile_propagates_origin_through_full_pass() {
    use crate::{register_cookfile, RegisterSessionBuilder};

    // origin must survive not only the no-body-invocation `list_names`
    // path but also the full register pass (body invocation, unit capture,
    // DAG discovery).
    let lua_src = r#"
        cook.recipe("workspace", {requires = {}, origin = "cook_pnpm.workspace"}, function()
            cook.exec("echo hi", 0)
        end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let registered = register_cookfile(builder, lua_src, None).unwrap();
    assert_eq!(registered.names.len(), 1);
    assert_eq!(
        registered.names[0].origin,
        Some("cook_pnpm.workspace".to_string())
    );
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
    // the recipe-name set so cross-recipe refs like `$<render[in]>` (COOK-96,
    // respelled COOK-221/CS-0137) and `{NAME.ACCESSOR}` resolve as recipe refs
    // rather than env vars.
    let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&parsed);
    let lua_src = cook_luagen::generate_with_names_checked(&parsed, &recipe_names)
        .expect("fixture must lower");
    register_cookfile(make_registry(dir), &lua_src, None)
}

#[test]
fn registered_static_recipe_warns_once_per_declared_empty_ingredient() {
    let dir = TempDir::new().unwrap();
    let registered = register_surface(
        dir.path(),
        "recipe build\n    ingredients \"first.none\" \"second.none\"\n    cook \"out.txt\" { touch out.txt }\n",
    )
    .unwrap();
    assert_eq!(registered.warnings, vec![
        "ingredient \"first.none\" matched 0 files (recipe build)",
        "ingredient \"second.none\" matched 0 files (recipe build)",
    ]);
}

#[test]
fn registered_dynamic_recipe_helper_repeats_do_not_duplicate_warning() {
    let dir = TempDir::new().unwrap();
    let lua = r#"
cook.recipe("manual", {ingredients = {"missing.*"}, excludes = {}}, function()
    cook.resolve_ingredients({"missing.*"}, {})
    cook.resolve_ingredients({"missing.*"}, {})
end)
"#;
    let registered = register_cookfile(make_registry(dir.path()), lua, None).unwrap();
    assert_eq!(registered.warnings, vec![
        "ingredient \"missing.*\" matched 0 files (recipe manual)",
    ]);
}

#[test]
fn for_each_probe_prepass_fans_out_units() {
    let dir = TempDir::new().unwrap();
    // A self-contained probe (no file inputs) returns a 3-element array; the
    // pre-pass must resolve it so the `ingredients <probe>` body fans out one
    // unit per card. Before COOK-64 the body errored on `cook.probes.get` returning nil.
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
// COOK-96 Task 8 — `$<recipe[in]>` per-member recipe-output accessor
// (respelled by COOK-221/CS-0137): end-to-end join + per-member
// fingerprint isolation.
// -----------------------------------------------------------------------

/// Build the `mux` fixture: three fan-out recipes over the same `sceneprobe`
/// (two records, ids `s1`/`s2`). `render` and `tts` each produce one artifact
/// per member; `mux` joins both producers FOR THE CURRENT MEMBER via
/// `$<render[in]>` / `$<tts[in]>`. Driving the full surface pipeline (parse →
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
        bin/mux --video $<render[in]> --audio $<tts[in]> --out $<out>
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

// -----------------------------------------------------------------------
// cook.recipe_name() tests (Standard §22.7, CS-0141)
// -----------------------------------------------------------------------

/// A recipe body observes its own name via `cook.recipe_name()`. Captured
/// observably through `cook.exec` so the assertion reads real register
/// output (the shell command text) rather than a mock.
#[test]
fn recipe_name_returns_own_name() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.exec(cook.recipe_name(), 1)
end)
"#;
    let result = register_one(rt, lua_src, "build");
    assert_eq!(result.units.len(), 1);
    match &result.units[0].payload {
        WorkPayload::Shell { cmd, .. } => assert_eq!(cmd, "build"),
        other => panic!("expected Shell payload, got: {:?}", other),
    }
}

/// No-leakage regression guard: two recipes in one Cookfile each observe
/// only their own name. A stale or un-re-stamped shared slot — or reading a
/// session-global instead of the per-body slot — would let the second body
/// see the first's name.
#[test]
fn recipe_name_does_not_leak_between_recipes() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("first", {}, function()
    cook.exec(cook.recipe_name(), 1)
end)
cook.recipe("second", {}, function()
    cook.exec(cook.recipe_name(), 1)
end)
"#;
    let registered = register_cookfile(rt, lua_src, None)
        .unwrap_or_else(|e| panic!("register_cookfile failed: {e:?}"));
    let first = registered.units_by_recipe.get("first").expect("first registered");
    let second = registered.units_by_recipe.get("second").expect("second registered");
    match &first.units[0].payload {
        WorkPayload::Shell { cmd, .. } => assert_eq!(cmd, "first"),
        other => panic!("expected Shell payload, got: {:?}", other),
    }
    match &second.units[0].payload {
        WorkPayload::Shell { cmd, .. } => assert_eq!(
            cmd, "second",
            "second body must not observe the first body's recipe name"
        ),
        other => panic!("expected Shell payload, got: {:?}", other),
    }
}

/// A non-empty `qualified_prefix` yields the qualified name ("lib.build"),
/// not the bare one — the settled design decision (`cook.recipe_name()`
/// mirrors `cook.add_test`'s already-normative qualified `suite` default,
/// Standard §22.4).
#[test]
fn recipe_name_returns_qualified_name_with_prefix() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path()).with_qualified_prefix("lib".to_string());
    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.exec(cook.recipe_name(), 1)
end)
"#;
    let result = register_one(rt, lua_src, "build");
    match &result.units[0].payload {
        WorkPayload::Shell { cmd, .. } => assert_eq!(cmd, "lib.build"),
        other => panic!("expected Shell payload, got: {:?}", other),
    }
}

/// A top-level call (outside any recipe body — the body slot is `None`
/// during top-level load) is a hard error naming the API and citing the
/// spec.
#[test]
fn recipe_name_errors_outside_recipe_body() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe_name()
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "top-level call must error");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("cook.recipe_name"), "error must name the API; got: {err}");
    assert!(err.contains("recipe body"), "error must state the inside-a-recipe-body requirement; got: {err}");
    assert!(err.contains("Standard \u{00a7}22.7") && err.contains("CS-0141"), "error must cite the spec; got: {err}");
}

/// A reachable, `for_each`-feeding probe's `produce` body runs on the
/// register VM before the body loop opens the body slot (`run_for_each_prepass`
/// runs before recipe bodies are invoked), so `cook.recipe_name()` inside it
/// must also hard error. Must be `for_each`-feeding (an ordinary probe's
/// `produce` never runs on the register VM at all — it runs in the execute
/// VM worker pool, where `cook.recipe_name` isn't registered — so a plain
/// probe would pass vacuously here) and reachable (no `target_recipe` is set,
/// so every driver is in scope).
#[test]
fn recipe_name_errors_inside_for_each_feeding_probe_produce() {
    let dir = TempDir::new().unwrap();
    let cookfile = r#"
register
    cook.probe("cards", {
        inputs = {},
        produce = [[ return cook.recipe_name() ]],
    })

recipe deal
    ingredients cards
    cook "build/$<in.id>.txt" {
        mkdir -p build
        printf '%s\n' "$<in.name>" > $<out>
    }
"#;
    let result = register_surface(dir.path(), cookfile);
    assert!(
        result.is_err(),
        "cook.recipe_name() in a for_each-feeding probe's produce must error"
    );
    let err = result.err().unwrap().to_string();
    assert!(err.contains("cook.recipe_name"), "error must name the API; got: {err}");
    assert!(err.contains("recipe body"), "error must state the inside-a-recipe-body requirement; got: {err}");
    assert!(err.contains("Standard \u{00a7}22.7") && err.contains("CS-0141"), "error must cite the spec; got: {err}");
}

// -----------------------------------------------------------------------
// cook.require_recipe() tests (Standard §22.8, CS-0144)
// -----------------------------------------------------------------------

/// Build a minimal Lua VM with `cook.require_recipe` wired against a
/// caller-supplied body slot, pre-stamped as if a recipe body named
/// `current_recipe_bare` were already active — mirroring the stamp
/// `engine.rs` performs immediately before invoking a recipe body. Lets the
/// accumulator (`dynamic_requires`) and the self-reference check be
/// exercised directly: `dynamic_requires` isn't surfaced anywhere else yet
/// (forcing/merging into `requires` is a later task), so there is no
/// observable side effect to assert on through the full `register_cookfile`
/// pipeline. Mirrors the existing `make_unit_api_lua` scaffold below.
fn require_recipe_vm(current_recipe_bare: &str) -> (mlua::Lua, SharedBodySlot) {
    let lua = mlua::Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    let mut body = BodyCaptureState::new();
    body.current_recipe = Some(current_recipe_bare.to_string());
    body.current_recipe_bare = Some(current_recipe_bare.to_string());
    let body_slot: SharedBodySlot = std::rc::Rc::new(std::cell::RefCell::new(Some(body)));
    // A no-op forcer: this scaffold exercises the guard rails and the
    // accumulator, all of which sit ahead of the forcing, and there is no
    // driver here to force against. It must be present rather than absent —
    // the body slot is stamped as active, so an empty forcer cell would
    // (correctly) hard-error as a missing driver. The end-to-end tests below
    // drive the real driver through `register_cookfile`.
    let forcer: crate::context::SharedRecipeForcer = std::rc::Rc::new(std::cell::RefCell::new(
        Some(std::rc::Rc::new(|_: &mlua::Lua, _: &str| Ok(()))),
    ));
    crate::context::register_require_recipe_api(&lua, body_slot.clone(), forcer).unwrap();
    (lua, body_slot)
}

/// A top-level call (outside any recipe body — the body slot is `None`
/// during top-level load) is a hard error naming the API and citing the
/// spec. Mirrors `recipe_name_errors_outside_recipe_body`.
#[test]
fn require_recipe_errors_outside_recipe_body_top_level() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.require_recipe("anything")
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "top-level call must error");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(err.contains("recipe body"), "error must state the inside-a-recipe-body requirement; got: {err}");
    assert!(err.contains("Standard \u{00a7}22.8") && err.contains("CS-0144"), "error must cite the spec; got: {err}");
}

/// A surface `register` block lowers to top-level Lua that runs before the
/// body-invocation loop opens the body slot, so a call there must also hard
/// error — same underlying `body_slot`/`current_recipe` `None` signal as
/// the top-level case, exercised through the real surface parser + codegen
/// this time rather than hand-written Lua.
#[test]
fn require_recipe_errors_outside_recipe_body_register_block() {
    let dir = TempDir::new().unwrap();
    let cookfile = r#"
register
    cook.require_recipe("build")

recipe build
    cook "out.txt" {
        echo build > $<out>
    }
"#;
    let result = register_surface(dir.path(), cookfile);
    assert!(result.is_err(), "a register block call must error");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(err.contains("recipe body"), "error must state the inside-a-recipe-body requirement; got: {err}");
    assert!(err.contains("Standard \u{00a7}22.8") && err.contains("CS-0144"), "error must cite the spec; got: {err}");
}

/// A non-string argument is rejected outright — matched on the raw Lua
/// value (CS-0143's `parse_origin_meta` precedent), never coerced. An
/// integer must NOT be silently turned into its decimal string.
#[test]
fn require_recipe_rejects_non_string_argument() {
    let (lua, _slot) = require_recipe_vm("build");
    let err = lua
        .load(r#"cook.require_recipe(42)"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(err.contains("`name`") && err.contains("must be a string"), "error must name the field and accepted form; got: {err}");
    assert!(!err.contains("42"), "the integer must not be coerced into the message; got: {err}");
    assert!(err.contains("Standard \u{00a7}22.8") && err.contains("CS-0144"), "error must cite the spec; got: {err}");
}

/// An empty string is rejected — an empty dependency name is never
/// meaningful.
#[test]
fn require_recipe_rejects_empty_string() {
    let (lua, _slot) = require_recipe_vm("build");
    let err = lua
        .load(r#"cook.require_recipe("")"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(err.contains("non-empty"), "error must reject the empty string; got: {err}");
    assert!(err.contains("Standard \u{00a7}22.8") && err.contains("CS-0144"), "error must cite the spec; got: {err}");
}

/// Self-reference — the argument equals the enclosing recipe's own bare
/// name — is a hard error naming the recipe, distinct from the general
/// cycle-detection path (a one-element cycle deserves its own message).
#[test]
fn require_recipe_rejects_self_reference() {
    let (lua, _slot) = require_recipe_vm("build");
    let err = lua
        .load(r#"cook.require_recipe("build")"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(err.contains("build"), "error must name the recipe; got: {err}");
    assert!(err.contains("itself") || err.contains("self"), "error must describe the self-reference; got: {err}");
    assert!(err.contains("Standard \u{00a7}22.8") && err.contains("CS-0144"), "error must cite the spec; got: {err}");
}

/// Two calls naming the same recipe record exactly one entry, in the order
/// first seen.
#[test]
fn require_recipe_dedups_repeated_names() {
    let (lua, slot) = require_recipe_vm("build");
    lua.load(
        r#"
        cook.require_recipe("a")
        cook.require_recipe("b")
        cook.require_recipe("a")
    "#,
    )
    .exec()
    .unwrap();
    let got = slot.borrow().as_ref().unwrap().dynamic_requires.clone();
    assert_eq!(got, vec!["a".to_string(), "b".to_string()], "must dedup while preserving first-seen order");
}

/// The critical regression guard: `current_recipe` holds the QUALIFIED
/// name (`engine.rs` stamps `body.current_recipe = Some(qualified_name)`),
/// but `cook.require_recipe`'s argument is bare (the settled design
/// ruling). Registering `build` under a qualified prefix (`lib.build`) and
/// then calling `cook.require_recipe("build")` (bare) from inside its own
/// body MUST still be caught as a self-reference — comparing against the
/// qualified `current_recipe` instead of `current_recipe_bare` would make
/// this silently pass (dynamic_requires would just gain a stray "build"
/// entry and register_cookfile would return Ok).
#[test]
fn require_recipe_self_reference_detected_under_qualified_prefix() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path()).with_qualified_prefix("lib".to_string());
    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.require_recipe("build")
end)
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(
        result.is_err(),
        "self-reference must be caught even under a qualified prefix (bare-vs-qualified mix-up would silently pass)"
    );
    let err = result.err().unwrap().to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(err.contains("build"), "error must name the recipe; got: {err}");
    assert!(err.contains("itself") || err.contains("self"), "error must describe the self-reference; got: {err}");
}

/// A dependency on a DIFFERENT recipe, called from inside a real recipe
/// body reached via the full `register_cookfile` pipeline (not the
/// low-level VM scaffold), must not error — proves the guard rails don't
/// false-positive on the ordinary case.
#[test]
fn require_recipe_accepts_distinct_recipe_name() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.require_recipe("other")
    cook.exec("echo hi", 1)
end)
cook.recipe("other", {}, function()
    cook.exec("echo other", 1)
end)
"#;
    let result = register_one(rt, lua_src, "build");
    assert_eq!(result.units.len(), 1, "require_recipe must not itself capture a unit");
}

// -----------------------------------------------------------------------
// cook.require_recipe() — forcing, the edge, and the skip arms
// (Standard §22.8, CS-0144)
// -----------------------------------------------------------------------

/// Lower a surface Cookfile and register it against an explicit build
/// target. `register_surface`'s no-target sibling can't reach the
/// `target_recipe.is_some()` skip arms at all.
fn register_surface_target(
    dir: &std::path::Path,
    cookfile: &str,
    target: &str,
) -> Result<RegisteredCookfile, RegisterError> {
    let parsed = cook_lang::parse(cookfile).expect("fixture must parse");
    let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&parsed);
    let lua_src = cook_luagen::generate_with_names_checked(&parsed, &recipe_names)
        .expect("fixture must lower");
    let rt = make_registry(dir).with_target_argv(target.to_string(), vec![]);
    register_cookfile(rt, &lua_src, None)
}

/// Pull the `requires` recorded for `name` off the registered set.
fn requires_of(registered: &RegisteredCookfile, name: &str) -> Vec<String> {
    registered
        .names
        .iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("recipe {name:?} not registered"))
        .requires
        .clone()
}

/// Assert the single shell command captured for `name`.
fn only_shell_cmd(registered: &RegisteredCookfile, name: &str) -> String {
    let units = &registered
        .units_by_recipe
        .get(name)
        .unwrap_or_else(|| panic!("recipe {name:?} has no units entry"))
        .units;
    assert_eq!(units.len(), 1, "recipe {name:?} must capture exactly one unit");
    match &units[0].payload {
        WorkPayload::Shell { cmd, .. } => cmd.clone(),
        other => panic!("expected Shell payload for {name:?}, got: {other:?}"),
    }
}

/// THE headline case — the finding this whole API exists to fix.
///
/// `consumer` is declared FIRST and carries no static dep on `producer`, so
/// registration order (dependency-driven, never declaration order) would run
/// its body first and `cook.import("producer")` would return **nil, silently**
/// — the maker would then emit a link line missing the library. The
/// `cook.require_recipe("producer")` call is the *only* thing that forces
/// `producer`'s body to completion first, so the export is observable when the
/// call returns.
#[test]
fn require_recipe_forces_producer_body_so_import_resolves() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("consumer", {}, function()
    cook.require_recipe("producer")
    local info = cook.import("producer")
    cook.exec("link " .. tostring(info and info.lib_path or "NIL"), 1)
end)
cook.recipe("producer", {}, function()
    cook.export("producer", { lib_path = "build/libproducer.a" })
end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "consumer"),
        "link build/libproducer.a",
        "cook.import must resolve to producer's export, not nil — the register-order \
         guarantee is what makes this differ from \"link NIL\""
    );
}

/// A body is evaluated at most once per pass: two requirers of the same
/// recipe, plus the seed loop's own visit of it, yield exactly one
/// evaluation.
#[test]
fn require_recipe_evaluates_forced_body_exactly_once() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
_G.runs = 0
cook.recipe("a", {}, function() cook.require_recipe("shared") end)
cook.recipe("b", {}, function() cook.require_recipe("shared") end)
cook.recipe("shared", {}, function()
    _G.runs = _G.runs + 1
    cook.exec("shared run " .. _G.runs, 1)
end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "shared"),
        "shared run 1",
        "shared's body must be evaluated exactly once despite two requirers plus the seed loop"
    );
}

/// Both re-entrancy hazards at once. A nested invocation shares the single
/// `body_slot` and rebinds the Lua `recipe` global, so without save/restore
/// the callee's units land in the caller's recipe and the caller sees
/// `recipe.name == "producer"` after the call returns.
#[test]
fn require_recipe_restores_caller_body_slot_and_recipe_global() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("consumer", {}, function()
    cook.exec("before " .. recipe.name, 1)
    cook.require_recipe("producer")
    cook.exec("after " .. recipe.name, 2)
end)
cook.recipe("producer", {}, function()
    cook.exec("producer body", 1)
end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    let consumer = &registered.units_by_recipe.get("consumer").unwrap().units;
    let cmds: Vec<&str> = consumer
        .iter()
        .map(|u| match &u.payload {
            WorkPayload::Shell { cmd, .. } => cmd.as_str(),
            other => panic!("expected Shell, got {other:?}"),
        })
        .collect();
    assert_eq!(
        cmds,
        vec!["before consumer", "after consumer"],
        "the caller's units and its `recipe` global must survive the nested invocation"
    );
    assert_eq!(only_shell_cmd(&registered, "producer"), "producer body");
}

/// The edge lands in `RegisteredRecipePub.requires` — the single source of
/// truth the engine's analyzer builds the closure from — and dedups against
/// a static dep naming the same recipe.
#[test]
fn require_recipe_edge_merges_into_requires_deduped_against_static_dep() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("consumer", {requires = {"producer"}}, function()
    cook.require_recipe("producer")
    cook.require_recipe("extra")
end)
cook.recipe("producer", {}, function() end)
cook.recipe("extra", {}, function() end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        requires_of(&registered, "consumer"),
        vec!["producer".to_string(), "extra".to_string()],
        "static + dynamic must merge to ONE producer entry, static order first"
    );
}

/// Every `cook.recipe` registration completes before any body runs, so a
/// name that isn't registered is definitively unknown at call time.
#[test]
fn require_recipe_unknown_name_errors_with_fix_hint() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("consumer", {}, function()
    cook.require_recipe("produsers")
end)
cook.recipe("producer", {}, function() end)
"#;
    let err = register_cookfile(rt, lua_src, None)
        .expect_err("an unregistered name must be a hard error")
        .to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(err.contains("produsers"), "error must name the unknown recipe; got: {err}");
    assert!(err.contains("consumer"), "error must name the requiring recipe; got: {err}");
    assert!(err.contains("producer"), "fix hint must list the closest registered name; got: {err}");
}

/// Forcing is synchronous, so a dynamic cycle would recurse without bound.
/// It must surface as a rendered cycle path, not a stack overflow.
#[test]
fn require_recipe_dynamic_cycle_errors_with_path() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("framework", {}, function() cook.require_recipe("idLib") end)
cook.recipe("idLib", {}, function() cook.require_recipe("framework") end)
"#;
    let err = register_cookfile(rt, lua_src, None)
        .expect_err("a dynamic cycle must be a hard error")
        .to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(
        err.contains("framework -> idLib -> framework"),
        "error must render the cycle path; got: {err}"
    );
}

/// A Cookfile that never calls the API keeps the pre-existing body order:
/// the seed loop still walks the static topo sort.
#[test]
fn body_order_unchanged_without_require_recipe_calls() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("z", {}, function() end)
cook.recipe("a", {requires = {"z"}}, function() end)
cook.recipe("m", {}, function() end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    let order: Vec<&str> = registered.names.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        order,
        vec!["z", "a", "m"],
        "static topo order (z before a; m last) must be untouched"
    );
}

/// Skip arm 1 — parametric chore, a target IS requested but this isn't it and
/// the chore isn't statically reachable from it. A forced chore is reachable
/// by definition, so it must run with empty argv (§7.5.1) and register its
/// units, not skip to zero.
#[test]
fn require_recipe_forces_parametric_chore_when_target_requested() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path()).with_target_argv("app".to_string(), vec![]);
    let registered =
        register_cookfile(rt, &parametric_chore_fixture("gen"), None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "gen"),
        "generate world",
        "a forced parametric chore must be invoked with empty argv, not skipped"
    );
    assert_eq!(requires_of(&registered, "app"), vec!["gen".to_string()]);
}

/// Skip arm 2 — parametric chore, NO target requested. Gated only on
/// `target_recipe.is_none() && Chore && params`, never on reachability, so a
/// fix that only touches arm 1 leaves this hole open. This is the arm
/// `cook list`, `cook dag`, and the default test harness all take.
#[test]
fn require_recipe_forces_parametric_chore_with_no_target() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let registered =
        register_cookfile(rt, &parametric_chore_fixture("gen"), None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "gen"),
        "generate world",
        "a forced parametric chore must be invoked with empty argv on the no-target path too"
    );
}

/// Skip arm 1, SEEDED FIRST. The two tests above pass on a driver that
/// conflates "body ran" with "body was deliberately skipped", but only by an
/// accident of naming: the seed loop walks a `BTreeMap::keys()` topo sort —
/// LEXICOGRAPHIC — so `app` precedes `gen` and the force always lands before
/// the seed visit. Rename the chore `agen` and the accident reverses: the
/// seed loop reaches it FIRST with `forced = false`, it hits the skip arm,
/// and a `Visited`-marking driver then short-circuits the later force to
/// `Ok(())` — zero units registered while the edge still places `agen` in the
/// build closure, the exact state §22.8 declares expressly non-conforming.
#[test]
fn require_recipe_forces_parametric_chore_seeded_before_its_requirer() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path()).with_target_argv("app".to_string(), vec![]);
    let registered =
        register_cookfile(rt, &parametric_chore_fixture("agen"), None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "agen"),
        "generate world",
        "a deliberate skip must not satisfy a later force: the seed loop reaches `agen` \
         first (it sorts before `app`), so the force has to RE-invoke the skipped body"
    );
    assert_eq!(requires_of(&registered, "app"), vec!["agen".to_string()]);
}

/// Skip arm 2, SEEDED FIRST — the no-target twin of the test above, the arm
/// `cook list` and `cook dag` take.
#[test]
fn require_recipe_forces_parametric_chore_seeded_before_its_requirer_no_target() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let registered =
        register_cookfile(rt, &parametric_chore_fixture("agen"), None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "agen"),
        "generate world",
        "the skip-then-force order must be rescued on the no-target path too"
    );
}

/// A re-invoked skip arm must not double-register. `register_skipped` already
/// pushed `agen` into `names` and `units_by_recipe`; the forced re-invocation
/// registers it again, and without dedup the recipe appears twice in the
/// discovered set (§22.6) with the second `names` entry shadowing nothing and
/// confusing every listing surface.
#[test]
fn require_recipe_reinvoked_skip_arm_registers_exactly_one_entry() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let registered =
        register_cookfile(rt, &parametric_chore_fixture("agen"), None).unwrap();
    let entries: Vec<&str> = registered
        .names
        .iter()
        .map(|r| r.name.as_str())
        .filter(|n| *n == "agen")
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "a skipped-then-forced recipe must yield ONE registered entry, not one per \
         invocation; got: {:?}",
        registered.names.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
}

/// Shared by the parametric-chore skip-arm tests: `app` (a plain recipe, the
/// only viable target) forces the parametric chore `chore_name`, which neither
/// statically requires nor is required by it. `chore_name` is a parameter
/// precisely because the seed loop's order is lexicographic — a fixture that
/// only ever names the chore `gen` can never exercise the seeded-first case.
fn parametric_chore_fixture(chore_name: &str) -> String {
    format!(
        r#"
cook.recipe("app", {{}}, function()
    cook.require_recipe("{chore_name}")
end)
cook.__register_surface_chore("{chore_name}",
    {{requires = {{}}, __line = 5,
     __params = {{ {{ kind = "defaulted_string", name = "who", default = "world" }} }}}},
    function(__cook_params)
        cook.exec("generate " .. __cook_params.who, 1)
    end)
"#
    )
}

/// Skip arm 3 — a probe-sourced `for_each` recipe that isn't statically
/// reachable from the target had its feeding probe skipped by the pre-pass,
/// so its body's `cook.probes.get` cannot resolve. Forcing it is a hard error
/// with a fix hint, NOT a silent registration of zero units (which would let
/// register and engine disagree about what got built).
#[test]
fn require_recipe_on_unreachable_probe_for_each_errors_with_fix_hint() {
    let dir = TempDir::new().unwrap();
    let cookfile = r#"
register
    cook.probe("items", { inputs = {}, produce = [[ return {{id = "x"}} ]] })
    cook.recipe("app", {}, function()
        cook.require_recipe("fan")
    end)

recipe fan
    ingredients items
    cook "build/$<in.id>.txt" {
        mkdir -p build
        printf '%s' "$<in.id>" > $<out>
    }
"#;
    let err = register_surface_target(dir.path(), cookfile, "app")
        .expect_err("forcing an unreachable probe-sourced for_each recipe must be a hard error")
        .to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(err.contains("fan"), "error must name the recipe; got: {err}");
    assert!(
        err.contains("for_each") && err.contains("probe"),
        "error must state the reason; got: {err}"
    );
    assert!(
        err.contains(": fan"),
        "fix hint must tell the author to add a static `: fan` dep to the requiring recipe's \
         header; got: {err}"
    );
}

/// Skip arm 3, SEEDED FIRST. The same lexicographic accident that hides the
/// parametric-chore holes also defeats this arm's DESIGNED hard error: name
/// the `for_each` recipe `afan` and the seed loop reaches it before `app`,
/// marks the skip as a completed visit, and the later force short-circuits to
/// `Ok(())` — the diagnostic never fires and `app` links against a recipe that
/// registered nothing.
#[test]
fn require_recipe_on_probe_for_each_seeded_first_still_errors() {
    let dir = TempDir::new().unwrap();
    let cookfile = r#"
register
    cook.probe("items", { inputs = {}, produce = [[ return {{id = "x"}} ]] })
    cook.recipe("app", {}, function()
        cook.require_recipe("afan")
    end)

recipe afan
    ingredients items
    cook "build/$<in.id>.txt" {
        mkdir -p build
        printf '%s' "$<in.id>" > $<out>
    }
"#;
    let err = register_surface_target(dir.path(), cookfile, "app")
        .expect_err(
            "a deliberate skip must not satisfy a later force: `afan` seeds before `app`, \
             so the arm-3 error has to fire on the force",
        )
        .to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(err.contains("afan"), "error must name the recipe; got: {err}");
    assert!(
        err.contains("for_each") && err.contains("probe"),
        "error must state the reason; got: {err}"
    );
}

/// CRITICAL — the rebind hole. `local rr = cook.require_recipe` at top level
/// is ordinary Lua (`local sh = cook.sh` is idiomatic), and the top-level
/// chunk runs BEFORE the driver exists. An alias therefore captures whatever
/// closure was installed at API-install time; if that closure carries its own
/// forcer by value, the alias holds a forcer-less one that still passes the
/// guard rail and still accumulates the edge — recording the dependency
/// WITHOUT the forcing that makes it mean anything. `cook.import` then
/// returns nil, silently, which is the precise failure this API exists to
/// eliminate.
#[test]
fn require_recipe_aliased_at_top_level_still_forces() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
local rr = cook.require_recipe
cook.recipe("consumer", {}, function()
    rr("producer")
    local info = cook.import("producer")
    cook.exec("link " .. tostring(info and info.lib_path or "NIL"), 1)
end)
cook.recipe("producer", {}, function()
    cook.export("producer", { lib_path = "build/libP.a" })
end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "consumer"),
        "link build/libP.a",
        "an aliased `cook.require_recipe` must force exactly as the un-aliased call does; \
         \"link NIL\" means the alias recorded the edge but skipped the forcing"
    );
    assert_eq!(
        requires_of(&registered, "consumer"),
        vec!["producer".to_string()],
        "the alias must record the edge too"
    );
}

/// A force error swallowed by `pcall` must not poison the visit map. The
/// driver marks a body `Visiting` on the way in; if the error path leaves that
/// mark behind, the seed loop's own later visit sees it and reports a
/// one-element "cycle" that does not exist — while the real failure, already
/// swallowed by the `pcall`, is gone for good. The build then fails with a
/// fabricated diagnostic pointing at the wrong problem.
#[test]
fn require_recipe_force_error_swallowed_by_pcall_surfaces_real_error_not_a_cycle() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("consumer", {}, function()
    pcall(function() cook.require_recipe("prod") end)
    cook.exec("consumer ran", 1)
end)
cook.recipe("prod", {}, function()
    error("boom: the real failure")
end)
"#;
    let err = register_cookfile(rt, lua_src, None)
        .expect_err("prod's body raises, so the pass must fail")
        .to_string();
    assert!(
        err.contains("boom: the real failure"),
        "the REAL error must survive the pcall-swallowed force; got: {err}"
    );
    assert!(
        !err.contains("cycle"),
        "a swallowed force must not fabricate a cycle out of the abandoned `Visiting` \
         mark; got: {err}"
    );
}

/// The OTHER half of `VisitState::Failed`: a body that raised MUST NOT re-run,
/// and the original error MUST be replayed verbatim on the second force.
///
/// Sibling of the test above, which pins the "don't leave `Visiting` behind"
/// half. Clearing the entry on failure instead of recording `Failed` would also
/// avoid the fabricated cycle, so that test alone does not distinguish the two
/// designs — this one does. §22.8: "A recipe body MUST be evaluated at most
/// once per registration pass", with no exemption for a body that raised, and
/// a `pcall`-swallowed force is exactly how a second force becomes reachable.
///
/// `flaky` succeeds on a second evaluation and fails on its first, so a re-run
/// is directly observable: `runs` reaches 2 and the replayed error goes `nil`.
#[test]
fn require_recipe_failed_body_is_not_re_evaluated_and_replays_original_error() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    // `consumer` sorts before `flaky`, so the seed loop reaches it first and
    // both forces happen from inside its body. It reports what it observed by
    // raising, which is the only channel out of a pass that must fail anyway.
    let lua_src = r#"
local runs = 0
cook.recipe("consumer", {}, function()
    local ok1 = pcall(function() cook.require_recipe("flaky") end)
    local ok2, err2 = pcall(function() cook.require_recipe("flaky") end)
    error("PROBE runs=" .. runs .. " ok1=" .. tostring(ok1)
          .. " ok2=" .. tostring(ok2) .. " err2=[" .. tostring(err2) .. "]")
end)
cook.recipe("flaky", {}, function()
    runs = runs + 1
    if runs == 1 then error("boom: only the first evaluation fails") end
    cook.exec("flaky re-ran", 1)
end)
"#;
    let err = register_cookfile(rt, lua_src, None)
        .expect_err("flaky's body raises, so the pass must fail")
        .to_string();
    assert!(
        err.contains("runs=1"),
        "the failed body MUST NOT be re-evaluated by the second force (§22.8 at-most-once); \
         `runs=2` means `Failed` cleared the entry instead of recording it. Got: {err}"
    );
    assert!(
        err.contains("ok1=false") && err.contains("ok2=false"),
        "both forces must fail — the second inherits the first's failure rather than \
         succeeding on a re-run; got: {err}"
    );
    assert!(
        err.contains("boom: only the first evaluation fails"),
        "the second force MUST replay the ORIGINAL error verbatim, not swallow it and not \
         report a fabricated one; got: {err}"
    );
}

/// A cycle whose two edges have DIFFERENT sources — `aaa : bbb` static, plus
/// `bbb`'s body forcing `aaa` — MUST be rejected with the cycle PATH rendered.
/// §22.8: the cycle is in the edges "taken together with all other cross-recipe
/// edges". Leaving it to the engine's analyzer would not discharge that:
/// `GraphError::CycleDetected` names one node, renders no path, and is scoped
/// to the target's closure — so `cook list` would see nothing at all.
///
/// The driver's `Visiting` arm is what raises it, since `ensure_static_requires`
/// makes the driver walk STATIC edges too: the seed loop marks `bbb` `Visiting`
/// (the static sort puts a dep before its dependent), `bbb`'s body forces `aaa`,
/// and `aaa`'s static dep on `bbb` recurses straight back into the `Visiting`
/// mark — yielding `cook.require_recipe: dependency cycle: bbb -> aaa -> bbb`.
/// Step 12a's merged-`requires` sort would also catch it, but never gets the
/// chance; it is a guard, not the live path (see `engine.rs` step 12a).
#[test]
fn require_recipe_mixed_static_dynamic_cycle_errors_with_path() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("aaa", {requires = {"bbb"}}, function() end)
cook.recipe("bbb", {}, function() cook.require_recipe("aaa") end)
"#;
    let err = register_cookfile(rt, lua_src, None)
        .expect_err("a static+dynamic cycle must be rejected at register phase")
        .to_string();
    assert!(
        err.contains("aaa") && err.contains("bbb"),
        "the diagnostic must render the cycle path across both recipes; got: {err}"
    );
    assert!(
        err.contains("->"),
        "the diagnostic must render the cycle as a PATH, not name a single node; got: {err}"
    );
}

/// A forced body must not jump ahead of its OWN static `requires`.
///
/// The seed loop's `topo` order puts `tools` before `gen` (`gen : tools`), and
/// a driver that leans on that order for correctness is fine right up until a
/// force bypasses the seed loop: `app` sorts first, its body forces `gen`, and
/// `gen`'s body then runs at seed index 0 — before `tools` has been reached at
/// seed index 1. `gen`'s own `cook.import("tools")` resolves **nil**, so the
/// mislink is PARTIAL: the force itself worked (`app` does see `gen`'s export)
/// while `gen` silently linked against nothing. That is worse than an outright
/// failure, and it is the exact silent-failure class §22.8 exists to kill.
///
/// The fix makes `ensure_invoked` a proper DFS over the requires graph, which
/// demotes `topo` from "load-bearing for correctness" to "deterministic tie-break
/// among independent recipes".
#[test]
fn require_recipe_forced_body_evaluates_its_own_static_requires_first() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    // Seed order is what let this through, so pin it explicitly: the static
    // sort of {app: [], gen: [tools], tools: []} is [app, tools, gen] — `app`
    // (the forcer) lands BEFORE `tools` (the forced recipe's own dep).
    let lua_src = r#"
cook.recipe("app", {}, function()
    cook.require_recipe("gen")
    local g = cook.import("gen")
    cook.exec("app " .. tostring(g and g.out or "NIL"), 1)
end)
cook.recipe("gen", {requires = {"tools"}}, function()
    local t = cook.import("tools")
    cook.export("gen", { out = "gen-using-" .. tostring(t and t.bin or "NIL") })
end)
cook.recipe("tools", {}, function()
    cook.export("tools", { bin = "protoc" })
end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "app"),
        "app gen-using-protoc",
        "forcing `gen` must also guarantee `gen`'s own declared dep `tools` is evaluated \
         first — otherwise `gen`'s cook.import(\"tools\") is nil and the mislink is silent \
         (\"app gen-using-NIL\")"
    );
}

/// The same defect without any `cook.import` in the picture: the ordering
/// itself is wrong, not some import artifact. `ccc` forces `ddd`, `ddd : eee`,
/// and the static sort is [ccc, eee, ddd] — so a force that ignores `ddd`'s
/// own dep list runs `ddd` at seed index 0, leaving `eee` unevaluated at the
/// moment `ddd`'s body observes the world.
#[test]
fn require_recipe_forced_body_static_dep_runs_before_it_in_raw_eval_order() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
_G.order = {}
cook.recipe("ccc", {}, function()
    cook.require_recipe("ddd")
end)
cook.recipe("ddd", {requires = {"eee"}}, function()
    table.insert(_G.order, "ddd")
    cook.exec("seen " .. table.concat(_G.order, ","), 1)
end)
cook.recipe("eee", {}, function()
    table.insert(_G.order, "eee")
end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "ddd"),
        "seen eee,ddd",
        "`eee` must have run by the time the forced `ddd`'s body observes eval order; \
         a force that skips its own static deps yields \"seen ddd\""
    );
}

/// Recursing into static `requires` must not evaluate anything twice. `ddd` is
/// reached three ways — forced directly by `ccc`, pulled in as `bbb`'s static
/// dep, and visited by the seed loop — and §22.8's at-most-once rule holds
/// across all of them. A naive "invoke my deps, then me" that forgets to
/// consult the visit map would run `ddd` once per inbound edge.
#[test]
fn require_recipe_static_dep_forced_by_two_requirers_evaluates_once() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
_G.runs = 0
cook.recipe("aaa", {}, function()
    cook.require_recipe("bbb")
    cook.require_recipe("ddd")
end)
cook.recipe("bbb", {requires = {"ddd"}}, function() end)
cook.recipe("ccc", {}, function()
    cook.require_recipe("ddd")
end)
cook.recipe("ddd", {}, function()
    _G.runs = _G.runs + 1
    cook.exec("ddd run " .. _G.runs, 1)
end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "ddd"),
        "ddd run 1",
        "a recipe reached as a forced target, as a forced requirer's static dep, and by \
         the seed loop must still be evaluated exactly once"
    );
}

/// The DFS must carry the force DOWN the static chain, not just to its first
/// hop. `app` forces `gen`; `gen : agen` where `agen` is a parametric chore
/// that every skip arm declines to invoke unless `forced`. `agen` sorts before
/// `app`, so the seed loop reaches and skips it first. If the recursion visits
/// `agen` un-forced, the skip stands, `agen` registers zero units — while the
/// `gen : agen` edge still puts it in the build closure. Register and engine
/// then disagree about what got built, which is the same non-conformance the
/// direct-force case already forbids.
#[test]
fn require_recipe_force_propagates_through_static_dep_to_skipped_chore() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path()).with_target_argv("app".to_string(), vec![]);
    let lua_src = r#"
cook.recipe("app", {}, function()
    cook.require_recipe("gen")
end)
cook.recipe("gen", {requires = {"agen"}}, function() end)
cook.__register_surface_chore("agen",
    {requires = {}, __line = 5,
     __params = { { kind = "defaulted_string", name = "who", default = "world" } }},
    function(__cook_params)
        cook.exec("generate " .. __cook_params.who, 1)
    end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "agen"),
        "generate world",
        "the force must reach `agen` THROUGH `gen`'s static dep list: a transitively \
         forced recipe is in the build closure just as surely as a directly forced one"
    );
}

/// The `Visited` short-circuit must still propagate `forced` INTO static deps.
///
/// Seed order is a test parameter, not a fixture detail — so the names here are
/// chosen to make the bug fire, and the previous round's test is the reason the
/// point needs restating. `require_recipe_force_propagates_through_static_dep_
/// to_skipped_chore` named its forcer `app`: `app` sorts BEFORE `gen`, so `gen`
/// was still unvisited when the force arrived, the force took the `None` arm,
/// and the propagation ran. Rename the forcer `zzz` and the accident reverses.
///
/// `topo` is seeded from `BTreeMap::keys()` — LEXICOGRAPHIC — so
/// {achore: [], bbb: [achore], zzz: []} seeds [achore, bbb, zzz]:
///   1. `achore` — parametric chore, un-forced, unreachable → skip arm, `Skipped`.
///   2. `bbb` — its static dep `achore` is visited un-forced (the skip stands,
///      correctly: nothing has forced anything yet); `bbb`'s body runs →`Visited`.
///   3. `zzz` — body forces `bbb`, which is already `Visited`, so the force
///      returns `Ok(())` BEFORE `ensure_static_requires` — and `achore` is
///      never rescued.
///
/// `achore` then registers zero units while `zzz -> bbb -> achore` sits in the
/// build closure the engine builds from `requires`: the register/execute
/// disagreement §22.8 calls expressly non-conforming, and silent.
#[test]
fn require_recipe_force_reaches_static_deps_of_already_visited_recipe() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path()).with_target_argv("zzz".to_string(), vec![]);
    let lua_src = r#"
cook.recipe("zzz", {}, function()
    cook.require_recipe("bbb")
end)
cook.recipe("bbb", {requires = {"achore"}}, function() end)
cook.__register_surface_chore("achore",
    {requires = {}, __line = 5,
     __params = { { kind = "defaulted_string", name = "who", default = "world" } }},
    function(__cook_params)
        cook.exec("generate " .. __cook_params.who, 1)
    end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "achore"),
        "generate world",
        "a force arriving at an ALREADY-VISITED recipe must still push `forced` down into \
         that recipe's static deps: `bbb` ran un-forced at seed time, so `achore`'s skip \
         has never been reconsidered, and only the force can stand it down"
    );
}

/// The same hole on the no-target path — the one `cook list`, `cook dag`, and
/// most tests take. Skip arm 2 is gated on `target_recipe.is_none()`, never on
/// reachability, so a fix verified only against arm 1 leaves this open.
///
/// Names as above and for the same reason: `zzz` sorts after `bbb`, so `bbb` is
/// already `Visited` when the force lands.
#[test]
fn require_recipe_force_reaches_static_deps_of_already_visited_recipe_no_target() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("zzz", {}, function()
    cook.require_recipe("bbb")
end)
cook.recipe("bbb", {requires = {"achore"}}, function() end)
cook.__register_surface_chore("achore",
    {requires = {}, __line = 5,
     __params = { { kind = "defaulted_string", name = "who", default = "world" } }},
    function(__cook_params)
        cook.exec("generate " .. __cook_params.who, 1)
    end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "achore"),
        "generate world",
        "the already-visited force hole must be closed on the no-target path too"
    );
}

/// Skip arm 3 reached THROUGH an already-`Visited` recipe. This is the
/// silent-swallow case, and the sharpest evidence the hole is real: arm 3's
/// error is DESIGNED — forcing an unreachable probe-sourced `for_each` cannot
/// be rescued, so it must be a hard failure. The existing arm-3 tests both
/// force `afan` directly. Route the force through `bbb` (which the seed loop
/// has already evaluated un-forced, because `zzz` sorts after it) and the
/// designed diagnostic simply never fires: the pass SUCCEEDS with `afan` at
/// zero units while `zzz -> bbb -> afan` is in the build closure.
///
/// `afan` is named to sort first (it must be seeded and skipped before anything
/// forces it); `zzz` is named to sort last (so `bbb` is `Visited`, not `None`,
/// at force time). Rename `zzz` to `app` and the bug cannot fire.
#[test]
fn require_recipe_probe_for_each_reached_via_visited_recipe_still_errors() {
    let dir = TempDir::new().unwrap();
    let cookfile = r#"
register
    cook.probe("items", { inputs = {}, produce = [[ return {{id = "x"}} ]] })
    cook.recipe("zzz", {}, function()
        cook.require_recipe("bbb")
    end)
    cook.recipe("bbb", {requires = {"afan"}}, function() end)

recipe afan
    ingredients items
    cook "build/$<in.id>.txt" {
        mkdir -p build
        printf '%s' "$<in.id>" > $<out>
    }
"#;
    let err = register_surface_target(dir.path(), cookfile, "zzz")
        .expect_err(
            "arm 3's designed hard error must still fire when the force reaches `afan` \
             through an already-visited `bbb`; swallowing it registers `afan` at zero \
             units while the edge still builds it",
        )
        .to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(err.contains("afan"), "error must name the recipe; got: {err}");
    assert!(
        err.contains("for_each") && err.contains("probe"),
        "error must state the reason; got: {err}"
    );
    assert!(
        err.contains(": afan"),
        "fix hint must survive the transitive route; got: {err}"
    );
}

/// The shape an author actually writes: `web` forces `assets` (a dynamic edge —
/// `web` needs `assets`' export), and `assets : codegen`. No contrived naming:
/// `web` genuinely sorts after `assets`, so `assets` is already `Visited` when
/// the force lands and `codegen` — a parametric chore — is never stood down.
/// `codegen` registers zero units and ships a `web` build with nothing
/// generated.
#[test]
fn require_recipe_force_reaches_codegen_under_realistic_web_assets_names() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path()).with_target_argv("web".to_string(), vec![]);
    let lua_src = r#"
cook.recipe("web", {}, function()
    cook.require_recipe("assets")
end)
cook.recipe("assets", {requires = {"codegen"}}, function() end)
cook.__register_surface_chore("codegen",
    {requires = {}, __line = 5,
     __params = { { kind = "defaulted_string", name = "lang", default = "ts" } }},
    function(__cook_params)
        cook.exec("codegen " .. __cook_params.lang, 1)
    end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "codegen"),
        "codegen ts",
        "`web` forces `assets`, `assets : codegen` — `codegen` is in the build closure, so \
         it must register its units; `web` sorting after `assets` must not decide that"
    );
}

/// The propagation must CHAIN through consecutive already-visited recipes, not
/// stop at the first hop. Found by auditing the new arm rather than by a
/// report: it recurses through `ensure_static_requires`, so depth ought to fall
/// out for free — but "ought to" is what produced three rounds of this bug, and
/// the arm is only correct if `Visited { forced: false }` reached FROM the
/// propagation re-enters it rather than short-circuiting.
///
/// Names, again, chosen so the bug fires: {achore, bbb: [ccc], ccc: [achore],
/// zzz} seeds [achore, ccc, bbb, zzz], so `ccc` AND `bbb` are both already
/// `Visited { forced: false }` when `zzz`'s force lands, and the rescue of
/// `achore` has to travel `zzz` -> `bbb` -> `ccc` -> `achore` through two
/// consecutive hits of the new arm.
#[test]
fn require_recipe_force_propagates_through_a_chain_of_visited_recipes() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("zzz", {}, function()
    cook.require_recipe("bbb")
end)
cook.recipe("bbb", {requires = {"ccc"}}, function() end)
cook.recipe("ccc", {requires = {"achore"}}, function() end)
cook.__register_surface_chore("achore",
    {requires = {}, __line = 5,
     __params = { { kind = "defaulted_string", name = "who", default = "world" } }},
    function(__cook_params)
        cook.exec("generate " .. __cook_params.who, 1)
    end)
"#;
    let registered = register_cookfile(rt, lua_src, None).unwrap();
    assert_eq!(
        only_shell_cmd(&registered, "achore"),
        "generate world",
        "forcing is transitive to the whole static closure, not one hop: the engine builds \
         the closure from `requires`, so `zzz -> bbb -> ccc -> achore` puts `achore` in it"
    );
}

/// Cookfile fixture shared by the pair of tests below: `mid` statically
/// requires `achore` (a parametric chore), and `achore`'s body forces `mid`
/// back — a mixed static/dynamic cycle in the same shape as
/// `require_recipe_mixed_static_dynamic_cycle_errors_with_path`, except
/// `achore` is gated behind the parametric-chore skip arm, so the cycle is
/// invisible to the ordinary un-forced seed pass (its body never runs) and
/// only surfaces once something forces `mid`. `forcer_name` is that
/// something; it is a parameter, not a fixture detail, because the whole
/// point of both tests is which internal arm the forcer's name routes into.
fn mid_achore_cycle_fixture(forcer_name: &str) -> String {
    format!(
        r#"
cook.recipe("{forcer_name}", {{}}, function()
    cook.require_recipe("mid")
end)
cook.recipe("mid", {{requires = {{"achore"}}}}, function() end)
cook.__register_surface_chore("achore",
    {{requires = {{}}, __line = 5,
     __params = {{ {{ kind = "defaulted_string", name = "who", default = "world" }} }}}},
    function(__cook_params)
        cook.require_recipe("mid")
    end)
"#
    )
}

/// Pull the `X -> Y -> X` path out of a `cook.require_recipe` cycle
/// diagnostic and normalize it to a canonical rotation: drop the repeated
/// closing node, then rotate to start at the lexicographically smallest
/// name. A cycle has no privileged starting point — which node the
/// `Visiting` check happens to catch first is an implementation accident,
/// not part of the cycle's identity — so two renderings of the SAME cycle
/// that merely start at different nodes must compare equal once normalized.
fn canonical_cycle_path(err: &str) -> Vec<String> {
    let marker = "dependency cycle: ";
    let start = err
        .find(marker)
        .unwrap_or_else(|| panic!("no cycle marker in: {err}"))
        + marker.len();
    let rest = &err[start..];
    let end = rest
        .find(". Forcing is synchronous")
        .unwrap_or_else(|| panic!("no cycle terminator in: {err}"));
    let mut nodes: Vec<String> = rest[..end].split("->").map(|s| s.trim().to_string()).collect();
    assert!(
        nodes.len() > 1 && nodes.first() == nodes.last(),
        "a cycle path must repeat its first node at the end; got: {nodes:?}"
    );
    nodes.pop();
    let min_idx = nodes
        .iter()
        .enumerate()
        .min_by_key(|(_, n)| n.as_str())
        .map(|(i, _)| i)
        .unwrap_or(0);
    nodes.rotate_left(min_idx);
    nodes
}

/// The `Visited { forced: false }` arm's `ensure_static_requires` re-walk
/// does not push `name` onto `self.path` before recursing, while the normal
/// (`None`-state) body path does (the `path.borrow_mut().push` right before
/// `visit_requires_then_body`). `self.path` is exactly what the `Visiting`
/// arm reads to render a cycle, so a cycle discovered THROUGH the
/// already-visited arm renders with a link missing from the stack — and here
/// that missing link is `mid` itself, so the diagnostic collapses to
/// `achore -> achore`: a one-element self-cycle `achore` never actually has
/// (it requires nothing; `mid` is the one with the static `requires`). That
/// is precisely the fabricated-cycle shape `VisitState::Failed` exists to
/// prevent elsewhere in this driver, and it violates §22.8's MUST to render
/// the cycle PATH, not name one node.
///
/// `zzz` is the forcer's name FOR A REASON, not a placeholder: `topo` seeds
/// lexicographically from `BTreeMap::keys()`, so `{achore, mid, zzz}` seeds
/// `[achore, mid, zzz]` — `mid` is visited un-forced (`Visited { forced:
/// false }`) before `zzz`'s turn arrives, so `zzz`'s force lands on the
/// buggy `Visited { forced: false }` arm rather than the normal `None` one.
/// A forcer sorting before `mid` (see the `app` control below) can never
/// reach this arm, which is exactly why four prior rounds of this bug family
/// were missed: every fixture happened to pick a name ordering that took the
/// innocent path.
#[test]
fn require_recipe_force_through_already_visited_recipe_renders_full_cycle() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let err = register_cookfile(rt, &mid_achore_cycle_fixture("zzz"), None)
        .expect_err("mid : achore plus achore's body forcing mid is a real cycle")
        .to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    assert!(
        !err.contains("achore -> achore"),
        "must not render a one-element self-cycle on `achore` — `achore` requires nothing; \
         `mid` is the missing link the buggy arm dropped off `self.path`; got: {err}"
    );
    assert_eq!(
        canonical_cycle_path(&err),
        vec!["achore".to_string(), "mid".to_string()],
        "the diagnostic must render the real 2-node cycle across BOTH recipes; got: {err}"
    );
}

/// Control for the test above, pinning §22.8's "the already-evaluated case
/// and the not-yet-evaluated case MUST be brought to the same result": the
/// IDENTICAL cycle (`mid : achore`, `achore`'s body forcing `mid`), but the
/// forcer is named `app` instead of `zzz`. `{achore, app, mid}` seeds
/// `[achore, app, mid]` — `app` sorts BEFORE `mid`, so `mid` is still `None`
/// when `app`'s body forces it, and the force takes the normal (`None`-state)
/// body path rather than the `Visited { forced: false }` arm the test above
/// exercises. That is the arm that already pushes `mid` onto `self.path`
/// correctly, so it is the reference behaviour the buggy arm must match.
///
/// The two renderings are not required to be the same STRING — `mid` sits at
/// a different point on the invocation stack in each case, so a correct
/// implementation can legitimately catch the cycle at either node and start
/// the printed path there — but they MUST be the same cycle once rotation is
/// normalized away. That equality is what actually pins the spec's "brought
/// to the same result": an author who renames only the forcer must not see
/// the diagnostic change out from under a Cookfile that did not change.
#[test]
fn require_recipe_force_before_visit_renders_full_cycle_matches_already_visited_case() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let err = register_cookfile(rt, &mid_achore_cycle_fixture("app"), None)
        .expect_err("mid : achore plus achore's body forcing mid is a real cycle")
        .to_string();
    assert!(err.contains("cook.require_recipe"), "error must name the API; got: {err}");
    let canonical = canonical_cycle_path(&err);
    assert_eq!(
        canonical,
        vec!["achore".to_string(), "mid".to_string()],
        "the diagnostic must render the real 2-node cycle across BOTH recipes; got: {err}"
    );

    // Same logical cycle, forcer sorted the other way — must normalize to
    // the identical cycle as the `zzz` case above (Standard §22.8).
    let dir2 = TempDir::new().unwrap();
    let rt2 = make_registry(dir2.path());
    let zzz_err = register_cookfile(rt2, &mid_achore_cycle_fixture("zzz"), None)
        .expect_err("mid : achore plus achore's body forcing mid is a real cycle")
        .to_string();
    assert_eq!(
        canonical,
        canonical_cycle_path(&zzz_err),
        "the already-evaluated (`zzz`) and not-yet-evaluated (`app`) cases must be brought \
         to the same result (Standard §22.8); app err: {err}, zzz err: {zzz_err}"
    );
}

// -----------------------------------------------------------------------
// CS-0149 (Standard §22.9): `cook.on_register_complete` — a finalizer queue
// drained once, after every recipe body of the pass has run and the pass's
// whole-graph validation has completed. Typed-argument-error coverage lives
// in `on_register_api.rs`'s own `#[cfg(test)]`; these drive the real
// `register_cookfile` pass end to end.
// -----------------------------------------------------------------------

/// Two callbacks queued in the top-level chunk run after both recipe
/// bodies, and in the order they were queued. The assertions live INSIDE
/// Lua (the VM is gone by the time `register_cookfile` returns), so a
/// violation raises and fails the pass — the test only needs to check
/// `is_ok()`.
#[test]
fn on_register_complete_runs_after_both_bodies_in_registration_order() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
_G.__order = {}
cook.recipe("a", {}, function()
    _G.__a_ran = true
    cook.exec("echo a", 1)
end)
cook.recipe("b", {}, function()
    _G.__b_ran = true
    cook.exec("echo b", 1)
end)
cook.on_register_complete(function()
    if not (_G.__a_ran and _G.__b_ran) then
        error("callback 1 ran before both recipe bodies had been evaluated")
    end
    table.insert(_G.__order, 1)
end)
cook.on_register_complete(function()
    if not (_G.__a_ran and _G.__b_ran) then
        error("callback 2 ran before both recipe bodies had been evaluated")
    end
    table.insert(_G.__order, 2)
    if #_G.__order ~= 2 or _G.__order[1] ~= 1 or _G.__order[2] ~= 2 then
        error("callbacks did not run in registration order: " .. table.concat(_G.__order, ","))
    end
end)
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
}

/// Each callback fires exactly once across the pass: a counter incremented
/// by the first callback must read exactly 1 from a later one.
#[test]
fn on_register_complete_callback_fires_exactly_once() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("a", {}, function()
    cook.exec("echo a", 1)
end)
_G.__count = 0
cook.on_register_complete(function()
    _G.__count = _G.__count + 1
end)
cook.on_register_complete(function()
    if _G.__count ~= 1 then
        error("expected the first callback to have fired exactly once, count=" .. tostring(_G.__count))
    end
end)
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
}

/// A callback that itself calls `cook.on_register_complete` re-queues: the
/// newly queued callback must still run this pass (drain-until-empty), not
/// be dropped when the outer loop's snapshot of the queue's original length
/// is exhausted. Verified via `fs.write`, observable after the VM is gone.
#[test]
fn on_register_complete_requeued_callback_runs_this_pass() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("a", {}, function()
    cook.exec("echo a", 1)
end)
cook.on_register_complete(function()
    cook.on_register_complete(function()
        fs.write("requeued.txt", "ran")
    end)
end)
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
    let content = fs::read_to_string(dir.path().join("requeued.txt"))
        .expect("requeued callback should have run and written the file");
    assert_eq!(content, "ran");
}

/// A callback calling `cook.recipe(...)` is a hard error (Standard §22.9):
/// the body-invocation pass has already closed the recipe set, so a
/// recipe minted here would never have its body evaluated this pass. Must
/// not be a silent no-op — the pass fails and the error names the API.
#[test]
fn on_register_complete_callback_recipe_registration_rejected() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("a", {}, function()
    cook.exec("echo a", 1)
end)
cook.on_register_complete(function()
    cook.recipe("late", {}, function() end)
end)
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "expected Err, got Ok");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("cook.on_register_complete"), "got: {err}");
    assert!(err.contains("recipe"), "got: {err}");
}

/// Same ban, for `cook.probe(...)`.
#[test]
fn on_register_complete_callback_probe_registration_rejected() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("a", {}, function()
    cook.exec("echo a", 1)
end)
cook.on_register_complete(function()
    cook.probe("late_probe", { produce = "return 1" })
end)
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "expected Err, got Ok");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("cook.on_register_complete"), "got: {err}");
    assert!(err.contains("probe"), "got: {err}");
}

/// A callback runs outside the dynamic extent of any recipe body, so a
/// body-scoped API called from it raises that API's ordinary
/// outside-a-recipe-body error — no special-casing needed for this one.
#[test]
fn on_register_complete_callback_add_unit_outside_body_rejected() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("a", {}, function()
    cook.exec("echo a", 1)
end)
cook.on_register_complete(function()
    cook.add_unit({ command = "echo x", inputs = {}, output = "x" })
end)
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "expected Err, got Ok");
    let err = result.err().unwrap().to_string();
    assert!(
        err.contains("cook.add_unit called outside a recipe body"),
        "got: {err}"
    );
}

/// An error raised by a callback fails the registration pass with that
/// error, exactly as an error from a recipe body would.
#[test]
fn on_register_complete_callback_error_fails_pass() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("a", {}, function()
    cook.exec("echo a", 1)
end)
cook.on_register_complete(function()
    error("boom")
end)
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_err(), "expected Err, got Ok");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("boom"), "got: {err}");
}

/// Consumer-contract test (Note 22.9.1's motivating case): a recipe body
/// exports a value; a callback imports it, calls `cook.sh`, and `fs.write`s
/// a file derived from the imported value into the working dir.
#[test]
fn on_register_complete_consumer_contract_export_import_sh_fs_write() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("lib", {}, function()
    cook.export("lib", { path = "build/lib.a" })
    cook.exec("echo lib", 1)
end)
cook.on_register_complete(function()
    local info = cook.import("lib")
    cook.sh("true")
    fs.write("summary.txt", info.path)
end)
"#;
    let result = register_cookfile(rt, lua_src, None);
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
    let content = fs::read_to_string(dir.path().join("summary.txt")).unwrap();
    assert_eq!(content, "build/lib.a");
}

/// `list_names` — the cheap name-only discovery surface — accepts a
/// `cook.on_register_complete` call (it is register-phase Lua like any
/// other) but MUST NOT be required to drain the queue: it invokes no
/// recipe body, and a callback can't register a recipe (see above), so
/// nothing it reports depends on having run one.
#[test]
fn list_names_never_drains_on_register_complete_queue() {
    let dir = TempDir::new().unwrap();
    let rt = make_registry(dir.path());
    let lua_src = r#"
cook.recipe("a", {}, function()
    cook.exec("echo a", 1)
end)
cook.on_register_complete(function()
    error("must not run")
end)
"#;
    let result = list_names(rt, lua_src);
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
    assert_eq!(result.unwrap().len(), 1);
}
