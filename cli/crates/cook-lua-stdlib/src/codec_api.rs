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
mod tests {
    use super::*;

    fn make_lua() -> Lua {
        let lua = Lua::new();
        let cook = lua.create_table().unwrap();
        register_codec_api(&lua, &cook).unwrap();
        lua.globals().set("cook", cook).unwrap();
        lua
    }

    #[test]
    fn test_json_decode_object() {
        let lua = make_lua();
        lua.load(r#"
            local t = cook.json_decode('{"name":"foo","version":1,"active":true,"items":[1,2,3]}')
            assert(t.name == "foo")
            assert(t.version == 1)
            assert(t.active == true)
            assert(t.items[1] == 1)
            assert(t.items[2] == 2)
            assert(t.items[3] == 3)
        "#)
        .exec()
        .unwrap();
    }

    #[test]
    fn test_json_decode_null() {
        let lua = make_lua();
        lua.load(r#"
            local t = cook.json_decode('{"a":null}')
            assert(t.a == nil)
        "#)
        .exec()
        .unwrap();
    }

    #[test]
    fn test_json_decode_nested() {
        let lua = make_lua();
        lua.load(r#"
            local t = cook.json_decode('{"scripts":{"build":"tsc","test":"jest"}}')
            assert(t.scripts.build == "tsc")
            assert(t.scripts.test == "jest")
        "#)
        .exec()
        .unwrap();
    }

    #[test]
    fn test_json_decode_error() {
        let lua = make_lua();
        let result = lua.load(r#"cook.json_decode("not json")"#).exec();
        assert!(result.is_err());
    }

    #[test]
    fn test_yaml_decode_workspace() {
        let lua = make_lua();
        lua.load(r#"
            local t = cook.yaml_decode([[
packages:
  - "packages/*"
catalog:
  typescript: "^5.4.0"
catalogs:
  internal:
    shared-utils: "workspace:*"
    ui: "workspace:*"
]])
            assert(t.packages[1] == "packages/*")
            assert(t.catalog.typescript == "^5.4.0")
            assert(t.catalogs.internal["shared-utils"] == "workspace:*")
            assert(t.catalogs.internal.ui == "workspace:*")
        "#)
        .exec()
        .unwrap();
    }

    #[test]
    fn test_yaml_decode_error() {
        let lua = make_lua();
        let result = lua
            .load(r#"cook.yaml_decode(":\n  :\n    - :")"#)
            .exec();
        assert!(result.is_err());
    }
}
