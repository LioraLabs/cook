use super::*;
use std::cell::RefCell;
use std::rc::Rc;
use crate::BodyCaptureState;
use std::collections::BTreeMap;

/// Convenience accessor used throughout the unit_api test module: borrow
/// the body slot and panic if it's `None`. The slot is set to `Some(...)`
/// by `make_lua_with_unit_api` for the duration of every test.
fn body_ref(body_slot: &SharedBodySlot) -> std::cell::Ref<'_, BodyCaptureState> {
    std::cell::Ref::map(body_slot.borrow(), |slot| {
        slot.as_ref().expect("body slot populated for test")
    })
}

fn make_lua_with_unit_api(recipe_name: &str) -> (Lua, SharedBodySlot) {
    use std::sync::{Arc, Mutex};
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    let body_slot: SharedBodySlot =
        Rc::new(RefCell::new(Some(BodyCaptureState::new())));
    let terminal_outputs: SharedTerminalOutputs =
        Arc::new(Mutex::new(BTreeMap::new()));
    // Tests reference paths like "main.c" that don't exist; the
    // directory-rejection check skips non-existent paths, so any
    // working_dir is fine here.
    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    register_unit_api(
        &lua,
        body_slot.clone(),
        recipe_name,
        terminal_outputs,
        working_dir,
    )
    .unwrap();
    (lua, body_slot)
}

fn fake_cache_ctx() -> std::sync::Arc<cook_cache::cache_ctx::CacheContext> {
    let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();
        std::mem::forget(dir); // tests are short-lived; let the OS clean up
        std::sync::Arc::new(cook_cache::cache_ctx::CacheContext {
            denylist: std::sync::Arc::new(cook_cache::envkey::EnvDenylist::baseline()),
            backend: std::sync::Arc::new(cook_cache::backend::LocalBackend::new(dir_path.clone())),
            cloud_config: std::sync::Arc::new(cook_cache::cloud_config::CloudConfig::default()),
            project_root: dir_path,
            project_id: "test-project".to_string(),
        publish_enabled: true,
    })
}

#[test]
fn test_add_unit_basic() {
    let (lua, capture_state) = make_lua_with_unit_api("my_recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook.add_unit({
                command = "gcc -o main main.c",
                inputs = {"main.c"},
                output = "main",
            })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    let unit = &state.units[0];

    match &unit.payload {
        WorkPayload::Shell { cmd, line } => {
            assert_eq!(cmd, "gcc -o main main.c");
            assert_eq!(*line, 0);
        }
        _ => panic!("expected Shell payload"),
    }

    let meta = unit.cache_meta.as_ref().expect("expected cache_meta");
    assert_eq!(meta.recipe_name, "my_recipe");
    assert_eq!(meta.input_paths, vec!["main.c"]);
    assert_eq!(meta.output_paths, vec!["main".to_string()]);
    assert_eq!(meta.command_hash, hash_str("gcc -o main main.c"));

    assert!(matches!(unit.dep_kind, DepKind::Sequential));
}

#[test]
fn test_add_unit_rejects_function_valued_command() {
    // A function-valued command used to coerce to "" and silently
    // no-op. It must now be a loud register-phase error.
    let (lua, _capture_state) = make_lua_with_unit_api("my_recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    let err = lua.load(r#"
            cook.add_unit({
                command = function() return "echo hi" end,
                inputs = {},
                output = "out/x.txt",
            })
        "#).exec().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("`command` must be a string"), "got: {msg}");
    assert!(msg.contains("function"), "error must name the received type; got: {msg}");
}

#[test]
fn test_add_unit_rejects_numeric_command() {
    let (lua, _capture_state) = make_lua_with_unit_api("my_recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    let err = lua.load(r#"cook.add_unit({ command = 42, output = "out/x.txt" })"#)
        .exec().unwrap_err();
    assert!(err.to_string().contains("`command` must be a string"), "got: {err}");
}

#[test]
fn test_add_unit_no_cache() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook.add_unit({
                command = "echo hello",
                cache = false,
            })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    assert!(state.units[0].cache_meta.is_none());
}

#[test]
fn test_add_unit_interactive_flag() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook.add_unit({
                command = "build/bin/lua -e 'print(1)'",
                interactive = true,
                cache = false,
            })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    match &state.units[0].payload {
        WorkPayload::Interactive { cmd, .. } => {
            assert_eq!(cmd, "build/bin/lua -e 'print(1)'");
        }
        other => panic!("expected Interactive payload, got {other:?}"),
    }
}

#[test]
fn test_add_unit_sequential_by_default() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook.add_unit({ command = "step1" })
            cook.add_unit({ command = "step2" })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 2);
    assert!(matches!(state.units[0].dep_kind, DepKind::Sequential));
    assert!(matches!(state.units[1].dep_kind, DepKind::Sequential));
}

#[test]
fn test_step_group_makes_parallel() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook.step_group(function()
                cook.add_unit({ command = "unit_a" })
                cook.add_unit({ command = "unit_b" })
            end)
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 2);
    assert!(matches!(state.units[0].dep_kind, DepKind::StepGroup(0)));
    assert!(matches!(state.units[1].dep_kind, DepKind::StepGroup(0)));
    assert_eq!(state.step_groups.len(), 1);
    assert_eq!(state.step_groups[0], vec![0, 1]);
}

#[test]
fn test_step_group_sequential_after() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook.step_group(function()
                cook.add_unit({ command = "parallel_unit" })
            end)
            cook.add_unit({ command = "sequential_unit" })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 2);
    assert!(matches!(state.units[0].dep_kind, DepKind::StepGroup(0)));
    assert!(matches!(state.units[1].dep_kind, DepKind::Sequential));
}

