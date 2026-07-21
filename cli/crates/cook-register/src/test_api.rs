use mlua::prelude::*;

use cook_contracts::{CapturedUnit, DepKind, WorkPayload};

use crate::SharedBodySlot;

/// Uniform register-phase type error for a `cook.add_test` field (CS-0127):
/// a wrong-typed field is a hard error naming the field, the expected type,
/// and the received Lua type — never silently coerced to its default. Mirrors
/// `unit_api::type_err`.
fn type_err(field: &str, expected: &str, got: &str) -> LuaError {
    LuaError::runtime(format!(
        "cook.add_test: `{field}` must be {expected}, got {got} (Standard \u{00a7}22.4, CS-0127)"
    ))
}

/// Register `cook.add_test(table)` on the cook table.
///
/// cook.add_test captures a test work unit (Standard §22.4: `command`/
/// `lua_code`, `suite`, `line`, `iteration_item`, and `inputs`). Uses
/// DepKind::TestSibling so test failures don't cancel siblings.
pub fn register_test_api(lua: &Lua, body_slot: SharedBodySlot) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    let body_slot_add = body_slot.clone();
    let add_test_fn = lua.create_function(move |_, tbl: LuaTable| {
        // CS-0127 §22.4: `command`, if present, must be a string — never
        // coerced. An empty string is treated as absent (`None`) so the
        // exactly-one check below reports it as missing, not as a supplied
        // value, matching the historical empty-command diagnostic.
        let command: Option<String> = match tbl.get::<LuaValue>("command") {
            Ok(LuaValue::Nil) | Err(_) => None,
            Ok(LuaValue::String(s)) => Some(s.to_string_lossy().to_string()),
            Ok(other) => return Err(type_err("command", "a string", other.type_name())),
        }
        .filter(|s| !s.is_empty());
        // CS-0127 §22.4: `lua_code`, if present, must be a string — never
        // coerced. Empty string treated as absent, as for `command`.
        let lua_code: Option<String> = match tbl.get::<LuaValue>("lua_code") {
            Ok(LuaValue::Nil) | Err(_) => None,
            Ok(LuaValue::String(s)) => Some(s.to_string_lossy().to_string()),
            Ok(other) => return Err(type_err("lua_code", "a string", other.type_name())),
        }
        .filter(|s| !s.is_empty());
        // CS-0127 §22.4: exactly one of `command` / `lua_code` MUST be
        // provided non-empty. Both empty/absent → the "required" arm (message
        // names `command`, the historical field); both present → "got both".
        let (command, lua_code) = match (command, lua_code) {
            (Some(c), None) => (c, None),
            (None, Some(l)) => (String::new(), Some(l)),
            (Some(_), Some(_)) => {
                return Err(mlua::Error::runtime(
                    "cook.add_test: exactly one of `command` or `lua_code` must be provided, got both (Standard \u{00a7}22.4, CS-0127)",
                ))
            }
            _ => {
                return Err(mlua::Error::runtime(
                    "cook.add_test: exactly one of `command` or `lua_code` is required and must be a non-empty string (Standard \u{00a7}22.4, CS-0127)",
                ))
            }
        };

        // CS-0135 §22.4 / §7: `cook.add_test` no longer accepts a `timeout`
        // field (the `test` step's `timeout` modifier was removed), and there
        // is no per-test time bound in v1.0 — a hung test hangs the run, the
        // same as `make` (App. E CS-0135). We therefore pass an effectively
        // unbounded timeout so the executor's kill loop never fires. The
        // `WorkPayload::Test::timeout` field stays populated for the engine
        // (and for the planned 1.x per-test-timeout re-add).
        let timeout: u64 = u64::MAX;

        // CS-0127 §22.4: `suite` defaults to the enclosing recipe's name.
        let suite_name: String = match tbl.get::<LuaValue>("suite") {
            Ok(LuaValue::Nil) | Err(_) => {
                let slot = body_slot_add.borrow();
                let body = slot.as_ref().ok_or_else(|| {
                    mlua::Error::runtime("cook.add_test called outside a recipe body")
                })?;
                body.current_recipe.clone().unwrap_or_default()
            }
            Ok(LuaValue::String(s)) => {
                let sv = s.to_string_lossy().to_string();
                if sv.is_empty() {
                    let slot = body_slot_add.borrow();
                    let body = slot.as_ref().ok_or_else(|| {
                        mlua::Error::runtime("cook.add_test called outside a recipe body")
                    })?;
                    body.current_recipe.clone().unwrap_or_default()
                } else {
                    sv
                }
            }
            Ok(other) => return Err(type_err("suite", "a string", other.type_name())),
        };

        // CS-0135 §22.4: `cook.add_test` no longer accepts a `name` field
        // (the `test` step's `as` modifier substitutes at codegen time,
        // not through this table field). `WorkPayload::Test::test_name`
        // stays populated for the engine executor (label/verdict
        // derivation), defaulting to the same empty string the field
        // used to fall back to when absent.
        let test_name: String = String::new();
        // CS-0135 §22.4: `cook.add_test` no longer accepts a
        // `should_fail` field (the `test` step's `should_fail` modifier
        // was removed). `WorkPayload::Test::should_fail` stays
        // populated for the engine executor's pass/fail inversion,
        // defaulting to the same value the field used to fall back to
        // when absent.
        let should_fail: bool = false;
        // CS-0127 §22.4: `line` must be a non-negative integer — never
        // coerced.
        let line: usize = match tbl.get::<LuaValue>("line") {
            Ok(LuaValue::Nil) | Err(_) => 0,
            Ok(LuaValue::Integer(n)) if n >= 0 => n as usize,
            Ok(other) => return Err(type_err("line", "a non-negative integer", other.type_name())),
        };
        // CS-0127 §22.4: `iteration_item` must be a string — never coerced.
        let iteration_item: Option<String> = match tbl.get::<LuaValue>("iteration_item") {
            Ok(LuaValue::Nil) | Err(_) => None,
            Ok(LuaValue::String(s)) => {
                let sv = s.to_string_lossy().to_string();
                if sv.is_empty() { None } else { Some(sv) }
            }
            Ok(other) => return Err(type_err("iteration_item", "a string", other.type_name())),
        };

        // COOK-84: declared ingredient files for this test (codegen passes the
        // recipe's resolved `ingredients` local). Unioned below with the step
        // group's dep-output paths, mirroring cook.add_unit's cache_input_paths
        // (unit_api.rs). CS-0127: `inputs` must be a table of strings —
        // never coerced (including mlua's implicit number-to-string
        // coercion on elements).
        let inputs: Vec<String> = match tbl.get::<LuaValue>("inputs") {
            Ok(LuaValue::Nil) | Err(_) => vec![],
            Ok(LuaValue::Table(t)) => {
                let mut out = Vec::new();
                for v in t.sequence_values::<LuaValue>() {
                    let v = v.map_err(|e| {
                        LuaError::runtime(format!("cook.add_test: `inputs`: {e}"))
                    })?;
                    match v {
                        LuaValue::String(s) => out.push(s.to_string_lossy().to_string()),
                        other => {
                            return Err(type_err("inputs", "a table of strings", other.type_name()))
                        }
                    }
                }
                out
            }
            Ok(other) => return Err(type_err("inputs", "a table of strings", other.type_name())),
        };

        // CS-0159: opts.seal — the test unit's effective seal set (bare probe
        // keys). Mirrors `cook.add_unit`'s `seal` field exactly: only the KEY
        // set is register-time data; the canonical VALUES fold into the test
        // fingerprint at execute phase, once the sealed probes have run.
        let seal_keys: std::collections::BTreeSet<String> = match tbl.get::<LuaValue>("seal") {
            Ok(LuaValue::Nil) | Err(_) => Default::default(),
            Ok(LuaValue::Table(t)) => {
                let mut out = std::collections::BTreeSet::new();
                for v in t.sequence_values::<LuaValue>() {
                    let v = v.map_err(|e| {
                        LuaError::runtime(format!("cook.add_test: `seal`: {e}"))
                    })?;
                    match v {
                        LuaValue::String(s) => {
                            out.insert(s.to_string_lossy().to_string());
                        }
                        other => {
                            return Err(type_err("seal", "a table of strings", other.type_name()))
                        }
                    }
                }
                out
            }
            Ok(other) => return Err(type_err("seal", "a table of strings", other.type_name())),
        };

        let mut slot = body_slot_add.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            mlua::Error::runtime("cook.add_test called outside a recipe body")
        })?;
        // COOK-84: inputs ∪ step_group_dep_input_paths, deduped, order-preserving.
        let mut input_paths: Vec<String> = inputs;
        for p in &body.step_group_dep_input_paths {
            if !input_paths.contains(p) {
                input_paths.push(p.clone());
            }
        }
        let payload = WorkPayload::Test {
            cmd: command,
            line,
            timeout,
            should_fail,
            suite_name,
            test_name,
            iteration_item,
            lua_code,
            input_paths,
            seal_keys: seal_keys.clone(),
        };
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
            // CS-0159: sealed probes are execute-phase determinants — the test
            // must run after them so their values are materialised before the
            // ready-time fingerprint is computed. Same wiring `cook.add_unit`
            // does for a sealing cook unit (unit_api.rs); without it the fold
            // would silently read an empty value for every sealed key.
            probes: seal_keys.into_iter().collect(),
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
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
            })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        match &state.units[0].payload {
            WorkPayload::Test { cmd, timeout, should_fail, suite_name, test_name, .. } => {
                assert_eq!(cmd, "./run_tests");
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
            })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        match &state.units[0].payload {
            WorkPayload::Test { timeout, should_fail, test_name, .. } => {
                assert_eq!(*timeout, u64::MAX); // CS-0135: no per-test time bound
                assert!(!should_fail);
                assert_eq!(test_name, "");
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

    // CS-0135 §22.4: `cook.add_test` no longer accepts a `timeout` field,
    // so the prior `add_test_rejects_non_positive_timeout` /
    // `add_test_rejects_non_integer_timeout` field-typing regression
    // tests no longer have a live contract to cover (the field is
    // silently ignored, not validated).

    // -----------------------------------------------------------------
    // COOK-84: input_paths capture (ingredients ∪ step-group dep paths)
    // -----------------------------------------------------------------

    #[test]
    fn add_test_captures_inputs_into_payload() {
        let (lua, capture_state) = make_lua_with_test_api();
        lua.load(r#"
            cook.add_test({
                command = "cargo test",
                suite = "s",
                name = "t",
                inputs = { "src/lib.rs", "src/main.rs" },
            })
        "#).exec().unwrap();
        let state = body_ref(&capture_state);
        match &state.units[0].payload {
            WorkPayload::Test { input_paths, .. } => {
                assert_eq!(input_paths, &["src/lib.rs".to_string(), "src/main.rs".to_string()]);
            }
            _ => panic!("expected Test payload"),
        }
    }

    #[test]
    fn add_test_unions_step_group_dep_input_paths() {
        let (lua, capture_state) = make_lua_with_test_api();
        capture_state.borrow_mut().as_mut().expect("body slot populated for test")
            .step_group_dep_input_paths
            .extend(["../core/build/core.so".to_string(), "src/lib.rs".to_string()]);
        lua.load(r#"
            cook.add_test({
                command = "pytest",
                suite = "s",
                name = "t",
                inputs = { "src/lib.rs" },
            })
        "#).exec().unwrap();
        let state = body_ref(&capture_state);
        match &state.units[0].payload {
            WorkPayload::Test { input_paths, .. } => {
                // union, deduped, declared inputs first
                assert_eq!(input_paths, &["src/lib.rs".to_string(), "../core/build/core.so".to_string()]);
            }
            _ => panic!("expected Test payload"),
        }
    }

    #[test]
    fn add_test_without_inputs_still_carries_dep_paths() {
        let (lua, capture_state) = make_lua_with_test_api();
        capture_state.borrow_mut().as_mut().expect("body slot populated for test")
            .step_group_dep_input_paths
            .push("build/lib.txt".to_string());
        lua.load(r#"
            cook.add_test({ command = "true", suite = "s", name = "t" })
        "#).exec().unwrap();
        let state = body_ref(&capture_state);
        match &state.units[0].payload {
            WorkPayload::Test { input_paths, .. } => {
                assert_eq!(input_paths, &["build/lib.txt".to_string()]);
            }
            _ => panic!("expected Test payload"),
        }
    }

    // -----------------------------------------------------------------
    // CS-0127 §22.4: lua_code XOR command, strict field typing
    // -----------------------------------------------------------------

    #[test]
    fn add_test_accepts_lua_code_without_command() {
        let (lua, capture_state) = make_lua_with_test_api();
        capture_state
            .borrow_mut()
            .as_mut()
            .expect("body slot populated for test")
            .current_recipe = Some("r".to_string());

        lua.load(r#"
            cook.add_test({ lua_code = "assert(true)", suite = "s", name = "t" })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        match &state.units[0].payload {
            WorkPayload::Test { cmd, lua_code, .. } => {
                assert_eq!(lua_code.as_deref(), Some("assert(true)"));
                assert_eq!(cmd, "");
            }
            _ => panic!("expected Test payload"),
        }
    }

    #[test]
    fn add_test_empty_lua_code_alongside_command_is_a_command_test() {
        // An empty `lua_code` is treated as absent, so `command` alone is a
        // valid command test — not a spurious "got both" rejection.
        let (lua, capture_state) = make_lua_with_test_api();
        capture_state
            .borrow_mut()
            .as_mut()
            .expect("body slot populated for test")
            .current_recipe = Some("r".to_string());

        lua.load(r#"
            cook.add_test({ command = "true", lua_code = "", suite = "s", name = "t" })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        match &state.units[0].payload {
            WorkPayload::Test { cmd, lua_code, .. } => {
                assert_eq!(cmd, "true");
                assert_eq!(lua_code.as_deref(), None);
            }
            _ => panic!("expected Test payload"),
        }
    }

    #[test]
    fn add_test_rejects_both_command_and_lua_code() {
        let (lua, _capture_state) = make_lua_with_test_api();
        let res = lua.load(r#"
            cook.add_test({ command = "true", lua_code = "assert(true)" })
        "#).exec();

        assert!(res.is_err(), "both command and lua_code must be rejected");
        assert!(format!("{:?}", res).contains("exactly one"), "got: {:?}", res);
    }

    #[test]
    fn add_test_rejects_non_string_command() {
        let (lua, _capture_state) = make_lua_with_test_api();
        let res = lua.load(r#"
            cook.add_test({ command = function() end })
        "#).exec();

        let msg = format!("{:?}", res);
        assert!(res.is_err(), "non-string command must be rejected");
        assert!(msg.contains("command"), "got: {msg}");
        assert!(msg.contains("function"), "got: {msg}");
    }

    #[test]
    fn add_test_rejects_non_string_lua_code() {
        let (lua, _capture_state) = make_lua_with_test_api();
        let res = lua.load(r#"
            cook.add_test({ lua_code = 42 })
        "#).exec();

        assert!(res.is_err(), "non-string lua_code must be rejected");
        assert!(format!("{:?}", res).contains("lua_code"), "got: {:?}", res);
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
