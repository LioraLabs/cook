use super::*;
use cook_contracts::{DepKind, WorkPayload};
use std::cell::RefCell;
use std::rc::Rc;
use crate::BodyCaptureState;

/// The declared input paths of a captured unit, kinds dropped: these cases are
/// about WHICH paths a declaration carries, not how each is read.
fn declared_inputs(unit: &cook_contracts::CapturedUnit) -> Vec<String> {
    unit.cache_meta
        .as_ref()
        .expect("a test unit carries cache metadata (CS-0186)")
        .inputs
        .iter()
        .map(|e| e.path.clone())
        .collect()
}

fn body_ref(body_slot: &SharedBodySlot) -> std::cell::Ref<'_, BodyCaptureState> {
    std::cell::Ref::map(body_slot.borrow(), |slot| {
        slot.as_ref().expect("body slot populated for test")
    })
}

/// CS-0185: these cases were written against `cook.add_test` and now exercise
/// the one registration function that replaced it. The harness registers
/// `cook.add_unit` under the recipe name the suite assertions already expected,
/// and `register_test_api` too — which now only binds the raising stub, so the
/// removal itself stays covered here alongside the behaviour it replaced.
fn make_lua_with_test_api() -> (Lua, SharedBodySlot) {
    use std::sync::{Arc, Mutex};
    use std::collections::BTreeMap;
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    let body_slot: SharedBodySlot =
        Rc::new(RefCell::new(Some(BodyCaptureState::new())));
    let terminal_outputs: crate::SharedTerminalOutputs = Arc::new(Mutex::new(BTreeMap::new()));
    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    crate::unit_api::register_unit_api(
        &lua,
        body_slot.clone(),
        "unit",
        terminal_outputs,
        working_dir,
    )
    .unwrap();
    register_test_api(&lua, body_slot.clone()).unwrap();
    body_slot.borrow_mut().as_mut().unwrap().current_recipe = Some("unit".to_string());
    (lua, body_slot)
}

