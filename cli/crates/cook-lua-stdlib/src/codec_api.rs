//! `cook.json_decode` / `cook.yaml_decode` — both-phase codecs (§24.8, CS-0123).
//!
//! Lives in cook-lua-stdlib so the register-phase VM (cook-register) and the
//! execute-phase worker VMs (cook-luaotp) install byte-identical behaviour.
use mlua::prelude::*;

/// Convert a serde_json::Value into a Lua value. JSON null maps to nil,
/// arrays to 1-indexed tables. Shared by the codecs and by cook-register's
/// module cache / export machinery.
pub fn json_to_lua_value(lua: &Lua, val: serde_json::Value) -> LuaResult<LuaValue> {
    match val {
        serde_json::Value::Null => Ok(LuaValue::Nil),
        serde_json::Value::Bool(b) => Ok(LuaValue::Boolean(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LuaValue::Integer(i))
            } else {
                Ok(LuaValue::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(LuaValue::String(lua.create_string(&s)?)),
        serde_json::Value::Array(arr) => {
            let tbl = lua.create_table()?;
            for (i, v) in arr.into_iter().enumerate() {
                tbl.set(i + 1, json_to_lua_value(lua, v)?)?;
            }
            Ok(LuaValue::Table(tbl))
        }
        serde_json::Value::Object(map) => {
            let tbl = lua.create_table()?;
            for (k, v) in map {
                tbl.set(k, json_to_lua_value(lua, v)?)?;
            }
            Ok(LuaValue::Table(tbl))
        }
    }
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
