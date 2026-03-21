use mlua::prelude::*;

use cook_contracts::{CapturedUnit, DepKind, WorkPayload};

use crate::SharedCaptureState;

/// Register `cook.add_test(table)` on the cook table.
///
/// cook.add_test captures a test work unit with timeout/should_fail metadata.
/// Uses DepKind::TestSibling so test failures don't cancel siblings.
pub fn register_test_api(lua: &Lua, capture_state: SharedCaptureState) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    let cs = capture_state.clone();
    let add_test_fn = lua.create_function(move |_, tbl: LuaTable| {
        let command: String = tbl.get::<String>("command").unwrap_or_default();
        let suite_name: String = tbl.get::<String>("suite").unwrap_or_default();
        let test_name: String = tbl.get::<String>("name").unwrap_or_default();
        let timeout: u64 = tbl.get::<Option<u64>>("timeout")?.unwrap_or(300);
        let should_fail: bool = tbl.get::<Option<bool>>("should_fail")?.unwrap_or(false);

        let payload = WorkPayload::Test {
            cmd: command,
            line: 0,
            timeout,
            should_fail,
            suite_name,
            test_name,
        };

        let mut state = cs.borrow_mut();
        let dep_kind = if let Some(group_idx) = state.current_group {
            DepKind::TestSibling(group_idx)
        } else {
            DepKind::Sequential
        };
        let unit_idx = state.units.len();
        state.units.push(CapturedUnit {
            payload,
            cache_meta: None,
            dep_kind: dep_kind.clone(),
        });
        if let DepKind::TestSibling(gi) = &dep_kind {
            state.step_groups[*gi].push(unit_idx);
        }

        Ok(())
    })?;
    cook.set("add_test", add_test_fn)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::CaptureState;

    fn make_lua_with_test_api() -> (Lua, SharedCaptureState) {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));
        register_test_api(&lua, capture_state.clone()).unwrap();
        (lua, capture_state)
    }

    #[test]
    fn test_add_test_basic() {
        let (lua, capture_state) = make_lua_with_test_api();
        lua.load(r#"
            cook.add_test({
                command = "./run_tests",
                suite = "unit",
                name = "test_foo",
                timeout = 30,
                should_fail = false,
            })
        "#).exec().unwrap();

        let state = capture_state.borrow();
        assert_eq!(state.units.len(), 1);
        match &state.units[0].payload {
            WorkPayload::Test { cmd, timeout, should_fail, suite_name, test_name, .. } => {
                assert_eq!(cmd, "./run_tests");
                assert_eq!(*timeout, 30);
                assert!(!should_fail);
                assert_eq!(suite_name, "unit");
                assert_eq!(test_name, "test_foo");
            }
            _ => panic!("expected Test payload"),
        }
        assert!(matches!(state.units[0].dep_kind, DepKind::Sequential));
    }

    #[test]
    fn test_add_test_defaults() {
        let (lua, capture_state) = make_lua_with_test_api();
        lua.load(r#"
            cook.add_test({
                command = "./test",
                suite = "s",
                name = "t",
            })
        "#).exec().unwrap();

        let state = capture_state.borrow();
        match &state.units[0].payload {
            WorkPayload::Test { timeout, should_fail, .. } => {
                assert_eq!(*timeout, 300);
                assert!(!should_fail);
            }
            _ => panic!("expected Test payload"),
        }
    }
}
