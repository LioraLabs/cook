//! The `cook.member_to_string` door, driven from Lua rather than from Rust.
//!
//! Driving it from Lua is the point: what both VMs install is a Lua-callable
//! function, and the thing that used to be duplicated was the mlua wrapper,
//! not the pure renderer underneath it. A test that called
//! `cook_contracts::member::member_to_string` directly would pass with the
//! wrapper missing entirely.

use mlua::prelude::*;

fn vm() -> Lua {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    super::install_member_to_string(&lua, &cook).unwrap();
    lua.globals().set("cook", cook).unwrap();
    lua
}

fn render(lua: &Lua, expr: &str) -> String {
    lua.load(format!("return cook.member_to_string({expr})"))
        .eval::<String>()
        .unwrap()
}

/// §9.3: a table renders as compact key-sorted JSON, so a record's rendering
/// does not depend on the order the author wrote the fields in. That is
/// load-bearing rather than cosmetic — the string names the unit a fan-out
/// member registers, so an insertion-order-dependent rendering would file the
/// same member under two identities on two runs.
#[test]
fn a_table_renders_as_key_sorted_json_whatever_order_it_was_written_in() {
    let lua = vm();
    let one = render(&lua, r#"{ name = "b", id = 2 }"#);
    let other = render(&lua, r#"{ id = 2, name = "b" }"#);
    assert_eq!(one, other);
    assert_eq!(one, r#"{"id":2,"name":"b"}"#);
}

/// §9.3: a string scalar renders as its raw text, with no surrounding JSON
/// quotes, because it is going into a command line and a path.
#[test]
fn a_scalar_renders_bare() {
    let lua = vm();
    assert_eq!(render(&lua, r#""src/main.c""#), "src/main.c");
    assert_eq!(render(&lua, "42"), "42");
    assert_eq!(render(&lua, "true"), "true");
}

/// A value the walker refuses reaches the author as this door's diagnostic,
/// not as the walker's. One sentence, one author, both phases — it was
/// spelled character-for-character in each VM before COOK-439.
#[test]
fn a_value_the_walker_refuses_surfaces_as_this_doors_diagnostic() {
    let lua = vm();
    let err = lua
        .load("local t = {} t.self = t return cook.member_to_string(t)")
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("cook.member_to_string:"),
        "expected the door's own prefix, got: {err}"
    );
}

/// The door is installed under the shared constant, which is also what
/// `cook-luagen` emits. A rename that reached the installer and not the
/// emitter would be a generated `cook.member_to_string(item)` resolving to
/// nil — no compile error, no link error, a runtime failure in the user's
/// build with no hint of why.
#[test]
fn the_door_is_installed_under_the_shared_name() {
    let lua = vm();
    let present: bool = lua
        .load(format!(
            "return type(cook[{:?}]) == 'function'",
            cook_contracts::registration::MEMBER_TO_STRING_NAME
        ))
        .eval()
        .unwrap();
    assert!(present);
}
