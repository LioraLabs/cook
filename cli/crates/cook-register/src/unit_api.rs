use mlua::prelude::*;
use cook_contracts::{CacheMeta, CapturedUnit, DepKind, WorkPayload};

use crate::dep_output_api::SharedTerminalOutputs;
use crate::{hash_str, SharedCaptureState};

/// Register `cook.add_unit(table)`, `cook.step_group(fn)`, `cook._enter_chore()`,
/// and `cook._exit_chore()` on the cook table.
pub fn register_unit_api(
    lua: &Lua,
    capture_state: SharedCaptureState,
    recipe_name: &str,
    terminal_outputs: SharedTerminalOutputs,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    // cook._enter_chore() — called by chore-generated Lua before the body runs.
    let cs_enter = capture_state.clone();
    let enter_fn = lua.create_function(move |_, ()| {
        cs_enter.borrow_mut().current_chore_active = true;
        Ok(())
    })?;
    cook.set("_enter_chore", enter_fn)?;

    // cook._exit_chore() — called by chore-generated Lua after the body runs.
    let cs_exit = capture_state.clone();
    let exit_fn = lua.create_function(move |_, ()| {
        cs_exit.borrow_mut().current_chore_active = false;
        Ok(())
    })?;
    cook.set("_exit_chore", exit_fn)?;

    // cook.add_unit(table)
    let cs = capture_state.clone();
    let rname = recipe_name.to_string();
    // terminal_outputs is no longer consulted in add_unit; dep_output_api.rs
    // now accumulates importer-relative rewritten paths in
    // capture_state.step_group_dep_input_paths so that cache_meta.input_paths
    // contains stat-able paths from the importer's working directory.
    let _ = terminal_outputs;
    let add_unit_fn = lua.create_function(move |lua, tbl: LuaTable| {
        let command: String = tbl.get::<String>("command").unwrap_or_default();
        let lua_code: Option<String> = tbl.get::<String>("lua_code").ok();
        let interactive: bool = tbl.get::<Option<bool>>("interactive").unwrap_or(None).unwrap_or(false);
        let line: usize = tbl.get::<Option<usize>>("line").unwrap_or(None).unwrap_or(0);
        let cache_enabled: bool = tbl.get::<Option<bool>>("cache").unwrap_or(None).unwrap_or(true);

        // §{chores.no-caching}: cache = true is not permitted inside a chore body.
        if cache_enabled && cs.borrow().current_chore_active {
            return Err(LuaError::RuntimeError(
                "cook.add_unit: cache = true is not permitted in a chore body \
                 (§{chores.no-caching}); chore units are never cached"
                    .into(),
            ));
        }
        let inputs: Vec<String> = match tbl.get::<LuaTable>("inputs") {
            Ok(t) => t.sequence_values::<String>().filter_map(Result::ok).collect(),
            Err(_) => vec![],
        };
        let output: Option<String> = tbl.get::<String>("output").ok();
        let outputs: Option<Vec<String>> = match tbl.get::<LuaTable>("outputs") {
            Ok(t) => Some(
                t.sequence_values::<String>()
                    .filter_map(Result::ok)
                    .collect(),
            ),
            Err(_) => None,
        };
        let ingredient_groups: Vec<Vec<String>> = match tbl.get::<LuaTable>("ingredient_groups") {
            Ok(outer) => outer
                .sequence_values::<LuaTable>()
                .filter_map(Result::ok)
                .map(|inner| {
                    inner
                        .sequence_values::<String>()
                        .filter_map(Result::ok)
                        .collect()
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        if output.is_some() && outputs.is_some() {
            return Err(LuaError::RuntimeError(
                "cook.add_unit: only one of `output` or `outputs` may be provided".into(),
            ));
        }
        let output_paths: Vec<String> = if let Some(list) = outputs {
            list
        } else if let Some(single) = output {
            vec![single]
        } else {
            Vec::new()
        };
        let outputs_for_tracking = output_paths.clone();

        // 2026-05-02 addendum spec §4.3: cross-recipe dep refs accumulated by
        // cook.dep_output / cook.dep_output_list calls within this step_group
        // produce paths the command consumed via {NAME} substitution. Append
        // those paths to cache_meta.input_paths so cache invalidation tracks
        // dep-output content drift. Keep them out of WorkPayload inputs (which
        // drive _cook_in iteration / Lua-visible inputs).
        //
        // Use step_group_dep_input_paths (the importer-relative rewritten paths
        // accumulated by dep_output_api) rather than reading raw paths from
        // terminal_outputs. The raw paths are importee-relative and cannot be
        // stat'd from the importer's working directory — using them would cause
        // MissingFile errors in record_completion, silently dropping demo.bin.
        let dep_input_paths: Vec<String> = {
            let state = cs.borrow();
            state.step_group_dep_input_paths.clone()
        };
        let cache_input_paths: Vec<String> = inputs
            .iter()
            .cloned()
            .chain(dep_input_paths.into_iter())
            .collect();

        // Read consulted_env_keys from the table and look up values in cook.env
        // (the merged Cookfile-config + process env that the command actually
        // consumed at substitution time, per spec §4.3). Reading from
        // std::env::var would miss config-overlay values and capture process
        // env that the command never saw — both produce false misses.
        let mut consulted_env: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        let env_table: Option<LuaTable> = lua
            .globals()
            .get::<LuaTable>("cook")
            .and_then(|c| c.get::<LuaTable>("env"))
            .ok();
        match tbl.get::<LuaValue>("consulted_env_keys") {
            Ok(LuaValue::Table(list)) => {
                if let Some(env) = &env_table {
                    for v in list.sequence_values::<String>().flatten() {
                        if let Ok(val) = env.get::<String>(v.clone()) {
                            consulted_env.insert(v, val);
                        }
                    }
                }
            }
            Ok(LuaValue::String(s)) if s.to_str().ok().as_deref() == Some("*") => {
                if let Some(env) = &env_table {
                    for pair in env.clone().pairs::<String, String>() {
                        if let Ok((k, v)) = pair {
                            consulted_env.insert(k, v);
                        }
                    }
                }
            }
            _ => {}
        }

        let command_hash = if let Some(code) = &lua_code {
            hash_str(code)
        } else {
            hash_str(&command)
        };

        // Retrieve the CacheContext if it was threaded in from cook-engine.
        // If absent (tests, legacy call sites), fall back to zero values.
        let cache_ctx = lua
            .app_data_ref::<std::sync::Arc<cook_cache::cache_ctx::CacheContext>>();

        let (context_hash, env_contribution_val, project_id, cookfile_path) =
            if let Some(ctx) = cache_ctx {
                let ch = ctx.exec_ctx.step_context_hash(&command);
                let ec = cook_cache::envkey::env_contribution(&consulted_env, &ctx.denylist);
                let pid = ctx.project_id.clone();
                let cfp = cookfile_relative_path(lua);
                (ch, ec, pid, cfp)
            } else {
                (0, 0, String::new(), cookfile_relative_path(lua))
            };

        let cache_meta = if cache_enabled {
            let cache_key = build_local_cache_key(
                &cookfile_path,
                &rname,
                &output_paths,
                &cache_input_paths,
                command_hash,
                context_hash,
                env_contribution_val,
            );
            Some(CacheMeta {
                recipe_name: rname.clone(),
                project_id,
                cookfile_path,
                cache_key,
                input_paths: cache_input_paths,
                output_paths: output_paths.clone(),
                command_hash,
                context_hash,
                env_contribution: env_contribution_val,
                consulted_env,
            })
        } else {
            None
        };

        let payload = if let Some(code) = lua_code {
            WorkPayload::LuaChunk {
                code,
                inputs,
                outputs: output_paths.clone(),
                ingredient_groups,
            }
        } else if interactive {
            WorkPayload::Interactive { cmd: command, line }
        } else {
            WorkPayload::Shell { cmd: command, line: 0 }
        };

        let mut state = cs.borrow_mut();
        let dep_kind = if let Some(group_idx) = state.current_group {
            DepKind::StepGroup(group_idx)
        } else {
            DepKind::Sequential
        };
        let unit_idx = state.units.len();
        state.units.push(CapturedUnit {
            payload,
            cache_meta,
            dep_kind: dep_kind.clone(),
        });
        if let DepKind::StepGroup(gi) = &dep_kind {
            state.step_groups[*gi].push(unit_idx);
        }
        for out in outputs_for_tracking {
            state.current_step_outputs.push(out);
        }
        // Record dep edges: every dep ref accumulated in this step_group
        // applies to this unit.
        let dep_refs: Vec<String> = state.step_group_dep_refs.clone();
        for dep_name in dep_refs {
            state.dep_edges.push((unit_idx, dep_name));
        }
        Ok(())
    })?;
    cook.set("add_unit", add_unit_fn)?;

    // cook.step_group(fn)
    let cs2 = capture_state.clone();
    let step_group_fn = lua.create_function(move |_, func: LuaFunction| {
        {
            let mut state = cs2.borrow_mut();
            let group_idx = state.step_groups.len();
            state.step_groups.push(Vec::new());
            state.current_group = Some(group_idx);
        }
        let result = func.call::<()>(());
        {
            let mut state = cs2.borrow_mut();
            state.current_group = None;
            let outputs: Vec<String> = state.current_step_outputs.drain(..).collect();
            if !outputs.is_empty() {
                state.last_cook_step_outputs = outputs;
            }
            state.step_group_dep_refs.clear();
            state.step_group_dep_input_paths.clear();
        }
        result
    })?;
    cook.set("step_group", step_group_fn)?;

    Ok(())
}

/// Build a local cache key that encodes context_hash and env_contribution
/// so simultaneous variant builds (debug↔release, gcc↔clang) coexist
/// without overwriting each other.
fn build_local_cache_key(
    _cookfile_path: &str,
    _recipe: &str,
    output_paths: &[String],
    inputs: &[String],
    command_hash: u64,
    context_hash: u64,
    env_contribution: u64,
) -> String {
    if let Some(first) = output_paths.first() {
        // When context or env differ from zero (real values), embed them to
        // avoid cross-variant collisions.
        if context_hash != 0 || env_contribution != 0 {
            format!(
                "{first}@{:x}:{:x}",
                context_hash, env_contribution
            )
        } else {
            first.clone()
        }
    } else {
        let base = inputs.first().map(|s| s.as_str()).unwrap_or("");
        if context_hash != 0 || env_contribution != 0 {
            format!(
                "{}@{:x}:{:x}:{:x}",
                base, command_hash, context_hash, env_contribution
            )
        } else {
            format!("{}@{:x}", base, command_hash)
        }
    }
}

/// Retrieve the cookfile-relative path stored in the Lua named registry value
/// `__cook_cookfile_path`. Falls back to "Cookfile" when absent (legacy / test
/// call sites that don't thread a `CacheContext` through).
fn cookfile_relative_path(lua: &Lua) -> String {
    lua.named_registry_value::<String>("__cook_cookfile_path")
        .unwrap_or_else(|_| "Cookfile".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::CaptureState;
    use std::collections::BTreeMap;

    fn make_lua_with_unit_api(recipe_name: &str) -> (Lua, SharedCaptureState) {
        use std::sync::{Arc, Mutex};
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));
        let terminal_outputs: SharedTerminalOutputs =
            Arc::new(Mutex::new(BTreeMap::new()));
        register_unit_api(&lua, capture_state.clone(), recipe_name, terminal_outputs).unwrap();
        (lua, capture_state)
    }

    fn fake_cache_ctx() -> std::sync::Arc<cook_cache::cache_ctx::CacheContext> {
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();
        std::mem::forget(dir); // tests are short-lived; let the OS clean up
        std::sync::Arc::new(cook_cache::cache_ctx::CacheContext {
            exec_ctx: std::sync::Arc::new(cook_cache::context::ExecutionContext::probe()),
            denylist: std::sync::Arc::new(cook_cache::envkey::EnvDenylist::baseline()),
            backend: std::sync::Arc::new(cook_cache::backend::LocalBackend::new(dir_path.clone())),
            cloud_config: std::sync::Arc::new(cook_cache::cloud_config::CloudConfig::default()),
            project_root: dir_path,
            project_id: "test-project".to_string(),
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

        let state = capture_state.borrow();
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

        let state = capture_state.borrow();
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

        let state = capture_state.borrow();
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

        let state = capture_state.borrow();
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

        let state = capture_state.borrow();
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

        let state = capture_state.borrow();
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

        let state = capture_state.borrow();
        // Terminal outputs = from the LAST step group that produced outputs: ["lib.a"]
        assert_eq!(state.last_cook_step_outputs, vec!["lib.a"]);
    }

    #[test]
    fn test_plate_step_group_does_not_overwrite_terminal() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            -- Cook step produces output
            cook.step_group(function()
                cook.add_unit({ command = "gcc -o app main.c", inputs = {"main.c"}, output = "app" })
            end)
            -- Plate-like step (no output field) -- should NOT overwrite terminal
            cook.step_group(function()
                cook.add_unit({ command = "./app", cache = false })
            end)
        "#).exec().unwrap();

        let state = capture_state.borrow();
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

        let state = capture_state.borrow();
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

        let state = capture_state.borrow();
        assert_eq!(state.units.len(), 1);
        let unit = &state.units[0];
        match &unit.payload {
            WorkPayload::LuaChunk {
                code,
                inputs,
                outputs,
                ingredient_groups,
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

        let state = capture_state.borrow();
        assert_eq!(state.units.len(), 1);
        match &state.units[0].payload {
            WorkPayload::LuaChunk {
                code,
                inputs,
                outputs,
                ingredient_groups,
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

        let state = capture_state.borrow();
        assert_eq!(state.last_cook_step_outputs, vec!["app"]);
    }

    #[test]
    fn add_unit_populates_consulted_env_from_keys_list() {
        // The lookup reads from cook.env (the Cook Lua VM env table), NOT the
        // process env — that's the merged config-overlay+process value the
        // command actually consumed. Populate cook.env directly here; in real
        // usage, capture.rs seeds cook.env from process env at startup and
        // config dispatch may overlay project-specific values.
        let lua = Lua::new();
        let cook_table = lua.create_table().unwrap();
        let env_table = lua.create_table().unwrap();
        env_table.set("FOO_TEST_VAR_X", "the-value").unwrap();
        cook_table.set("env", env_table).unwrap();
        lua.globals().set("cook", cook_table).unwrap();

        let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));
        let terminal_outputs: SharedTerminalOutputs =
            std::sync::Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        register_unit_api(&lua, capture_state.clone(), "my_recipe", terminal_outputs).unwrap();

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

        let state = capture_state.borrow();
        assert_eq!(state.units.len(), 1);
        let meta = state.units[0].cache_meta.as_ref().expect("cache_meta");
        assert_eq!(
            meta.consulted_env.get("FOO_TEST_VAR_X").map(|s| s.as_str()),
            Some("the-value"),
            "consulted_env must contain FOO_TEST_VAR_X=the-value (read from cook.env)"
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

        let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));
        let terminal_outputs: SharedTerminalOutputs = std::sync::Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        terminal_outputs
            .lock().unwrap()
            .insert("greet".into(), vec!["build/greet.o".into()]);
        terminal_outputs
            .lock().unwrap()
            .insert("util".into(), vec!["build/util.o".into()]);

        register_unit_api(
            &lua,
            capture_state.clone(),
            "demo",
            terminal_outputs.clone(),
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

        let state = capture_state.borrow();
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
}
