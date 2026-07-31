//! THE Lua↔JSON codec (§22.5.5, CS-0102) — single home, both phases.
//!
//! One validating `lua_to_json` and one `json_to_lua`, shared by the
//! register-phase VM (cook-register: probe pre-pass, `cook.export`,
//! module-context `cook.probes.set`) and every execute-phase worker VM
//! (cook-luaotp: probe dispatch, `cook.probes.get`). The two phases
//! serialize probe values into the SAME store and the SAME
//! `seal_contribution` fingerprint, so they must agree byte-for-byte;
//! before COOK-388 they were manually-synchronized copies in
//! `cook_register::probe_value` and `cook_luaotp::probe_value`, and a
//! third, weaker walker on the module-export path silently dropped what
//! this one rejects.
//!
//! Why here and not the homes the old copy headers rejected: the copies
//! predate CS-0123 (2026-07-06) and argued only against cook-contracts,
//! which is pure plain-data types and carries no mlua dependency — that
//! rejection stands. cook-lua-stdlib was never considered; it is the
//! mlua-bearing shared crate BOTH consumers already depend on, which is
//! exactly the stratum rule's answer for mlua-bearing shared logic
//! (cook-contracts/README.md).
//!
//! `encode_canonical_json` / `decode_json` remain in
//! `cook_contracts::probe_value` (pure, no mlua) and are not duplicated
//! here.

use mlua::prelude::*;
use serde_json::{Map as JsonMap, Value as JsonValue};

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

// ---------------------------------------------------------------------------
// Lua ↔ JSON walkers (CS-0102 §22.5.5)
// ---------------------------------------------------------------------------
//
// Two rejections are specific to the JSON value contract (CS-0102):
//   • non-UTF-8 Lua string → "... (non-UTF-8 string)" at path
//   • non-finite float     → "... (non-finite number)" at path

/// Convert a Lua value to JSON. Validates the §22.5.5 value contract,
/// reporting the offending path on failure. CS-0102 rejects non-UTF-8
/// strings and non-finite numbers, which JSON cannot carry.
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

/// Convert a JSON value to a Lua value. Used by `cook.probes.get` on both
/// VMs to materialise probe values from the JSON store (§22.5.7, CS-0102),
/// and by `cook.import` / `cook.json_decode` / `cook.yaml_decode`.
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
#[path = "tests/json_codec_tests.rs"]
mod tests;