#[test]
fn test_last_cook_step_outputs_tracked() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            -- First cook step (OneToOne, 2 outputs)
            cook.step_group(function()
                cook.add_unit({ command = "gcc -c a.c -o a.o", inputs = {"a.c"}, output = "a.o" })
                cook.add_unit({ command = "gcc -c b.c -o b.o", inputs = {"b.c"}, output = "b.o" })
            end)
            -- Second cook step (ManyToOne, 1 output)
            cook.step_group(function()
                cook.add_unit({ command = "ar rcs lib.a a.o b.o", inputs = {"a.o", "b.o"}, output = "lib.a" })
            end)
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    // Terminal outputs = from the LAST step group that produced outputs: ["lib.a"]
    assert_eq!(state.last_cook_step_outputs, vec!["lib.a"]);
}

#[test]
fn test_no_output_step_group_does_not_overwrite_terminal() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            -- Cook step produces output
            cook.step_group(function()
                cook.add_unit({ command = "gcc -o app main.c", inputs = {"main.c"}, output = "app" })
            end)
            -- No-output step group -- should NOT overwrite terminal
            cook.step_group(function()
                cook.add_unit({ command = "./app", cache = false })
            end)
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.last_cook_step_outputs, vec!["app"]);
}

#[test]
fn test_add_unit_outputs_plural() {
    let (lua, capture_state) = make_lua_with_unit_api("my_recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook.add_unit({
                command = "split a.c",
                inputs = {"a.c"},
                outputs = {"a.o", "a.d"},
            })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    let unit = &state.units[0];
    let meta = unit.cache_meta.as_ref().expect("expected cache_meta");
    assert_eq!(
        meta.output_paths,
        vec!["a.o".to_string(), "a.d".to_string()]
    );
    // cache_key should embed context+env when they are non-zero
    assert!(meta.cache_key.starts_with("a.o"), "cache_key starts with first output");
}

#[test]
fn test_add_unit_outputs_and_output_conflict_errors() {
    let (lua, _capture_state) = make_lua_with_unit_api("my_recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    let result = lua.load(r#"
            cook.add_unit({
                command = "split a.c",
                inputs = {"a.c"},
                output = "a.o",
                outputs = {"a.o", "a.d"},
            })
        "#).exec();
    assert!(
        result.is_err(),
        "expected error when both `output` and `outputs` are provided"
    );
}

#[test]
fn test_add_unit_lua_code_one_to_one() {
    let (lua, capture_state) = make_lua_with_unit_api("my_recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(
        r#"
            cook.add_unit({
                inputs = {"main.c"},
                output = "main.o",
                lua_code = "print('hi')",
                ingredient_groups = {{"a.c", "b.c"}},
            })
        "#,
    )
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    let unit = &state.units[0];
    match &unit.payload {
        WorkPayload::LuaChunk {
            code,
            inputs,
            outputs,
            ingredient_groups,
            step_kind: _,
            is_chore: _,
            line: _,
        } => {
            assert_eq!(code, "print('hi')");
            assert_eq!(inputs, &vec!["main.c".to_string()]);
            assert_eq!(outputs, &vec!["main.o".to_string()]);
            assert_eq!(
                ingredient_groups,
                &vec![vec!["a.c".to_string(), "b.c".to_string()]]
            );
        }
        other => panic!("expected LuaChunk, got {other:?}"),
    }
}

#[test]
fn test_add_unit_lua_code_multi_output_block_step() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(
        r#"
            cook.add_unit({
                inputs = {"src.rs"},
                outputs = {"a.js", "a.wasm"},
                lua_code = "os.execute('wasm-pack build')",
                ingredient_groups = {{"src.rs"}},
            })
        "#,
    )
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    match &state.units[0].payload {
        WorkPayload::LuaChunk {
            code,
            inputs,
            outputs,
            ingredient_groups,
            step_kind: _,
            is_chore: _,
            line: _,
        } => {
            assert_eq!(code, "os.execute('wasm-pack build')");
            assert_eq!(inputs, &vec!["src.rs".to_string()]);
            assert_eq!(
                outputs,
                &vec!["a.js".to_string(), "a.wasm".to_string()]
            );
            assert_eq!(ingredient_groups, &vec![vec!["src.rs".to_string()]]);
        }
        other => panic!("expected LuaChunk, got {other:?}"),
    }
}

#[test]
fn test_single_step_terminal_outputs() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook.step_group(function()
                cook.add_unit({ command = "gcc -o app main.c", inputs = {"main.c"}, output = "app" })
            end)
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.last_cook_step_outputs, vec!["app"]);
}

#[test]
fn add_unit_populates_consulted_env_from_keys_list() {
    // The lookup reads from cook.env (the Cook Lua VM env table), NOT the
    // process env — that's the merged config-overlay+process value the
    // command actually consumed. Populate the declared-variable store directly
    // here; in real usage capture.rs creates it and the config-block dispatch
    // fills it (CS-0172).
    let lua = Lua::new();
    let cook_table = lua.create_table().unwrap();
    let store = lua.create_table().unwrap();
    store.set("FOO_TEST_VAR_X", "the-value").unwrap();
    lua.set_named_registry_value(crate::VAR_STORE_REGISTRY_KEY, store)
        .unwrap();
    lua.globals().set("cook", cook_table).unwrap();

    let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
    let terminal_outputs: SharedTerminalOutputs =
        std::sync::Arc::new(std::sync::Mutex::new(BTreeMap::new()));
    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    register_unit_api(
        &lua,
        capture_state.clone(),
        "my_recipe",
        terminal_outputs,
        working_dir,
    )
    .unwrap();

    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                command = "make all",
                inputs = {"main.c"},
                output = "main",
                consulted_env_keys = {"FOO_TEST_VAR_X"},
            })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    let meta = state.units[0].cache_meta.as_ref().expect("cache_meta");
    assert_eq!(
        meta.consulted_env.get("FOO_TEST_VAR_X").map(|s| s.as_str()),
        Some("the-value"),
        "consulted_env must contain FOO_TEST_VAR_X=the-value (read from the var store)"
    );
    // env_contribution must be non-zero because a non-denylisted var was consulted
    assert_ne!(meta.env_contribution, 0, "env_contribution must be non-zero");
}

