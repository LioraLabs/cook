//! `cook.json_decode` / `cook.yaml_decode` — both-phase codecs (§24.8, CS-0123).
//!
//! Lives in cook-lua-stdlib so the register-phase VM (cook-register) and the
//! execute-phase worker VMs (cook-luaotp) install byte-identical behaviour.
use mlua::prelude::*;

/// Convert a serde_json::Value into a Lua value. JSON null maps to nil,
/// arrays to 1-indexed tables. Shared by the codecs and by cook-register's
/// module cache / export machinery.
///
/// Owned-value convenience over [`crate::json_codec::json_to_lua`] — the one
/// JSON→Lua walker. This wrapper's own body used to be a third independent
/// copy that had drifted: a number outside i64/f64 range silently became
/// `0.0` where the probe-path twins raise (COOK-388).
pub fn json_to_lua_value(lua: &Lua, val: serde_json::Value) -> LuaResult<LuaValue> {
    crate::json_codec::json_to_lua(lua, &val)
}

/// Register `cook.json_decode(str)` and `cook.yaml_decode(str)` on `cook`.
pub fn register_codec_api(lua: &Lua, cook: &LuaTable) -> LuaResult<()> {
    // cook.json_decode(json_string) -> lua table
    let json_decode = lua.create_function(|lua, s: String| {
        let val: serde_json::Value =
            serde_json::from_str(&s).map_err(|e| LuaError::runtime(format!("json error: {e}")))?;
        json_to_lua_value(lua, val)
    })?;
    cook.set("json_decode", json_decode)?;

    // cook.yaml_decode(yaml_string) -> lua table
    // Parse YAML into serde_json::Value (serde_yaml supports this) to reuse json_to_lua_value.
    let yaml_decode = lua.create_function(|lua, s: String| {
        let val: serde_json::Value = serde_yml::from_str(&s)
            .map_err(|e| LuaError::runtime(format!("yaml error: {e}")))?;
        json_to_lua_value(lua, val)
    })?;
    cook.set("yaml_decode", yaml_decode)?;

    Ok(())
}

#[cfg(test)]
#[path = "tests/codec_api_tests.rs"]
mod tests;
