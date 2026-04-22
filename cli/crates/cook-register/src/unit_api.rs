use mlua::prelude::*;
use cook_contracts::{CacheMeta, CapturedUnit, DepKind, WorkPayload};

use crate::{hash_str, SharedCaptureState};

/// Register `cook.add_unit(table)` and `cook.step_group(fn)` on the cook table.
pub fn register_unit_api(lua: &Lua, capture_state: SharedCaptureState, recipe_name: &str) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    // cook.add_unit(table)
    let cs = capture_state.clone();
    let rname = recipe_name.to_string();
    let add_unit_fn = lua.create_function(move |_, tbl: LuaTable| {
        let command: String = tbl.get::<String>("command").unwrap_or_default();
        let interactive: bool = tbl.get::<Option<bool>>("interactive").unwrap_or(None).unwrap_or(false);
        let line: usize = tbl.get::<Option<usize>>("line").unwrap_or(None).unwrap_or(0);
        let cache_enabled: bool = tbl.get::<Option<bool>>("cache").unwrap_or(None).unwrap_or(true);
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

        let cache_meta = if cache_enabled {
            let cache_key = if let Some(first) = output_paths.first() {
                first.clone()
            } else {
                format!("{}@{:x}", inputs.first().map(|s| s.as_str()).unwrap_or(""), hash_str(&command))
            };
            Some(CacheMeta {
                recipe_name: rname.clone(),
                cache_key,
                input_paths: inputs,
                output_paths: output_paths.clone(),
                command_hash: hash_str(&command),
            })
        } else {
            None
        };

        let payload = if interactive {
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
        }
        result
    })?;
    cook.set("step_group", step_group_fn)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::CaptureState;

    fn make_lua_with_unit_api(recipe_name: &str) -> (Lua, SharedCaptureState) {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));
        register_unit_api(&lua, capture_state.clone(), recipe_name).unwrap();
        (lua, capture_state)
    }

    #[test]
    fn test_add_unit_basic() {
        let (lua, capture_state) = make_lua_with_unit_api("my_recipe");
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
        assert_eq!(meta.cache_key, "main");
        assert_eq!(meta.input_paths, vec!["main.c"]);
        assert_eq!(meta.output_paths, vec!["main".to_string()]);
        assert_eq!(meta.command_hash, hash_str("gcc -o main main.c"));

        assert!(matches!(unit.dep_kind, DepKind::Sequential));
    }

    #[test]
    fn test_add_unit_no_cache() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
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
        // cache_key should be derived from the first output when outputs is used
        assert_eq!(meta.cache_key, "a.o");
    }

    #[test]
    fn test_add_unit_outputs_and_output_conflict_errors() {
        let (lua, _capture_state) = make_lua_with_unit_api("my_recipe");
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
    fn test_single_step_terminal_outputs() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.load(r#"
            cook.step_group(function()
                cook.add_unit({ command = "gcc -o app main.c", inputs = {"main.c"}, output = "app" })
            end)
        "#).exec().unwrap();

        let state = capture_state.borrow();
        assert_eq!(state.last_cook_step_outputs, vec!["app"]);
    }
}