#[test]
fn add_unit_appends_resolved_dep_paths_to_input_paths() {
    // Spec §4.3: cross-recipe dep refs accumulated by cook.dep_output(name)
    // resolve to terminal output paths and land in cache_meta.input_paths
    // (only — never in WorkPayload.inputs).
    let lua = Lua::new();
    let cook_table = lua.create_table().unwrap();
    cook_table.set("env", lua.create_table().unwrap()).unwrap();
    lua.globals().set("cook", cook_table).unwrap();

    let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
    let terminal_outputs: SharedTerminalOutputs = std::sync::Arc::new(std::sync::Mutex::new(BTreeMap::new()));
    terminal_outputs
        .lock().unwrap()
        .insert("greet".into(), vec!["build/greet.o".into()]);
    terminal_outputs
        .lock().unwrap()
        .insert("util".into(), vec!["build/util.o".into()]);

    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    register_unit_api(
        &lua,
        capture_state.clone(),
        "demo",
        terminal_outputs.clone(),
        working_dir,
    )
    .unwrap();
    crate::dep_output_api::register_dep_output_api(
        &lua,
        terminal_outputs,
        capture_state.clone(),
        std::collections::BTreeMap::new(),
        String::new(),
        std::collections::BTreeMap::new(),
    )
    .unwrap();

    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string())
        .expect("set");

    // Codegen sequence: cook.dep_output() called inside command construction
    // accumulates dep refs; add_unit then picks them up.
    lua.load(
        r#"
            local _ = cook.dep_output("greet")
            local _ = cook.dep_output("util")
            cook.add_unit({
                command = "gcc build/greet.o build/util.o -o build/demo",
                inputs = {},
                output = "build/demo",
            })
        "#,
    )
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    let meta = state.units[0]
        .cache_meta
        .as_ref()
        .expect("cache_meta present");
    assert_eq!(
        meta.input_paths,
        vec!["build/greet.o".to_string(), "build/util.o".to_string()],
        "cross-recipe dep paths must land in cache_meta.input_paths"
    );

    // WorkPayload inputs MUST remain empty — those drive iteration vars.
    match &state.units[0].payload {
        WorkPayload::Shell { cmd, .. } => {
            assert!(cmd.contains("gcc"));
        }
        other => panic!("expected Shell, got {other:?}"),
    }
}

#[test]
fn add_unit_inside_chore_marks_payload_is_chore_true() {
    let (lua, capture_state) = make_lua_with_unit_api("my_chore");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook._enter_chore()
            cook.add_unit({
                command = "fzf --prompt='> '",
                interactive = true,
                cache = false,
            })
            cook._exit_chore()
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    match &state.units[0].payload {
        WorkPayload::Interactive { is_chore, .. } => {
            assert!(*is_chore, "unit emitted inside chore body must have is_chore=true");
        }
        other => panic!("expected Interactive payload, got {other:?}"),
    }
}

#[test]
fn add_unit_inside_chore_marks_lua_chunk_is_chore_true() {
    let (lua, capture_state) = make_lua_with_unit_api("my_chore");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook._enter_chore()
            cook.add_unit({
                lua_code = "print('hello from chore')",
                interactive = true,
                cache = false,
            })
            cook._exit_chore()
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    match &state.units[0].payload {
        WorkPayload::LuaChunk { is_chore, .. } => {
            assert!(*is_chore, "lua chunk emitted inside chore body must have is_chore=true");
        }
        other => panic!("expected LuaChunk payload, got {other:?}"),
    }
}

#[test]
fn add_unit_outside_chore_marks_payload_is_chore_false() {
    let (lua, capture_state) = make_lua_with_unit_api("my_recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
    lua.load(r#"
            cook.add_unit({
                command = "build/bin/lua -e 'print(1)'",
                interactive = true,
                cache = false,
            })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    match &state.units[0].payload {
        WorkPayload::Interactive { is_chore, .. } => {
            assert!(!*is_chore, "unit emitted outside chore must have is_chore=false");
        }
        other => panic!("expected Interactive payload, got {other:?}"),
    }
}

#[test]
fn add_unit_reads_discovered_inputs_table() {
    let (lua, capture_state) = make_lua_with_unit_api("demo");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                inputs = { "src/a.c" },
                output = "build/a.o",
                command = "gcc -c src/a.c -o build/a.o",
                discovered_inputs = { from = ".cook/deps/a.d", format = "make" },
            })
        "#).exec().expect("exec");

    let st = body_ref(&capture_state);
    let unit: &CapturedUnit = st.units.last().expect("one unit");
    let cm = unit.cache_meta.as_ref().expect("cache_meta");
    let di = cm.discovered_inputs.as_ref().expect("discovered_inputs");
    assert_eq!(di.from, ".cook/deps/a.d");
    assert_eq!(di.format, "make");
}

