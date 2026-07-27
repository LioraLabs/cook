//! `cook.add_test` — removed (CS-0185).
//!
//! A test unit is an ordinary work unit, recorded by `cook.add_unit` with
//! `step_kind = "test"` and no declared outputs (§22.4). There is no separate
//! registration function, and this module exists only to make calling the one
//! that used to be here fail loudly.
//!
//! Why a raising stub rather than nothing: with the name unbound, a Cookfile or
//! module calling it gets Lua's `attempt to call a nil value (field 'add_test')`
//! — which reads as a Cook bug rather than as a removed API, and names no
//! replacement. The Standard requires the diagnostic to name one, in the manner
//! of §8.6's removed-modifier did-you-mean errors.
//!
//! Why not an alias: it carried nothing `cook.add_unit` lacked. `command`,
//! `lua_code`, `inputs`, `seal` and `line` were the same fields under the same
//! names — §22.4 documented two of them as "mirroring" and "exactly" mirroring
//! their `add_unit` counterparts — `iteration_item` duplicated `member`,
//! `timeout` and `should_fail` had been inert since CS-0135, and `suite`
//! captured nothing. A shim would have preserved exactly the ambiguity the
//! entry removes: two ways to record one thing, drifting apart the moment
//! either changed.

use mlua::prelude::*;

use crate::SharedBodySlot;

/// Bind `cook.add_test` to a function that raises.
///
/// The signature keeps `body_slot` so the call site in `engine.rs` is
/// unchanged; the slot is unused because the stub records nothing.
pub fn register_test_api(lua: &Lua, _body_slot: SharedBodySlot) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let removed = lua.create_function(|_, _: LuaMultiValue| -> LuaResult<()> {
        Err(LuaError::runtime(
            "cook.add_test was removed in v1.0 (Cook Standard \u{00a7}22.4, CS-0185). \
             A test unit is an ordinary work unit: record it with \
             cook.add_unit({ step_kind = \"test\", ... }) and declare no outputs. \
             The `suite` field was removed with it — a test unit belongs to the \
             recipe that registers it."
                .to_string(),
        ))
    })?;
    cook.set("add_test", removed)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/test_api_tests.rs"]
mod tests;
