use super::*;

fn setup() -> (Lua, SharedFinalizerQueue) {
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    let queue: SharedFinalizerQueue = Rc::new(RefCell::new(Vec::new()));
    register_on_register_complete(&lua, queue.clone()).unwrap();
    (lua, queue)
}

#[test]
fn queues_a_function_without_running_it() {
    let (lua, queue) = setup();
    lua.load(r#"cook.on_register_complete(function() error("must not run") end)"#)
        .exec()
        .unwrap();
    assert_eq!(queue.borrow().len(), 1, "callback should be queued, not run");
}

#[test]
fn queues_multiple_calls_in_order() {
    let (lua, queue) = setup();
    lua.load(
        r#"
cook.on_register_complete(function() end)
cook.on_register_complete(function() end)
cook.on_register_complete(function() end)
"#,
    )
    .exec()
    .unwrap();
    assert_eq!(queue.borrow().len(), 3);
}

#[test]
fn rejects_number() {
    let (lua, _queue) = setup();
    let err = lua
        .load("cook.on_register_complete(42)")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("cook.on_register_complete"), "got: {err}");
    assert!(err.contains("function"), "got: {err}");
    // mlua (Lua 5.4) distinguishes the integer/float subtypes in
    // `type_name()` even though Lua itself reports both as `"number"`;
        // a bare integer literal like `42` is an mlua `Integer`.
        assert!(err.contains("integer"), "got: {err}");
    assert!(err.contains("22.9"), "got: {err}");
    assert!(err.contains("CS-0149"), "got: {err}");
}

#[test]
fn rejects_string() {
    let (lua, _queue) = setup();
    let err = lua
        .load(r#"cook.on_register_complete("nope")"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("cook.on_register_complete"), "got: {err}");
    assert!(err.contains("string"), "got: {err}");
}

#[test]
fn rejects_nil() {
    let (lua, _queue) = setup();
    let err = lua
        .load("cook.on_register_complete(nil)")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("cook.on_register_complete"), "got: {err}");
    assert!(err.contains("nil"), "got: {err}");
}

#[test]
fn rejects_table() {
    let (lua, _queue) = setup();
    let err = lua
        .load("cook.on_register_complete({})")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("cook.on_register_complete"), "got: {err}");
    assert!(err.contains("table"), "got: {err}");
}
