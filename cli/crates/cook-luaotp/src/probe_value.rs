//! Lua↔msgpack and Lua↔JSON walkers for the execute-phase probe dispatch (§22.5.4/§22.5.5).
//!
//! This is a copy of `cook_register::probe_value::lua_to_msgpack`, kept here
//! so cook-luaotp does not depend on cook-register (which would make the
//! dependency graph more complex without benefit).
//!
//! `encode_msgpack` and `decode_msgpack` come from `cook_contracts::probe_value`
//! and are shared across all crates.

use mlua::prelude::*;
use rmpv::Value as MsgPackValue;

/// Convert a Lua value to msgpack. Validates the value-type contract (§22.5.4)
/// and rejects non-serialisable values with a path-tagged diagnostic.
pub fn lua_to_msgpack(v: &LuaValue) -> Result<MsgPackValue, String> {
    lua_to_msgpack_inner(v, &mut vec![], &mut vec![])
}

fn lua_to_msgpack_inner(
    v: &LuaValue,
    path: &mut Vec<String>,
    visited: &mut Vec<*const std::ffi::c_void>,
) -> Result<MsgPackValue, String> {
    match v {
        LuaValue::Nil => Ok(MsgPackValue::Nil),
        LuaValue::Boolean(b) => Ok(MsgPackValue::Boolean(*b)),
        LuaValue::Integer(i) => Ok(MsgPackValue::Integer((*i).into())),
        LuaValue::Number(f) => Ok(MsgPackValue::F64(*f)),
        LuaValue::String(s) => {
            let bytes = s.as_bytes().to_vec();
            match std::str::from_utf8(&bytes) {
                Ok(utf8) => Ok(MsgPackValue::String(rmpv::Utf8String::from(utf8.to_owned()))),
                Err(_) => Ok(MsgPackValue::Binary(bytes)),
            }
        }
        LuaValue::Table(t) => {
            let raw_ptr = t.to_pointer();
            if visited.contains(&raw_ptr) {
                return Err(format!(
                    "non-serialisable value at .{} (cycle)",
                    render_path(path)
                ));
            }
            visited.push(raw_ptr);
            let result = table_to_msgpack(t, path, visited);
            visited.pop();
            result
        }
        LuaValue::Function(_) => Err(format!(
            "non-serialisable value at .{} (function)",
            render_path(path)
        )),
        LuaValue::UserData(_) => Err(format!(
            "non-serialisable value at .{} (userdata)",
            render_path(path)
        )),
        LuaValue::Thread(_) => Err(format!(
            "non-serialisable value at .{} (thread)",
            render_path(path)
        )),
        LuaValue::Error(e) => Err(format!(
            "non-serialisable value at .{} (error: {})",
            render_path(path),
            e
        )),
        LuaValue::LightUserData(_) => Err(format!(
            "non-serialisable value at .{} (lightuserdata)",
            render_path(path)
        )),
        _ => Err(format!(
            "non-serialisable value at .{} (unknown variant)",
            render_path(path)
        )),
    }
}

