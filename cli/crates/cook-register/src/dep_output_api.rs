use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mlua::prelude::*;

use crate::SharedCaptureState;

/// Shared storage for terminal outputs of registered recipes.
pub type SharedTerminalOutputs = Rc<RefCell<BTreeMap<String, Vec<String>>>>;

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
    let to = terminal_outputs.clone();
    let cs = capture_state.clone();
    let dep_output_fn = lua.create_function(move |_, name: String| {
        let store = to.borrow();
        let outputs = store.get(&name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;
        // Record dep edge
        {
            let mut state = cs.borrow_mut();
            let unit_idx = state.units.len().saturating_sub(1);
            state.dep_edges.push((unit_idx, name.clone()));
        }
        Ok(outputs.join(" "))
    })?;
    cook.set("dep_output", dep_output_fn)?;

    // cook.dep_output_list(name) → Lua table
    let to2 = terminal_outputs.clone();
    let cs2 = capture_state.clone();
    let dep_output_list_fn = lua.create_function(move |lua, name: String| {
        let store = to2.borrow();
        let outputs = store.get(&name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;
        // Record dep edge
        {
            let mut state = cs2.borrow_mut();
            let unit_idx = state.units.len().saturating_sub(1);
            state.dep_edges.push((unit_idx, name.clone()));
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

    fn setup_lua() -> (Lua, SharedTerminalOutputs, SharedCaptureState) {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        let terminal_outputs: SharedTerminalOutputs = Rc::new(RefCell::new(BTreeMap::new()));
        let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));
        (lua, terminal_outputs, capture_state)
    }

    #[test]
    fn test_dep_output_returns_space_joined() {
        let (lua, outputs, cs) = setup_lua();
        outputs.borrow_mut().insert(
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
            .borrow_mut()
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
    fn test_dep_output_records_edge() {
        let (lua, outputs, cs) = setup_lua();
        outputs
            .borrow_mut()
            .insert("libmath".into(), vec!["libmath.a".into()]);
        // Pre-add a unit so unit_idx makes sense
        {
            let mut state = cs.borrow_mut();
            state.units.push(cook_contracts::CapturedUnit {
                payload: cook_contracts::WorkPayload::Shell {
                    cmd: "test".into(),
                    line: 0,
                },
                cache_meta: None,
                dep_kind: cook_contracts::DepKind::Sequential,
            });
        }
        register_dep_output_api(&lua, outputs, cs.clone()).unwrap();
        lua.load(r#"cook.dep_output("libmath")"#).exec().unwrap();
        let state = cs.borrow();
        assert_eq!(state.dep_edges.len(), 1);
        assert_eq!(state.dep_edges[0], (0, "libmath".to_string()));
    }
}