#[test]
fn add_unit_rejects_unsupported_discovered_inputs_format() {
    let (lua, _capture_state) = make_lua_with_unit_api("demo");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    let result = lua.load(r#"
            cook.add_unit({
                inputs = { "x" }, output = "y", command = "true",
                discovered_inputs = { from = "x.d", format = "ninja" },
            })
        "#).exec();

    let err = result.expect_err("expected error for unsupported format").to_string();
    assert!(err.contains("ninja"), "diagnostic must name the unsupported format; got: {err}");
    assert!(err.contains("supported"), "diagnostic must say what is supported; got: {err}");
}

#[test]
fn add_unit_rejects_absolute_discovered_from() {
    let (lua, _capture_state) = make_lua_with_unit_api("demo");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    let result = lua.load(r#"
            cook.add_unit({
                inputs = { "x" }, output = "y", command = "true",
                discovered_inputs = { from = "/etc/secrets.d", format = "make" },
            })
        "#).exec();

    let err = result.expect_err("expected error for absolute path").to_string();
    assert!(
        err.contains("relative") || err.contains("absolute"),
        "diagnostic must mention 'relative' or 'absolute'; got: {err}"
    );
}

#[test]
fn add_unit_rejects_dotdot_discovered_from() {
    let (lua, _capture_state) = make_lua_with_unit_api("demo");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    let result = lua.load(r#"
            cook.add_unit({
                inputs = { "x" }, output = "y", command = "true",
                discovered_inputs = { from = "../escape.d", format = "make" },
            })
        "#).exec();

    let err = result.expect_err("expected error for '..' path").to_string();
    assert!(err.contains(".."), "diagnostic must contain '..'; got: {err}");
}

/// Regression: `cook.add_unit` MUST reject directory inputs at register
/// time. The cache hashing layer reads files; passing a directory used
/// to silently produce an empty cache record (only `_source_hash`),
/// causing the unit to re-run on every invocation. We now fail fast
/// with a clear, actionable diagnostic instead.
#[test]
fn add_unit_rejects_directory_input() {
    use std::sync::{Arc, Mutex};

    let tmp = tempfile::tempdir().expect("tempdir");
        // Build a real directory the recipe will (mistakenly) declare as
        // an input.
        let upstream = tmp.path().join("upstream").join("lib");
    std::fs::create_dir_all(&upstream).expect("mkdir upstream/lib");
    std::fs::write(upstream.join("a.txt"), b"a").expect("write a.txt");

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
    let terminal_outputs: SharedTerminalOutputs =
        Arc::new(Mutex::new(BTreeMap::new()));
    register_unit_api(
        &lua,
        capture_state.clone(),
        "vendor",
            terminal_outputs,
            tmp.path().to_path_buf(),
        )
        .unwrap();

        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value(
            "__cook_cookfile_path",
        "Cookfile".to_string(),
    )
    .expect("set");

    let result = lua
        .load(
            r#"
                cook.add_unit({
                    command = "cp -a upstream/lib build/lib",
                    inputs = { "upstream/lib" },
                    output = "build/lib.stamp",
                })
            "#,
        )
        .exec();

    let err = result
        .expect_err("expected error for directory input")
        .to_string();
    assert!(
        err.contains("is a directory"),
        "diagnostic must contain 'is a directory'; got: {err}"
    );
    assert!(
        err.contains("upstream/lib"),
        "diagnostic must name the offending path; got: {err}"
    );
    assert!(
        err.contains("glob") || err.contains("specific files"),
        "diagnostic must suggest a fix (glob or list specific files); got: {err}"
    );
    // No unit must have been recorded.
    assert!(
        body_ref(&capture_state).units.is_empty(),
        "rejected add_unit must not record a unit"
    );
}

/// Files (existing or not) MUST still pass through. Verifies the
/// directory-rejection check doesn't accidentally reject valid file
/// inputs (the common case).
#[test]
fn add_unit_accepts_file_inputs() {
    use std::sync::{Arc, Mutex};

    let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("upstream").join("lib");
    std::fs::create_dir_all(&src).expect("mkdir upstream/lib");
    std::fs::write(src.join("a.txt"), b"a").expect("write a.txt");
    std::fs::write(src.join("b.txt"), b"b").expect("write b.txt");

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
    let terminal_outputs: SharedTerminalOutputs =
        Arc::new(Mutex::new(BTreeMap::new()));
    register_unit_api(
        &lua,
        capture_state.clone(),
        "vendor",
            terminal_outputs,
            tmp.path().to_path_buf(),
        )
        .unwrap();

        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value(
            "__cook_cookfile_path",
        "Cookfile".to_string(),
    )
    .expect("set");

    // Real file (exists) and a not-yet-built output (does not exist).
    lua.load(
        r#"
            cook.add_unit({
                command = "cp upstream/lib/a.txt build/a.txt",
                inputs = { "upstream/lib/a.txt" },
                output = "build/a.txt",
            })
        "#,
    )
    .exec()
    .expect("file input must be accepted");

    assert_eq!(body_ref(&capture_state).units.len(), 1);
}

/// `outputs` (plural) is also covered: declaring a directory as a
/// declared output is rejected.
#[test]
fn add_unit_rejects_directory_in_outputs_plural() {
    use std::sync::{Arc, Mutex};

    let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("build").join("artifacts");
    std::fs::create_dir_all(&dir).expect("mkdir build/artifacts");

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
    let terminal_outputs: SharedTerminalOutputs =
        Arc::new(Mutex::new(BTreeMap::new()));
    register_unit_api(
        &lua,
        capture_state.clone(),
        "build",
        terminal_outputs,
        tmp.path().to_path_buf(),
    )
    .unwrap();

    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value(
        "__cook_cookfile_path",
        "Cookfile".to_string(),
    )
    .expect("set");

    let result = lua
        .load(
            r#"
                cook.add_unit({
                    command = "mkdir -p build/artifacts && touch build/a.o build/b.o",
                    inputs = {},
                    outputs = { "build/a.o", "build/artifacts" },
                })
            "#,
        )
        .exec();

    let err = result
        .expect_err("expected error for directory output")
        .to_string();
    assert!(
        err.contains("is a directory"),
        "diagnostic must contain 'is a directory'; got: {err}"
    );
    assert!(
        err.contains("build/artifacts"),
        "diagnostic must name the offending path; got: {err}"
    );
}

#[test]
fn add_unit_captures_probes_field() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                name = "myapp.o",
                inputs = { "myapp.c" },
                outputs = { "build/myapp.o" },
                probes = { "cc:zlib", "cc:compiler" },
                command = "true",
            })
        "#)
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    let u = state.units.first().expect("one unit");
    assert_eq!(u.probes, vec!["cc:zlib", "cc:compiler"]);
    }

    /// CS-0152: a literal `cook.probes.get("key")` read inside a `lua_code`
