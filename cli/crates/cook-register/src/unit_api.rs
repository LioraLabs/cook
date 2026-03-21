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
        let cache_enabled: bool = tbl.get::<Option<bool>>("cache").unwrap_or(None).unwrap_or(true);
        let inputs: Vec<String> = match tbl.get::<LuaTable>("inputs") {
            Ok(t) => t.sequence_values::<String>().filter_map(Result::ok).collect(),
            Err(_) => vec![],
        };
        let output: Option<String> = tbl.get::<String>("output").ok();

        let cache_meta = if cache_enabled {
            let cache_key = if let Some(ref out) = output {
                out.clone()
            } else {
                format!("{}@{:x}", inputs.first().map(|s| s.as_str()).unwrap_or(""), hash_str(&command))
            };
            Some(CacheMeta {
                recipe_name: rname.clone(),
                cache_key,
                input_paths: inputs,
                output_path: output,
                command_hash: hash_str(&command),
            })
        } else {
            None
        };

        let mut state = cs.borrow_mut();
        let dep_kind = if let Some(group_idx) = state.current_group {
            DepKind::StepGroup(group_idx)
        } else {
            DepKind::Sequential
        };
        let unit_idx = state.units.len();
        state.units.push(CapturedUnit {
            payload: WorkPayload::Shell { cmd: command, line: 0 },
            cache_meta,
            dep_kind: dep_kind.clone(),
        });
        if let DepKind::StepGroup(gi) = &dep_kind {
            state.step_groups[*gi].push(unit_idx);
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
        assert_eq!(meta.output_path, Some("main".to_string()));
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
}