fn table_to_msgpack(
    t: &LuaTable,
    path: &mut Vec<String>,
    visited: &mut Vec<*const std::ffi::c_void>,
) -> Result<MsgPackValue, String> {
    // First pass: classify keys.
    let mut int_keys: Vec<i64> = vec![];
    let mut str_keys: Vec<String> = vec![];
    let mut other_keys = 0usize;
    for pair in t.clone().pairs::<LuaValue, LuaValue>() {
        let (k, _) = pair.map_err(|e| {
            format!("table iteration failed at .{}: {}", render_path(path), e)
        })?;
        match k {
            LuaValue::Integer(i) => int_keys.push(i),
            LuaValue::String(s) => str_keys.push(s.to_string_lossy().to_owned()),
            _ => other_keys += 1,
        }
    }
    if other_keys > 0 {
        return Err(format!(
            "non-serialisable value at .{} (mixed/unsupported key types)",
            render_path(path)
        ));
    }
    if !int_keys.is_empty() && !str_keys.is_empty() {
        return Err(format!(
            "non-serialisable value at .{} (mixed string/integer keys not allowed; §22.5.4)",
            render_path(path)
        ));
    }

    if !int_keys.is_empty() {
        int_keys.sort();
        for (idx, k) in int_keys.iter().enumerate() {
            if *k != (idx as i64) + 1 {
                return Err(format!(
                    "non-serialisable value at .{}[{}] (array hole; not contiguous 1..N)",
                    render_path(path),
                    idx + 1
                ));
            }
        }
        let mut items = Vec::with_capacity(int_keys.len());
        for k in &int_keys {
            path.push(format!("[{}]", k));
            let v: LuaValue = t.get(*k).map_err(|e| format!("get failed: {}", e))?;
            let mv = lua_to_msgpack_inner(&v, path, visited)?;
            path.pop();
            items.push(mv);
        }
        Ok(MsgPackValue::Array(items))
    } else if !str_keys.is_empty() {
        str_keys.sort();
        let mut pairs = Vec::with_capacity(str_keys.len());
        for k in &str_keys {
            path.push(k.clone());
            let v: LuaValue = t.get(k.as_str()).map_err(|e| format!("get failed: {}", e))?;
            let mv = lua_to_msgpack_inner(&v, path, visited)?;
            path.pop();
            pairs.push((
                MsgPackValue::String(rmpv::Utf8String::from(k.as_str())),
                mv,
            ));
        }
        Ok(MsgPackValue::Map(pairs))
    } else {
        // Empty table — empty Map.
        Ok(MsgPackValue::Map(vec![]))
    }
}

fn render_path(path: &[String]) -> String {
    let mut s = String::new();
    for (i, seg) in path.iter().enumerate() {
        if seg.starts_with('[') {
            s.push_str(seg);
        } else {
            if i > 0 {
                s.push('.');
            }
            s.push_str(seg);
        }
    }
    s
}

/// Convert an rmpv::Value to a Lua value. Used by `cook.cache.get` on the
/// execute-phase VM to materialise probe values from the store (§22.5.7).
pub fn msgpack_to_lua(lua: &Lua, mp: &MsgPackValue) -> LuaResult<LuaValue> {
    use rmpv::Value as V;
    Ok(match mp {
        V::Nil => LuaValue::Nil,
        V::Boolean(b) => LuaValue::Boolean(*b),
        V::Integer(i) => match i.as_i64() {
            Some(n) => LuaValue::Integer(n),
            None => LuaValue::Number(i.as_f64().unwrap_or(0.0)),
        },
        V::F32(f) => LuaValue::Number(*f as f64),
        V::F64(f) => LuaValue::Number(*f),
        V::String(s) => {
            let bytes = s.as_bytes();
            LuaValue::String(lua.create_string(bytes)?)
        }
        V::Binary(bytes) => LuaValue::String(lua.create_string(bytes)?),
        V::Array(items) => {
            let t = lua.create_table()?;
            for (i, v) in items.iter().enumerate() {
                t.set(i + 1, msgpack_to_lua(lua, v)?)?;
            }
            LuaValue::Table(t)
        }
        V::Map(pairs) => {
            let t = lua.create_table()?;
            for (k, v) in pairs {
                let key_str = k.as_str().ok_or_else(|| {
                    LuaError::runtime("non-string map key in msgpack probe value")
                })?;
                t.set(key_str, msgpack_to_lua(lua, v)?)?;
            }
            LuaValue::Table(t)
        }
        V::Ext(_, _) => return Err(LuaError::runtime("msgpack ext type not supported")),
    })
}

// ---------------------------------------------------------------------------
// Lua ↔ JSON walkers (CS-0102 §22.5.5)
// ---------------------------------------------------------------------------
//
// These mirror lua_to_msgpack_inner / table_to_msgpack above, changing only
// the output constructors. The path-tracking and cycle-detection mechanics
// are byte-for-byte identical so that diagnostic messages match.
//
// Two rejections are NEW here (representable in msgpack, not JSON):
//   • non-UTF-8 Lua string → "... (non-UTF-8 string)" at path
//   • non-finite float     → "... (non-finite number)" at path

use serde_json::{Map as JsonMap, Value as JsonValue};

/// Convert a Lua value to JSON. Validates the §22.5.5 value contract,
/// reporting the offending path on failure (mirrors lua_to_msgpack, which
/// it replaces; CS-0102 additionally rejects non-UTF-8 strings and
/// non-finite numbers, which JSON cannot carry).
pub fn lua_to_json(v: &LuaValue) -> Result<JsonValue, String> {
    lua_to_json_inner(v, &mut vec![], &mut vec![])
}