/// unit body must be statically scanned and unioned into `probes` at
/// capture time, so the probe is demand-scheduled ahead of the unit
/// instead of reading nil at execute time.
#[test]
fn add_unit_lua_code_probe_get_call_captures_probes_field() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                name = "u",
                outputs = { "out.txt" },
                lua_code = "local v = cook.probes.get(\"cc:zlib\")",
            })
        "#)
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    let u = state.units.first().expect("one unit");
    assert_eq!(u.probes, vec!["cc:zlib"]);
}

/// An explicit `probes` list and scanned literal `cook.probes.get` keys
/// must union without duplicating an entry present in both.
#[test]
fn add_unit_lua_code_probe_get_unions_with_explicit_probes_without_dup() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                name = "u",
                outputs = { "out.txt" },
                probes = { "cc:zlib" },
                lua_code = "local a = cook.probes.get(\"cc:zlib\")\nlocal b = cook.probes.get(\"cc:compiler\")",
            })
        "#)
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    let u = state.units.first().expect("one unit");
    assert_eq!(u.probes, vec!["cc:zlib", "cc:compiler"]);
    }

    /// A shell-command unit (no `lua_code`) must be unaffected by the new
    /// scan — regression guard against the union firing on the wrong path.
    #[test]
    fn add_unit_shell_command_unit_unaffected_by_probe_scan() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                name = "u",
                outputs = { "out.txt" },
                command = "echo 'cook.probes.get(\"cc:zlib\")'",
            })
        "#)
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    let u = state.units.first().expect("one unit");
    assert!(u.probes.is_empty());
}

#[test]
fn add_unit_seal_field_sets_cache_meta_and_probes() {
    // COOK-161: opts.seal carries the effective seal set onto CacheMeta and
    // unions into the unit's probe-dependency vec so the unit runs after the
    // sealed probes are materialised.
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                name = "x.o",
                outputs = { "x.o" },
                command = "cc",
                seal = { "host" },
            })
        "#)
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    let u = state.units.first().expect("one unit");
    assert!(
        u.probes.contains(&"host".to_string()),
        "sealed key must be unioned into probes; got: {:?}",
        u.probes
    );
    let cm = u.cache_meta.as_ref().expect("cache_meta present");
    assert!(
        cm.seal_keys.contains("host"),
        "sealed key must be on CacheMeta.seal_keys; got: {:?}",
        cm.seal_keys
    );
}

#[test]
fn add_unit_local_pinned_disposition_booleans() {
    // COOK-162 / I3: opts.sharing ("local"/"pinned") is parsed into the
    // CacheMeta.sharing enum. Three sub-cases: local, pinned, neither.

    // Case 1: sharing = "local" → CacheMeta.sharing == Local
    {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
                cook.add_unit({
                    command = "echo local",
                    output = "out.txt",
                    sharing = "local",
                })
            "#)
        .exec()
        .unwrap();
        let state = body_ref(&capture_state);
        let cm = state.units[0].cache_meta.as_ref().expect("cache_meta present");
        assert_eq!(
            cm.sharing,
            cook_contracts::Sharing::Local,
            "sharing=\"local\" should propagate to CacheMeta.sharing"
        );
    }

    // Case 2: sharing = "pinned" → CacheMeta.sharing == Pinned
    {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
                cook.add_unit({
                    command = "echo pinned",
                    output = "out.txt",
                    sharing = "pinned",
                })
            "#)
        .exec()
        .unwrap();
        let state = body_ref(&capture_state);
        let cm = state.units[0].cache_meta.as_ref().expect("cache_meta present");
        assert_eq!(
            cm.sharing,
            cook_contracts::Sharing::Pinned,
            "sharing=\"pinned\" should propagate to CacheMeta.sharing"
        );
    }

    // Case 3: neither → Shared default
    {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
                cook.add_unit({
                    command = "echo neither",
                    output = "out.txt",
                })
            "#)
        .exec()
        .unwrap();
        let state = body_ref(&capture_state);
        let cm = state.units[0].cache_meta.as_ref().expect("cache_meta present");
        assert_eq!(
            cm.sharing,
            cook_contracts::Sharing::Shared,
            "omitting sharing should leave CacheMeta.sharing Shared"
        );
    }
}

#[test]
fn add_unit_without_probes_defaults_to_empty() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                command = "echo hello",
                cache = false,
            })
        "#)
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    let u = state.units.first().expect("one unit");
    assert!(u.probes.is_empty());
}

