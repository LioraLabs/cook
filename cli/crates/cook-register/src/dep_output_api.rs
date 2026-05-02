use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mlua::prelude::*;

use crate::SharedCaptureState;

/// Shared storage for terminal outputs of registered recipes, keyed by
/// **fully-qualified** recipe name (e.g., `"lib.lib_build"` or just
/// `"build"` for root-Cookfile recipes). Hoisted to workspace scope so all
/// Registries write to and read from the same map.
pub type SharedTerminalOutputs = Arc<Mutex<BTreeMap<String, Vec<String>>>>;

/// Register `cook.dep_output(name)` and `cook.dep_output_list(name)` on the cook table.
///
/// - `cook.dep_output(name)` returns the terminal outputs of recipe `name` as a space-joined string.
/// - `cook.dep_output_list(name)` returns the terminal outputs of recipe `name` as a Lua table.
///
/// Both functions record a dep edge in `capture_state.dep_edges` for fine-grained DAG wiring.
pub fn register_dep_output_api(
    lua: &Lua,
    terminal_outputs: SharedTerminalOutputs,
    capture_state: SharedCaptureState,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    // cook.dep_output(name) → space-joined string
    // Accumulates dep ref in step_group_dep_refs; actual edge recording
    // happens in cook.add_unit() which attaches the ref to the correct unit.
    let to = terminal_outputs.clone();
    let cs = capture_state.clone();
    let dep_output_fn = lua.create_function(move |_, name: String| {
        let store = to.lock().expect("terminal_outputs mutex poisoned");
        let outputs = store.get(&name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;
        // Accumulate dep ref for the step_group — add_unit will pick it up
        {
            let mut state = cs.borrow_mut();
            if !state.step_group_dep_refs.contains(&name) {
                state.step_group_dep_refs.push(name.clone());
            }
        }
        Ok(outputs.join(" "))
    })?;
    cook.set("dep_output", dep_output_fn)?;

    // cook.dep_output_list(name) → Lua table
    // Same accumulation pattern as dep_output.
    let to2 = terminal_outputs.clone();
    let cs2 = capture_state.clone();
    let dep_output_list_fn = lua.create_function(move |lua, name: String| {
        let store = to2.lock().expect("terminal_outputs mutex poisoned");
        let outputs = store.get(&name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;
        // Accumulate dep ref
        {
            let mut state = cs2.borrow_mut();
            if !state.step_group_dep_refs.contains(&name) {
                state.step_group_dep_refs.push(name.clone());
            }
        }
        let table = lua.create_table()?;
        for (i, path) in outputs.iter().enumerate() {
            table.set(i + 1, path.as_str())?;
        }
        Ok(table)
    })?;
    cook.set("dep_output_list", dep_output_list_fn)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CaptureState;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn setup_lua() -> (Lua, SharedTerminalOutputs, SharedCaptureState) {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        let terminal_outputs: SharedTerminalOutputs = Arc::new(Mutex::new(BTreeMap::new()));
        let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));
        (lua, terminal_outputs, capture_state)
    }

    #[test]
    fn test_dep_output_returns_space_joined() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "protos".into(),
            vec!["gen/foo.pb.o".into(), "gen/bar.pb.o".into()],
        );
        register_dep_output_api(&lua, outputs, cs).unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("protos")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "gen/foo.pb.o gen/bar.pb.o");
    }

    #[test]
    fn test_dep_output_list_returns_table() {
        let (lua, outputs, cs) = setup_lua();
        outputs
            .lock().unwrap()
            .insert("libmath".into(), vec!["build/lib/libmath.a".into()]);
        register_dep_output_api(&lua, outputs, cs).unwrap();
        let result: Vec<String> = lua
            .load(r#"return cook.dep_output_list("libmath")"#)
            .eval()
            .unwrap();
        assert_eq!(result, vec!["build/lib/libmath.a"]);
    }

    #[test]
    fn test_dep_output_unknown_recipe_errors() {
        let (lua, outputs, cs) = setup_lua();
        register_dep_output_api(&lua, outputs, cs).unwrap();
        let result = lua
            .load(r#"return cook.dep_output("nonexistent")"#)
            .eval::<String>();
        assert!(result.is_err());
    }

    #[test]
    fn test_dep_output_accumulates_dep_ref() {
        let (lua, outputs, cs) = setup_lua();
        outputs
            .lock().unwrap()
            .insert("libmath".into(), vec!["libmath.a".into()]);
        register_dep_output_api(&lua, outputs, cs.clone()).unwrap();
        lua.load(r#"cook.dep_output("libmath")"#).exec().unwrap();
        // dep_output accumulates in step_group_dep_refs, not dep_edges directly.
        // Actual edge recording happens in cook.add_unit().
        let state = cs.borrow();
        assert_eq!(state.step_group_dep_refs, vec!["libmath".to_string()]);
        // No direct dep_edges yet — add_unit would create them.
        assert!(state.dep_edges.is_empty());
    }

    #[test]
    fn test_dep_output_deduplicates_refs() {
        let (lua, outputs, cs) = setup_lua();
        outputs
            .lock().unwrap()
            .insert("libmath".into(), vec!["libmath.a".into()]);
        register_dep_output_api(&lua, outputs, cs.clone()).unwrap();
        lua.load(r#"
            cook.dep_output("libmath")
            cook.dep_output("libmath")
        "#).exec().unwrap();
        let state = cs.borrow();
        // Should not duplicate
        assert_eq!(state.step_group_dep_refs, vec!["libmath".to_string()]);
    }
}