fn lua_to_json_inner(
    v: &LuaValue,
    path: &mut Vec<String>,
    visited: &mut Vec<*const std::ffi::c_void>,
) -> Result<JsonValue, String> {
    match v {
        LuaValue::Nil => Ok(JsonValue::Null),
        LuaValue::Boolean(b) => Ok(JsonValue::Bool(*b)),
        LuaValue::Integer(i) => Ok(JsonValue::Number((*i).into())),
        LuaValue::Number(n) => {
            serde_json::Number::from_f64(*n)
                .map(JsonValue::Number)
                .ok_or_else(|| {
                    format!(
                        "non-serialisable value at .{} (non-finite number)",
                        render_path(path)
                    )
                })
        }
        LuaValue::String(s) => match s.to_str() {
            Ok(utf8) => Ok(JsonValue::String(utf8.to_owned())),
            Err(_) => Err(format!(
                "non-serialisable value at .{} (non-UTF-8 string)",
                render_path(path)
            )),
        },
        LuaValue::Table(t) => {
            let raw_ptr = t.to_pointer();
            if visited.contains(&raw_ptr) {
                return Err(format!(
                    "non-serialisable value at .{} (cycle)",
                    render_path(path)
                ));
            }
            visited.push(raw_ptr);
            let result = table_to_json(t, path, visited);
            visited.pop();
            result
        }
        LuaValue::Function(_) => Err(format!(
            "non-serialisable value at .{} (function)",
            render_path(path)
        )),
        LuaValue::UserData(_) => Err(format!(
            "non-serialisable value at .{} (userdata)",
            render_path(path)
        )),
        LuaValue::Thread(_) => Err(format!(
            "non-serialisable value at .{} (thread)",
            render_path(path)
        )),
        LuaValue::Error(e) => Err(format!(
            "non-serialisable value at .{} (error: {})",
            render_path(path),
            e
        )),
        LuaValue::LightUserData(_) => Err(format!(
            "non-serialisable value at .{} (lightuserdata)",
            render_path(path)
        )),
        _ => Err(format!(
            "non-serialisable value at .{} (unknown variant)",
            render_path(path)
        )),
    }
}

fn table_to_json(
    t: &LuaTable,
    path: &mut Vec<String>,
    visited: &mut Vec<*const std::ffi::c_void>,
) -> Result<JsonValue, String> {
    // First pass: classify keys.
    let mut int_keys: Vec<i64> = vec![];
    let mut str_keys: Vec<String> = vec![];
    let mut other_keys = 0usize;
    for pair in t.clone().pairs::<LuaValue, LuaValue>() {
        let (k, _) = pair.map_err(|e| {
            format!("table iteration failed at .{}: {}", render_path(path), e)
        })?;
        match k {
            LuaValue::Integer(i) => int_keys.push(i),
            LuaValue::String(s) => str_keys.push(s.to_string_lossy().to_owned()),
            _ => other_keys += 1,
        }
    }
    if other_keys > 0 {
        return Err(format!(
            "non-serialisable value at .{} (mixed/unsupported key types)",
            render_path(path)
        ));
    }
    if !int_keys.is_empty() && !str_keys.is_empty() {
        return Err(format!(
            "non-serialisable value at .{} (mixed string/integer keys not allowed; §22.5.4)",
            render_path(path)
        ));
    }

    if !int_keys.is_empty() {
        int_keys.sort();
        for (idx, k) in int_keys.iter().enumerate() {
            if *k != (idx as i64) + 1 {
                return Err(format!(
                    "non-serialisable value at .{}[{}] (array hole; not contiguous 1..N)",
                    render_path(path),
                    idx + 1
                ));
            }
        }
        let mut items = Vec::with_capacity(int_keys.len());
        for k in &int_keys {
            path.push(format!("[{}]", k));
            let v: LuaValue = t.get(*k).map_err(|e| format!("get failed: {}", e))?;
            let jv = lua_to_json_inner(&v, path, visited)?;
            path.pop();
            items.push(jv);
        }
        Ok(JsonValue::Array(items))
    } else if !str_keys.is_empty() {
        str_keys.sort();
        let mut map = JsonMap::new();
        for k in &str_keys {
            path.push(k.clone());
            let v: LuaValue = t.get(k.as_str()).map_err(|e| format!("get failed: {}", e))?;
            let jv = lua_to_json_inner(&v, path, visited)?;
            path.pop();
            map.insert(k.clone(), jv);
        }
        Ok(JsonValue::Object(map))
    } else {
        // Empty table — empty Object (mirrors empty-Map decision in msgpack walker).
        Ok(JsonValue::Object(JsonMap::new()))
    }
}