#[test]
fn add_unit_probes_non_list_errors() {
    let (lua, _capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    let result = lua.load(r#"
            cook.add_unit({
                command = "echo hello",
                cache = false,
                probes = "not-a-list",
            })
        "#).exec();

    assert!(result.is_err(), "probes must be a list, not a string");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("probes"), "error must mention 'probes'; got: {err}");
}

#[test]
fn add_unit_legacy_requires_field_is_rejected() {
    let (lua, _capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    let result = lua.load(r#"
            cook.add_unit({
                name = "u",
                inputs = {}, outputs = {"out.txt"},
                cache = false,
                requires = { "cc:zlib" },
                command = "true",
            })
        "#).exec();

    assert!(result.is_err(), "legacy `requires` field must be rejected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("rename to `probes`"),
        "diagnostic must direct user to `probes`; got: {err}"
    );
}

#[test]
fn add_unit_legacy_requires_field_as_string_is_rejected() {
    // A mid-migration Cookfile might write `requires = "cc:zlib"` (string)
    // rather than a table. The guard MUST still fire so the author learns
    // the field is gone — silently accepting non-table values would leave
    // partial migrations undetected.
    let (lua, _capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    let result = lua.load(r#"
            cook.add_unit({
                name = "u",
                inputs = {}, outputs = {"out.txt"},
                cache = false,
                requires = "cc:zlib",
                command = "true",
            })
        "#).exec();

    assert!(result.is_err(), "legacy `requires` field must be rejected even when non-table");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("rename to `probes`"),
        "diagnostic must direct user to `probes`; got: {err}"
    );
}

/// CS-0074: cook.add_unit with a command containing `$<key:field>` probe-value
/// sigils MUST be rewritten into a LuaChunk that resolves the probe value at
/// execute time via cook.probes.get.
#[test]
fn add_unit_command_with_probe_template_is_rewritten() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                name = "u",
                inputs = {}, outputs = {"out.txt"},
                cache = false,
                command = "echo $<demo:k.v> > out.txt",
            })
        "#)
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    let unit = state.units.first().expect("one unit");

    let has_cache_get = match &unit.payload {
        WorkPayload::LuaChunk { code, .. } => code.contains("cook.probes.get"),
        WorkPayload::Shell { cmd, .. } => cmd.contains("cook.probes.get"),
        _ => false,
    };
    assert!(
        has_cache_get,
        "expected template to be expanded; got payload: {:?}",
        unit.payload
    );

    // The probe key (everything before the first `.` after the `:`) must be
    // auto-added to probes.
    assert!(
        unit.probes.contains(&"demo:k".to_string()),
        "detected probe key must be auto-added to probes; got: {:?}",
        unit.probes
    );
}

/// CS-0101: `$<file:PATH>` in a raw cook.add_unit command string is the
/// reserved file-reference namespace, NOT a probe key. v1 does not
/// support file refs in raw register-block command strings — the
/// template expander must reject them loudly instead of misparsing
/// `file` as a probe key.
#[test]
fn add_unit_command_with_file_ref_sigil_is_rejected() {
    let (lua, _capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    let err = lua.load(r#"
            cook.add_unit({
                inputs = {}, outputs = {"out.txt"},
                cache = false,
                command = "render --tokens $<file:tokens.css> > out.txt",
            })
        "#)
    .exec()
    .unwrap_err()
    .to_string();

    assert!(
        err.contains("not supported in raw cook.add_unit command strings"),
        "expected the raw-command file-ref rejection; got: {err}"
    );
}

/// COOK-96 Task 5: add_unit must record `member` and `output_paths` on the
/// resulting `CapturedUnit` so the engine can build the per-member output map
/// needed by `$<recipe[in]>` (COOK-221/CS-0137).
#[test]
fn add_unit_retains_member_and_outputs() {
    let (lua, capture_state) = make_lua_with_unit_api("encode");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                output = "build/s1.mp4",
                command = "echo hi",
                member = "{\"id\":\"s1\"}",
            })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    let u = state.units.last().expect("a unit was captured");
    assert_eq!(u.member.as_deref(), Some("{\"id\":\"s1\"}"));
    assert_eq!(u.output_paths, vec!["build/s1.mp4".to_string()]);
}

#[test]
fn add_unit_record_flag_threads_to_cache_meta() {
    // COOK-163: opts.record marks an intrinsically non-reproducible artifact.
    // The register layer must read opts.record and set it on CacheMeta.
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                name = "x.o",
                outputs = { "x.o" },
                command = "cc",
                record = true,
            })
        "#)
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    let u = state.units.first().expect("one unit");
    let cm = u.cache_meta.as_ref().expect("cache_meta present");
    assert!(
        cm.record,
        "record must be true on CacheMeta when opts.record = true; got: {}",
        cm.record
    );
}

