//! The read-only `var` global's seal, exercised through Lua.
//!
//! Each of the three guards is tested separately, because each one alone is
//! the whole seal: reads that bypass `__index` skip the declared-name check,
//! a writable `var` lets a step redefine a determinant it was keyed on, and a
//! visible metatable makes both of those one `setmetatable` call away.

use mlua::prelude::*;

/// A proxy over a fixed store, with a refusal sentence a test can recognise.
fn vm() -> Lua {
    let lua = Lua::new();
    super::install_var_proxy(
        &lua,
        |lua, name: String| match name.as_str() {
            "TARGET" => Ok(LuaValue::String(lua.create_string("release")?)),
            other => Err(LuaError::runtime(format!("undeclared: {other}"))),
        },
        |name| format!("var.{name} is refused by this phase"),
    )
    .unwrap();
    lua
}

/// Reads route through `__index`, so whatever check the phase installs runs
/// on every access rather than on the first one.
#[test]
fn a_read_routes_through_the_index_metamethod() {
    let lua = vm();
    let value: String = lua.load("return var.TARGET").eval().unwrap();
    assert_eq!(value, "release");

    let err = lua
        .load("return var.NOPE")
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("undeclared: NOPE"), "got: {err}");
}

/// A write is refused with the CALLER's sentence. The seal is shared; the
/// wording is not, because register phase can point at a `config` block and
/// execute phase cannot.
#[test]
fn a_write_is_refused_with_the_callers_own_sentence() {
    let lua = vm();
    let err = lua
        .load(r#"var.TARGET = "debug""#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("var.TARGET is refused by this phase"),
        "got: {err}"
    );
}

/// The metatable is hidden, so the two guards above cannot be lifted off.
/// Without `__metatable = false` the whole seal is `setmetatable(var, {})`,
/// and every test above would still pass.
#[test]
fn the_metatable_cannot_be_read_or_replaced() {
    let lua = vm();
    let hidden: bool = lua
        .load("return getmetatable(var) == false")
        .eval()
        .unwrap();
    assert!(hidden, "getmetatable(var) must return the false sentinel");

    let err = lua
        .load("setmetatable(var, {})")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("protected metatable"), "got: {err}");
}

/// The global's name comes from one constant. `cook-luagen` lowers a
/// `$<NAME>` read into `var.NAME`, so a rename that reached one VM and not
/// the emitter is a generated program indexing a nil global.
#[test]
fn the_proxy_is_installed_under_the_shared_global_name() {
    let lua = vm();
    let present: bool = lua
        .load(format!(
            "return type(_G[{:?}]) == 'table'",
            super::VAR_GLOBAL_NAME
        ))
        .eval()
        .unwrap();
    assert!(present);
}