#[test]
fn test_add_test_basic() {
    let (lua, capture_state) = make_lua_with_test_api();
    lua.load(r#"
            cook.add_unit({step_kind = "test", 
                command = "./run_tests",
            })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    match &state.units[0].payload {
        // CS-0191: a command test is a Shell unit. `timeout` and `should_fail`
        // are no longer asserted because they no longer exist: both were
        // hardcoded at the one construction site since CS-0135 removed the
        // modifiers that set them, so the only thing those assertions pinned
        // was that a constant was still a constant.
        WorkPayload::Shell { cmd, .. } => assert_eq!(cmd, "./run_tests"),
        other => panic!("expected a Shell payload, got {other:?}"),
    }
    // No current_recipe in this harness — the derived `<recipe>_test<N>` label
    // degrades to a bare ordinal.
    assert_eq!(
        state.units[0].test_name.as_deref(),
        Some("unit_test0"), // CS-0185: <recipe>_test<line>
    );
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
            cook.add_unit({step_kind = "test", 
                command = "./check",
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
            cook.add_unit({step_kind = "test", 
                command = "./test",
            })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    match &state.units[0].payload {
        WorkPayload::Shell { .. } => {}
        other => panic!("expected a Shell payload, got {other:?}"),
    }
    assert_eq!(state.units[0].test_name.as_deref(), Some("unit_test0"));
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
            cook.add_unit({step_kind = "test",  command = "true" })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    // CS-0185: `suite` is gone; the enclosing recipe — qualified prefix and
    // all — reaches the unit through its derived NAME instead. CS-0191 moves
    // that name off the payload and onto the unit.
    assert_eq!(
        state.units[0].test_name.as_deref(),
        Some("frontend.unit_test0")
    );
}

#[test]
fn test_unit_name_derives_from_recipe_and_line() {
    let (lua, capture_state) = make_lua_with_test_api();
    capture_state
        .borrow_mut()
        .as_mut()
        .expect("body slot populated for test")
        .current_recipe = Some("rust-test".to_string());

    lua.load(r#"
            cook.add_unit({step_kind = "test",  command = "true",  line = 4 })
            cook.add_unit({step_kind = "test",  command = "false", line = 7 })
        "#).exec().unwrap();

    let state = body_ref(&capture_state);
    let names: Vec<&str> = state
        .units
        .iter()
        .map(|u| u.test_name.as_deref().expect("a test unit carries its name"))
        .collect();
    // CS-0185: the discriminator is the LINE, not an ordinal. Both are unique
    // within a recipe — two test steps cannot share a line — but a line needs
    // no counting over the units already recorded.
    assert_eq!(names, ["rust-test_test4", "rust-test_test7"]);
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
        cook.add_unit({step_kind = "test",  command = "" })
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
        cook.add_unit({step_kind = "test",  name = "x" })
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
            cook.add_unit({step_kind = "test", 
                command = "cargo test",
                name = "t",
                inputs = { "src/lib.rs", "src/main.rs" },
            })
        "#).exec().unwrap();
    let state = body_ref(&capture_state);
    // CS-0186: a test unit's declared inputs live on its `CacheMeta`, where
    // every other unit's do, and are read from there by the one cache path.
    let inputs = declared_inputs(&state.units[0]);
    assert_eq!(inputs, vec!["src/lib.rs", "src/main.rs"]);
}

#[test]
fn add_test_unions_step_group_dep_input_paths() {
    let (lua, capture_state) = make_lua_with_test_api();
    capture_state.borrow_mut().as_mut().expect("body slot populated for test")
        .step_group_dep_input_paths
        .extend(["../core/build/core.so".to_string(), "src/lib.rs".to_string()]);
    lua.load(r#"
            cook.add_unit({step_kind = "test", 
                command = "pytest",
                name = "t",
                inputs = { "src/lib.rs" },
            })
        "#).exec().unwrap();
    let state = body_ref(&capture_state);
    // union, deduped, declared inputs first
    assert_eq!(
        declared_inputs(&state.units[0]),
        vec!["src/lib.rs", "../core/build/core.so"]
    );
}

#[test]
fn add_test_without_inputs_still_carries_dep_paths() {
    let (lua, capture_state) = make_lua_with_test_api();
    capture_state.borrow_mut().as_mut().expect("body slot populated for test")
        .step_group_dep_input_paths
        .push("build/lib.txt".to_string());
    lua.load(r#"
            cook.add_unit({step_kind = "test",  command = "true", name = "t" })
        "#).exec().unwrap();
    let state = body_ref(&capture_state);
    assert_eq!(declared_inputs(&state.units[0]), vec!["build/lib.txt"]);
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
        cook.add_unit({step_kind = "test",  lua_code = "assert(true)", name = "t" })
    "#).exec().unwrap();

    let state = body_ref(&capture_state);
    assert_eq!(state.units.len(), 1);
    match &state.units[0].payload {
        // CS-0191: a Lua-bodied test is a LuaChunk, exactly as a Lua-bodied
        // `cook` step is. The old assertion — `lua_code` set and `cmd` empty —
        // described one payload carrying both possibilities; now the payload
        // itself is the answer.
        WorkPayload::LuaChunk { code, outputs, .. } => {
            assert_eq!(code, "assert(true)");
            // A test declares no outputs; that is what makes it observing.
            assert!(outputs.is_empty());
        }
        other => panic!("expected a LuaChunk payload, got {other:?}"),
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
        cook.add_unit({step_kind = "test",  command = "true", lua_code = "", name = "t" })
    "#).exec().unwrap();

    let state = body_ref(&capture_state);
    match &state.units[0].payload {
        // An empty `lua_code` reads as absent, so this is a command test and
        // therefore a Shell payload.
        WorkPayload::Shell { cmd, .. } => assert_eq!(cmd, "true"),
        other => panic!("expected a Shell payload, got {other:?}"),
    }
}

#[test]
fn add_test_rejects_both_command_and_lua_code() {
    let (lua, _capture_state) = make_lua_with_test_api();
    let res = lua.load(r#"
            cook.add_unit({step_kind = "test",  command = "true", lua_code = "assert(true)" })
        "#).exec();

    assert!(res.is_err(), "both command and lua_code must be rejected");
    assert!(format!("{:?}", res).contains("exactly one"), "got: {:?}", res);
}

#[test]
fn add_test_rejects_non_string_command() {
    let (lua, _capture_state) = make_lua_with_test_api();
    let res = lua.load(r#"
            cook.add_unit({step_kind = "test",  command = function() end })
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
            cook.add_unit({step_kind = "test",  lua_code = 42 })
        "#).exec();

    assert!(res.is_err(), "non-string lua_code must be rejected");
    assert!(format!("{:?}", res).contains("lua_code"), "got: {:?}", res);
}
// CS-0185: `suite` is removed, so there is no override left to test. Passing
// it is refused — pinned by `a_test_unit_may_not_declare_a_suite` in the
// unit_api tests, next to the other §22.4 refusals.