#[test]
fn add_unit_record_defaults_false() {
    // COOK-163: when opts.record is absent, it defaults to false.
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

    lua.load(r#"
            cook.add_unit({
                name = "x.o",
                outputs = { "x.o" },
                command = "cc",
            })
        "#)
    .exec()
    .unwrap();

    let state = body_ref(&capture_state);
    let u = state.units.first().expect("one unit");
    let cm = u.cache_meta.as_ref().expect("cache_meta present");
    assert!(
        !cm.record,
        "record must be false on CacheMeta when opts.record is absent; got: {}",
        cm.record
    );
}

/// CS-0119: a trailing-slash output `"pkg/"` declares a directory output.
/// On the SECOND build `pkg/` already exists as a directory on disk; the
/// register-time directory-rejection check MUST NOT fire for trailing-slash
/// entries — they are intentionally directories, not a mistake.
#[test]
fn add_unit_accepts_directory_output_trailing_slash() {
    use std::sync::{Arc, Mutex};

    let tmp = tempfile::tempdir().expect("tempdir");
        // Simulate the "second build": pkg/ already exists on disk.
    let pkg_dir = tmp.path().join("pkg");
    std::fs::create_dir_all(&pkg_dir).expect("mkdir pkg");

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
    let terminal_outputs: SharedTerminalOutputs =
        Arc::new(Mutex::new(BTreeMap::new()));
    register_unit_api(
        &lua,
        capture_state.clone(),
        "build",
        terminal_outputs,
        tmp.path().to_path_buf(),
    )
    .unwrap();

    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value(
        "__cook_cookfile_path",
        "Cookfile".to_string(),
    )
    .expect("set");

    // pkg/ already exists as a directory; the trailing slash signals
    // CS-0119 directory-output semantics — must be accepted, not rejected.
    let result = lua
        .load(
            r#"
                cook.add_unit({
                    command = "npm pack --pack-destination pkg/",
                    inputs = { "package.json" },
                    outputs = { "pkg/" },
                })
            "#,
        )
        .exec();

    assert!(
        result.is_ok(),
        "directory output with trailing slash must be accepted even when the \
         directory already exists on disk (CS-0119); got: {:?}",
        result.err()
    );
    assert_eq!(
        body_ref(&capture_state).units.len(),
        1,
        "unit must be recorded"
    );
}

// -------------------------------------------------------------------------
// CS-0185: one registration function
// -------------------------------------------------------------------------

/// CS-0185: `cook.add_test` is removed, and calling it raises rather than
/// resolving to nil — the diagnostic has to name the replacement, which
/// `attempt to call a nil value` cannot.
///
/// This replaces the equivalence test that stood here through commits 1 and 2,
/// which asserted that both functions recorded the same unit. That was the
/// claim that made switching codegen safe; it cannot be restated now that
/// there is only one function, and the switch it guarded has landed.
#[test]
fn add_test_is_removed_and_names_its_replacement() {
    let (lua, slot) = make_lua_with_unit_api("checks");
    crate::test_api::register_test_api(&lua, slot.clone()).unwrap();

    let err = lua
        .load(r#"cook.add_test({ command = "./run" })"#)
        .exec()
        .expect_err("cook.add_test must raise");
    let msg = err.to_string();
    assert!(msg.contains("was removed"), "got: {msg}");
    assert!(msg.contains("step_kind = \"test\""), "must name the replacement: {msg}");
    assert!(msg.contains("CS-0185"), "must cite the entry: {msg}");
    // Nothing was recorded on the way out.
    assert_eq!(body_ref(&slot).units.len(), 0);
}

/// §22.4: outputs and test-ness are independent facts, so an output on a test
/// unit is an author error rather than a silent reclassification.
#[test]
fn a_test_unit_may_not_declare_outputs() {
    let (lua, _slot) = make_lua_with_unit_api("checks");
    let err = lua
        .load(r#"cook.add_unit({ step_kind = "test", command = "./run", output = "out.o" })"#)
        .exec()
        .expect_err("outputs on a test unit must be refused");
    assert!(err.to_string().contains("declares no outputs"), "got: {err}");
}

/// `suite` is removed, and passing it is an error rather than a silent no-op:
/// a caller who writes it means something by it.
#[test]
fn a_test_unit_may_not_declare_a_suite() {
    let (lua, _slot) = make_lua_with_unit_api("checks");
    let err = lua
        .load(r#"cook.add_unit({ step_kind = "test", command = "./run", suite = "x" })"#)
        .exec()
        .expect_err("suite must be refused");
    assert!(err.to_string().contains("`suite` was removed"), "got: {err}");
}

/// COOK-360: a test unit INSIDE a step group. The equivalence work compared
/// units recorded outside one, where every dep kind collapses to `Sequential`,
/// so this is the case none of it exercised — and it is the case that matters,
/// because it is where `DepKind::TestSibling` used to differ and where CS-0030's
/// dep-edge wiring is load-bearing.
///
/// `TestSibling` is gone. It claimed to be "like StepGroup but failures don't
/// cancel siblings", and enforced nothing: group members all depend on the same
/// barrier and never on each other, so a sibling is never a dependent and the
/// cancellation walk cannot reach one. CS-0177's sibling exemption is a
/// property of the graph's shape.
#[test]
fn a_test_unit_in_a_step_group_is_grouped_like_any_other_unit() {
    let (lua, slot) = make_lua_with_unit_api("checks");
    lua.load(
        r#"
        cook.step_group(function()
            cook.add_unit({ step_kind = "test", command = "./a", line = 3 })
            cook.add_unit({ step_kind = "test", command = "./b", line = 4 })
        end)
        "#,
    )
    .exec()
    .unwrap();

    let state = body_ref(&slot);
    assert_eq!(state.units.len(), 2);
    for u in &state.units {
        assert!(
            matches!(u.dep_kind, DepKind::StepGroup(_)),
            "a grouped test unit is grouped like any other unit"
        );
    }
    // Both members belong to the same group, which is what makes them siblings
    // rather than a chain.
    match (&state.units[0].dep_kind, &state.units[1].dep_kind) {
        (DepKind::StepGroup(a), DepKind::StepGroup(b)) => assert_eq!(a, b),
        _ => panic!("expected both units in one step group"),
    }
}

// CS-0030's dep-edge wiring for a grouped test unit is already covered by
// `test_api::tests::test_add_test_propagates_step_group_dep_refs_to_dep_edges`,
// which was written against the removed function and now drives this one. It
// uses the register-phase `cook.dep_output` surface, which this harness does
// not bind; duplicating it here with a hand-set body field would test the
// harness rather than the wiring.

// ---------------------------------------------------------------------------
// Local cache key: the identity a unit is filed under (§17.1.1.1, CS-0186)
// ---------------------------------------------------------------------------

/// `build_local_cache_key` is private, and these call it directly rather than
/// through `cook.add_unit`, because the properties under test are properties of
/// the key composition: driving them through the Lua surface would let a
/// declaration detail decide whether the assertion held.
fn key(outputs: &[&str], inputs: &[&str], command_hash: u64, env: u64) -> String {
    let outputs: Vec<String> = outputs.iter().map(|s| s.to_string()).collect();
    let inputs: Vec<String> = inputs.iter().map(|s| s.to_string()).collect();
    build_local_cache_key("Cookfile", "r", &outputs, &inputs, command_hash, env)
}

#[test]
fn a_producing_unit_is_identified_by_its_first_output() {
    assert_eq!(key(&["a.o", "a.d"], &["a.c"], 0xbeef, 0), "a.o");
    assert_eq!(key(&["a.o"], &["a.c"], 0xbeef, 0x1234), "a.o@1234");
}

/// The producing branch reads NOTHING but the output list and the env
/// contribution. Stated as a test because CS-0186 restructured this function
/// around a shared `<identity>@<env>` tail, and a cook unit's key moving would
/// invalidate every artifact in every index in the project.
#[test]
fn a_producing_units_key_ignores_inputs_and_command() {
    let a = key(&["a.o"], &["a.c"], 0xbeef, 0);
    let b = key(&["a.o"], &["totally", "different"], 0xfeed, 0);
    assert_eq!(a, b, "a producing key is its output path and nothing else");
}

#[test]
fn an_observing_unit_is_identified_by_a_declaration_digest() {
    let k = key(&[], &["a.c"], 0xbeef, 0);
    assert!(k.starts_with(OBSERVING_KEY_MARKER), "observing keys carry the marker: {k}");
    assert_eq!(k.len(), 17, "marker plus 16 hex digits: {k}");
    assert!(k[1..].chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn an_observing_unit_carries_the_env_contribution_in_the_same_position() {
    let bare = key(&[], &["a.c"], 0xbeef, 0);
    let with_env = key(&[], &["a.c"], 0xbeef, 0x1234);
    assert_eq!(with_env, format!("{bare}@1234"));
}

/// The marker is what keeps the two identity spaces apart in one index. A
/// declared output path is workspace-relative and never opens with it.
#[test]
fn the_two_identity_spaces_do_not_overlap() {
    let producing = key(&["build/demo"], &["a.c"], 0xbeef, 0);
    let observing = key(&[], &["a.c"], 0xbeef, 0);
    assert!(!producing.starts_with(OBSERVING_KEY_MARKER));
    assert!(observing.starts_with(OBSERVING_KEY_MARKER));
    assert_ne!(producing, observing);
}

// --- what the old `<first-input>@<command-hash>` form could not tell apart ---

/// The defect that made the old form unsafe to rely on: two units sharing a
/// first input and a command text were ONE entry, because nothing past the
/// first input reached the key.
#[test]
fn units_differing_only_past_the_first_input_are_distinct() {
    let a = key(&[], &["shared.c", "one.c"], 0xbeef, 0);
    let b = key(&[], &["shared.c", "two.c"], 0xbeef, 0);
    assert_ne!(a, b, "every declared input reaches the identity, not just the first");
}

#[test]
fn units_differing_only_in_input_count_are_distinct() {
    assert_ne!(key(&[], &["a.c"], 0xbeef, 0), key(&[], &["a.c", "b.c"], 0xbeef, 0));
}

/// A unit declaring no inputs at all keyed as `@<command-hash>` under the old
/// form — the empty string plus a hash — so two such units in one recipe
/// collided unless their command text differed.
#[test]
fn units_declaring_no_inputs_are_still_told_apart_by_command() {
    let a = key(&[], &[], 0xbeef, 0);
    let b = key(&[], &[], 0xfeed, 0);
    assert_ne!(a, b);
    assert!(a.starts_with(OBSERVING_KEY_MARKER) && b.starts_with(OBSERVING_KEY_MARKER));
}

#[test]
fn units_differing_only_in_command_are_distinct() {
    assert_ne!(key(&[], &["a.c"], 0xbeef, 0), key(&[], &["a.c"], 0xfeed, 0));
}

/// Path boundaries are unambiguous: the concatenation of one input list must
/// not hash like the concatenation of a different one.
#[test]
fn input_paths_cannot_run_together() {
    assert_ne!(key(&[], &["ab", "c"], 0xbeef, 0), key(&[], &["a", "bc"], 0xbeef, 0));
}

// --- what the identity deliberately does NOT depend on ---

/// The exclusion the whole design rests on. If contents reached the identity,
/// no record would ever be found twice and every invalidation would present as
/// a first-ever build with no reportable cause. The key is a function of the
/// declaration alone, and these arguments carry no content.
#[test]
fn the_identity_is_stable_across_everything_but_the_declaration() {
    let first = key(&[], &["a.c", "b.c"], 0xbeef, 0x1234);
    let again = key(&[], &["a.c", "b.c"], 0xbeef, 0x1234);
    assert_eq!(first, again, "the identity is a pure function of the declaration");
}

/// §17.4: moving a test within a recipe, or a recipe between Cookfiles, MUST
/// NOT bust its cache. Both are arguments to this function and both are unused.
#[test]
fn neither_the_recipe_nor_the_cookfile_reaches_the_identity() {
    let outputs: Vec<String> = vec![];
    let inputs = vec!["a.c".to_string()];
    let here = build_local_cache_key("Cookfile", "check", &outputs, &inputs, 0xbeef, 0);
    let moved = build_local_cache_key("sub/Cookfile", "verify", &outputs, &inputs, 0xbeef, 0);
    assert_eq!(here, moved);
}

/// A path reached by two routes — named by `inputs` and again by a step-group
/// dep — must not change what the unit IS. The test payload's input list has
/// deduplicated since COOK-84; the identity now does too.
#[test]
fn reaching_one_path_twice_does_not_change_the_identity() {
    assert_eq!(
        key(&[], &["a.c", "b.c"], 0xbeef, 0),
        key(&[], &["a.c", "b.c", "a.c"], 0xbeef, 0)
    );
}

/// Order is kept, because a reordering IS a change to the declaration. Stated
/// so that a later switch to sorting is a deliberate decision rather than a
/// silent one.
#[test]
fn input_order_is_part_of_the_declaration() {
    assert_ne!(key(&[], &["a.c", "b.c"], 0xbeef, 0), key(&[], &["b.c", "a.c"], 0xbeef, 0));
}
