//! Lua↔JSON walkers for the register-VM probe paths (§22.5.5, CS-0102).
//!
//! This is an intentional in-crate copy of `cook_luaotp::probe_value`'s
//! `lua_to_json` / `json_to_lua`, kept here so cook-register does not depend
//! on cook-luaotp (which would make the dependency graph more complex without
//! benefit). The walkers cannot live in cook-contracts because that crate is
//! pure types with no mlua dependency.
//!
//! `encode_canonical_json` and `decode_json` are re-exported from
//! `cook_contracts::probe_value` so callers that import this module continue
//! to work without change.

use mlua::prelude::*;
use serde_json::{Map as JsonMap, Value as JsonValue};

// Re-export the encode/decode helpers from cook-contracts so callers can use
// either path interchangeably.
pub use cook_contracts::probe_value::{decode_json, encode_canonical_json};

/// Convert a Lua value to JSON. Validates the §22.5.5 value contract,
/// reporting the offending path on failure. CS-0102 rejects non-UTF-8
/// strings and non-finite numbers, which JSON cannot carry.
pub fn lua_to_json(v: &LuaValue) -> Result<JsonValue, String> {
    lua_to_json_inner(v, &mut vec![], &mut vec![])
}

/// Convert a decoded JSON probe value back into a Lua value on the register VM.
///
/// The inverse of [`lua_to_json`], used by the COOK-64 register pre-pass:
/// a member-source probe's value is decoded once and handed back to the
/// recipe body through `cook.probes.get`. Mirrors the worker-VM converter in
/// `cook-luaotp` (§22.5.7) — arrays become 1-based sequences, objects become
/// string-keyed tables, integers stay integers (falling back to float when
/// they overflow `i64`).
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
            "non-serialisable value at .{} (mixed string/integer keys not allowed)",
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
        // Empty table — empty Object (the canonical "no entries" shape).
        Ok(JsonValue::Object(JsonMap::new()))
    }
}

fn render_path(path: &[String]) -> String {
    // Render path segments separated by '.', with [N] kept attached.
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

#[cfg(test)]
#[path = "tests/probe_value_tests.rs"]
mod tests;
