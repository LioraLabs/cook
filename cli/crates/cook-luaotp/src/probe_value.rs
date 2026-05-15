//! Lua→msgpack walker for the execute-phase probe dispatch (§22.5.4).
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
