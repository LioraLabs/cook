use mlua::prelude::*;

use cook_contracts::{CapturedUnit, DepKind, WorkPayload};

use crate::SharedBodySlot;

/// Register `cook.add_test(table)` on the cook table.
///
/// cook.add_test captures a test work unit with timeout/should_fail metadata.
/// Uses DepKind::TestSibling so test failures don't cancel siblings.
pub fn register_test_api(lua: &Lua, body_slot: SharedBodySlot) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    let body_slot_add = body_slot.clone();
    let add_test_fn = lua.create_function(move |_, tbl: LuaTable| {
        // CS-0061 §3.2: `command` is required and must be non-empty.
        let command: String = tbl
            .get::<Option<String>>("command")?
            .ok_or_else(|| mlua::Error::external("cook.add_test: command field is required"))?;
        if command.is_empty() {
            return Err(mlua::Error::external(
                "cook.add_test: command field is required and must be a non-empty string",
            ));
        }

        // CS-0061 §3.2: `timeout` must be a positive integer; default 300.
        let timeout: u64 = tbl.get::<Option<u64>>("timeout")?.unwrap_or(300);
        if timeout == 0 {
            return Err(mlua::Error::external(
                "cook.add_test: timeout must be a positive number, got 0",
            ));
        }

        // CS-0061 §3.2: `suite` defaults to the enclosing recipe's name.
        let suite_name: String = match tbl.get::<Option<String>>("suite")? {
            Some(s) if !s.is_empty() => s,
            _ => {
                let slot = body_slot_add.borrow();
                let body = slot.as_ref().ok_or_else(|| {
                    mlua::Error::runtime("cook.add_test called outside a recipe body")
                })?;
                body.current_recipe.clone().unwrap_or_default()
            }
        };

        let test_name: String = tbl.get::<Option<String>>("name")?.unwrap_or_default();
        let should_fail: bool = tbl.get::<Option<bool>>("should_fail")?.unwrap_or(false);
        let line: usize = tbl.get::<Option<i64>>("line")?.unwrap_or(0).max(0) as usize;
        let iteration_item: Option<String> = tbl.get::<Option<String>>("iteration_item")?
            .filter(|s| !s.is_empty());

        let payload = WorkPayload::Test {
            cmd: command,
            line,
            timeout,
            should_fail,
            suite_name,
            test_name,
            iteration_item,
        };

        let mut slot = body_slot_add.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            mlua::Error::runtime("cook.add_test called outside a recipe body")
        })?;
        let dep_kind = if let Some(group_idx) = body.current_group {
            DepKind::TestSibling(group_idx)
        } else {
            DepKind::Sequential
        };
        let unit_idx = body.units.len();
        body.units.push(CapturedUnit {
            payload,
            cache_meta: None,
            dep_kind: dep_kind.clone(),
            probes: vec![],
        });
        if let DepKind::TestSibling(gi) = &dep_kind {
            body.step_groups[*gi].push(unit_idx);
        }
        // Mirrors cook.add_unit: every dep ref accumulated in this step_group
        // (e.g. via cook.dep_output("alias.recipe") calls inside the test
        // body) must be wired as a dep edge for this unit, so the wave
        // grouper schedules the upstream recipe before this test runs.
        // Without this, a test body refing a sibling recipe races that
        // sibling under --jobs > 1.
        let dep_refs: Vec<String> = body.step_group_dep_refs.clone();
        for dep_name in dep_refs {
            body.dep_edges.push((unit_idx, dep_name));
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
    use crate::BodyCaptureState;

    fn body_ref(body_slot: &SharedBodySlot) -> std::cell::Ref<'_, BodyCaptureState> {
        std::cell::Ref::map(body_slot.borrow(), |slot| {
            slot.as_ref().expect("body slot populated for test")
        })
    }

    fn make_lua_with_test_api() -> (Lua, SharedBodySlot) {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        let body_slot: SharedBodySlot =
            Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        register_test_api(&lua, body_slot.clone()).unwrap();
        (lua, body_slot)
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

        let state = body_ref(&capture_state);
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

    /// Regression: a test body that calls `cook.dep_output("X")` (lowered from
    /// a `{X}` body ref) must propagate that dep into `state.dep_edges` so the
    /// wave grouper schedules X before the test runs. Pre-fix, add_test
    /// dropped step_group_dep_refs on the floor and the test raced X under
    /// --jobs > 1.
    #[test]
    fn test_add_test_propagates_step_group_dep_refs_to_dep_edges() {
        let (lua, capture_state) = make_lua_with_test_api();
        // Seed a dep ref as if cook.dep_output("upstream") had been called
        // earlier in the same step group (codegen lowering of a `{upstream}`
        // body ref).
        capture_state
            .borrow_mut()
            .as_mut()
            .expect("body slot populated for test")
            .step_group_dep_refs
            .push("upstream".to_string());

        lua.load(r#"
            cook.add_test({
                command = "./check",
                suite = "s",
                name = "t",
            })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        // unit_idx 0 must have an edge to "upstream".
        assert_eq!(state.dep_edges, vec![(0usize, "upstream".to_string())]);
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

        let state = body_ref(&capture_state);
        match &state.units[0].payload {
            WorkPayload::Test { timeout, should_fail, .. } => {
                assert_eq!(*timeout, 300);
                assert!(!should_fail);
            }
            _ => panic!("expected Test payload"),
        }
    }

    // -----------------------------------------------------------------
    // CS-0061 §3.2 field-defaults contract tests
    // -----------------------------------------------------------------

    #[test]
    fn add_test_defaults_suite_to_recipe_name() {
        let (lua, capture_state) = make_lua_with_test_api();
        capture_state
            .borrow_mut()
            .as_mut()
            .expect("body slot populated for test")
            .current_recipe = Some("frontend.unit".to_string());

        lua.load(r#"
            cook.add_test({ command = "true" })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        let payload = match &state.units[0].payload {
            WorkPayload::Test { suite_name, .. } => suite_name,
            _ => panic!("expected Test payload"),
        };
        assert_eq!(payload, "frontend.unit");
    }

    #[test]
    fn add_test_rejects_empty_command() {
        let (lua, capture_state) = make_lua_with_test_api();
        capture_state
            .borrow_mut()
            .as_mut()
            .expect("body slot populated for test")
            .current_recipe = Some("r".to_string());

        let res = lua.load(r#"
            cook.add_test({ command = "" })
        "#).exec();

        assert!(res.is_err(), "empty command must be rejected");
        assert!(format!("{:?}", res).contains("command"));
    }

    #[test]
    fn add_test_rejects_missing_command() {
        let (lua, capture_state) = make_lua_with_test_api();
        capture_state
            .borrow_mut()
            .as_mut()
            .expect("body slot populated for test")
            .current_recipe = Some("r".to_string());

        let res = lua.load(r#"
            cook.add_test({ name = "x" })
        "#).exec();

        assert!(res.is_err(), "missing command must be rejected");
    }

    #[test]
    fn add_test_rejects_non_positive_timeout() {
        let (lua, capture_state) = make_lua_with_test_api();
        capture_state
            .borrow_mut()
            .as_mut()
            .expect("body slot populated for test")
            .current_recipe = Some("r".to_string());

        let res = lua.load(r#"
            cook.add_test({ command = "true", timeout = 0 })
        "#).exec();

        assert!(res.is_err());
        assert!(format!("{:?}", res).contains("timeout"));
    }

    #[test]
    fn add_test_explicit_suite_overrides_default() {
        let (lua, capture_state) = make_lua_with_test_api();
        capture_state
            .borrow_mut()
            .as_mut()
            .expect("body slot populated for test")
            .current_recipe = Some("r".to_string());

        lua.load(r#"
            cook.add_test({ command = "true", suite = "explicit" })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        let suite = match &state.units[0].payload {
            WorkPayload::Test { suite_name, .. } => suite_name,
            _ => panic!(),
        };
        assert_eq!(suite, "explicit");
    }
}