/// Convert a JSON value to a Lua value. Used by `cook.cache.get` on the
/// execute-phase VM to materialise probe values from the JSON store (§22.5.7).
/// Replaces msgpack_to_lua once all call sites have migrated to JSON (CS-0102).
pub fn json_to_lua(lua: &Lua, v: &JsonValue) -> LuaResult<LuaValue> {
    Ok(match v {
        JsonValue::Null => LuaValue::Nil,
        JsonValue::Bool(b) => LuaValue::Boolean(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                LuaValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                LuaValue::Number(f)
            } else {
                return Err(LuaError::runtime("JSON number out of range for Lua"));
            }
        }
        JsonValue::String(s) => LuaValue::String(lua.create_string(s.as_bytes())?),
        JsonValue::Array(items) => {
            let t = lua.create_table()?;
            for (i, item) in items.iter().enumerate() {
                t.set(i + 1, json_to_lua(lua, item)?)?;
            }
            LuaValue::Table(t)
        }
        JsonValue::Object(map) => {
            let t = lua.create_table()?;
            for (k, val) in map {
                t.set(k.as_str(), json_to_lua(lua, val)?)?;
            }
            LuaValue::Table(t)
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_to_json_round_trips_nested_table() {
        let lua = mlua::Lua::new();
        let v: mlua::Value = lua
            .load(r#"return { name = "ace", tags = {"a", "b"}, meta = { k = 2 } }"#)
            .eval()
            .unwrap();
        let json = lua_to_json(&v).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"name": "ace", "tags": ["a", "b"], "meta": {"k": 2}})
        );
        let back = json_to_lua(&lua, &json).unwrap();
        assert_eq!(lua_to_json(&back).unwrap(), json);
    }

    #[test]
    fn lua_to_json_empty_table_is_empty_object() {
        let lua = mlua::Lua::new();
        let v: mlua::Value = lua.load(r#"return {}"#).eval().unwrap();
        let json = lua_to_json(&v).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }

    #[test]
    fn lua_to_json_rejects_function_with_path() {
        let lua = mlua::Lua::new();
        let v: mlua::Value = lua
            .load(r#"return { cflags = { 'a', 'b', function() end } }"#)
            .eval()
            .unwrap();
        let err = lua_to_json(&v).unwrap_err();
        assert!(
            err.contains(".cflags[3]"),
            "error must name path .cflags[3]; got: {err}"
        );
        assert!(
            err.contains("function"),
            "error must mention 'function'; got: {err}"
        );
    }

    #[test]
    fn lua_to_json_rejects_non_utf8_string() {
        let lua = mlua::Lua::new();
        // Build a Lua string with raw non-UTF-8 bytes via string.char.
        let v: mlua::Value = lua
            .load(r#"return { blob = string.char(0xff, 0xfe) }"#)
            .eval()
            .unwrap();
        let err = lua_to_json(&v).unwrap_err();
        assert!(
            err.contains(".blob"),
            "error must name path .blob; got: {err}"
        );
        assert!(
            err.contains("non-UTF-8"),
            "error must mention 'non-UTF-8'; got: {err}"
        );
    }

    #[test]
    fn lua_to_json_rejects_non_finite_number() {
        let lua = mlua::Lua::new();
        let v: mlua::Value = lua
            .load(r#"return { x = 1/0 }"#)
            .eval()
            .unwrap();
        let err = lua_to_json(&v).unwrap_err();
        assert!(
            err.contains(".x"),
            "error must name path .x; got: {err}"
        );
        assert!(
            err.contains("non-finite"),
            "error must mention 'non-finite'; got: {err}"
        );
    }

    #[test]
    fn json_to_lua_null_is_nil() {
        let lua = mlua::Lua::new();
        let v = json_to_lua(&lua, &serde_json::Value::Null).unwrap();
        assert!(
            matches!(v, mlua::Value::Nil),
            "Null must become LuaValue::Nil"
        );
    }
}
