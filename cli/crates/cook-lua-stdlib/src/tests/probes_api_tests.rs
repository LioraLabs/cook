//! `cook.probes` built over two recording closures.
//!
//! The store is a toy on purpose. What is under test is the part both VMs
//! share — the table's shape, the scope view, the label rule, and the key
//! each operation is finally handed — not what either phase's real store does
//! with it.

use mlua::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Every key `get` and `set` were called with, in call order.
type Calls = Rc<RefCell<Vec<String>>>;

/// A VM whose `set` records and writes, i.e. the register-phase shape.
fn writing_vm() -> (Lua, Calls, Calls) {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    let gets: Calls = Rc::new(RefCell::new(Vec::new()));
    let sets: Calls = Rc::new(RefCell::new(Vec::new()));
    let gets_in = Rc::clone(&gets);
    let sets_in = Rc::clone(&sets);
    super::install_probes_api(
        &lua,
        &cook,
        move |lua, key: &str| {
            gets_in.borrow_mut().push(key.to_string());
            Ok(LuaValue::String(lua.create_string(key)?))
        },
        move |_lua, key: &str, _value: &LuaValue| {
            sets_in.borrow_mut().push(key.to_string());
            Ok(())
        },
    )
    .unwrap();
    lua.globals().set("cook", cook).unwrap();
    (lua, gets, sets)
}

/// A VM whose `set` refuses, i.e. the CS-0074 execute-phase shape.
fn refusing_vm() -> Lua {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    super::install_probes_api(
        &lua,
        &cook,
        |_lua, _key: &str| Ok(LuaValue::Nil),
        |_lua, _key: &str, _value: &LuaValue| {
            Err(LuaError::runtime("this phase does not write probe values"))
        },
    )
    .unwrap();
    lua.globals().set("cook", cook).unwrap();
    lua
}

/// The unscoped pair reaches the phase's own store with the key verbatim.
#[test]
fn get_and_set_reach_the_phases_store_with_the_key_verbatim() {
    let (lua, gets, sets) = writing_vm();
    let got: String = lua.load(r#"return cook.probes.get("cc:version")"#).eval().unwrap();
    assert_eq!(got, "cc:version");
    lua.load(r#"cook.probes.set("cc:version", 1)"#).exec().unwrap();
    assert_eq!(*gets.borrow(), vec!["cc:version".to_string()]);
    assert_eq!(*sets.borrow(), vec!["cc:version".to_string()]);
}

/// §24.4.3: a scope view prefixes `label:`, and the store never learns that
/// scopes exist. This is the half of the copy that had to keep agreeing —
/// a prefix format fixed in one VM and not the other files the same probe
/// under two keys.
#[test]
fn a_scope_view_prefixes_the_label_on_both_operations() {
    let (lua, gets, sets) = writing_vm();
    lua.load(
        r#"
        local s = cook.probes.scope("cc")
        s.get("version")
        s.set("version", 1)
    "#,
    )
    .exec()
    .unwrap();
    assert_eq!(*gets.borrow(), vec!["cc:version".to_string()]);
    assert_eq!(*sets.borrow(), vec!["cc:version".to_string()]);
}

/// The label rule is the shared §24.4.3 one, applied once here rather than
/// once per VM, and it fires before the store is consulted.
#[test]
fn an_invalid_scope_label_raises_the_shared_rules_own_sentence() {
    let (lua, gets, sets) = writing_vm();
    // A label carrying the scope separator itself: the one thing §24.4.3
    // rejects, because `scoped_key` would produce a key nothing could parse
    // back.
    let expected = cook_contracts::probe_key::scope_label_error("cc:extra")
        .expect("a label containing ':' must be rejected by the shared rule");
    let err = lua
        .load(r#"return cook.probes.scope("cc:extra")"#)
        .eval::<LuaValue>()
        .unwrap_err()
        .to_string();
    assert!(err.contains(&expected), "got: {err}");
    assert!(gets.borrow().is_empty() && sets.borrow().is_empty());
}

/// The one thing that genuinely differs by phase is an argument, so a phase
/// that refuses writes refuses them identically through the scoped and the
/// unscoped door. Both used to be spelled separately, which is how one of
/// them could have kept working after the other was withdrawn.
#[test]
fn a_refusing_set_refuses_through_both_the_scoped_and_the_unscoped_door() {
    let lua = refusing_vm();
    for program in [
        r#"cook.probes.set("k", 1)"#,
        r#"cook.probes.scope("s").set("k", 1)"#,
    ] {
        let err = lua.load(program).exec().unwrap_err().to_string();
        assert!(
            err.contains("this phase does not write probe values"),
            "{program} gave: {err}"
        );
    }
}

/// CS-0136: the `cook.cache` rename stub belongs to this install, not to each
/// caller's memory of making it. Both VMs used to call it separately right
/// after building the table, which is a step a third caller could omit.
#[test]
fn the_renamed_cache_stub_comes_with_the_table() {
    let lua = refusing_vm();
    let err = lua
        .load("return cook.cache.get")
        .eval::<LuaValue>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("cook.probes"), "got: {err}");
}
