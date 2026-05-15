//! Convert mlua::Value trees to rmpv::Value with value-type validation per §22.5.4.

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
        // Empty table — empty Map. msgpack distinguishes empty array vs empty
        // map; we pick map as the canonical "no entries" shape.
        Ok(MsgPackValue::Map(vec![]))
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

/// Encode an rmpv::Value to msgpack bytes.
pub fn encode_msgpack(v: &MsgPackValue) -> Vec<u8> {
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, v).expect("rmpv encode never fails for in-memory");
    buf
}

/// Decode msgpack bytes into an rmpv::Value.
pub fn decode_msgpack(bytes: &[u8]) -> Result<MsgPackValue, String> {
    let mut cursor = std::io::Cursor::new(bytes);
    rmpv::decode::read_value(&mut cursor).map_err(|e| format!("msgpack decode: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    fn convert(src: &str) -> Result<MsgPackValue, String> {
        let lua = Lua::new();
        let v: LuaValue = lua.load(src).eval().unwrap();
        lua_to_msgpack(&v)
    }

    #[test]
    fn converts_nil() {
        assert_eq!(convert("return nil").unwrap(), MsgPackValue::Nil);
    }

    #[test]
    fn converts_bool() {
        assert_eq!(convert("return true").unwrap(), MsgPackValue::Boolean(true));
        assert_eq!(
            convert("return false").unwrap(),
            MsgPackValue::Boolean(false)
        );
    }

    #[test]
    fn converts_number_int() {
        assert_eq!(
            convert("return 42").unwrap(),
            MsgPackValue::Integer(42.into())
        );
    }

    #[test]
    fn converts_number_float() {
        match convert("return 1.5").unwrap() {
            MsgPackValue::F64(f) => assert!((f - 1.5).abs() < 1e-9),
            other => panic!("expected F64, got {:?}", other),
        }
    }

    #[test]
    fn converts_string() {
        assert_eq!(
            convert("return \"hello\"").unwrap(),
            MsgPackValue::String("hello".into())
        );
    }

    #[test]
    fn converts_array_table() {
        let v = convert("return {1, 2, 3}").unwrap();
        match v {
            MsgPackValue::Array(items) => assert_eq!(items.len(), 3),
            other => panic!("expected Array, got {:?}", other),
        }
    }

    #[test]
    fn converts_string_keyed_table() {
        let v = convert("return { a = 1, b = 2 }").unwrap();
        match v {
            MsgPackValue::Map(pairs) => assert_eq!(pairs.len(), 2),
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn rejects_function() {
        let e = convert("return function() end").unwrap_err();
        assert!(e.contains("function"), "got: {}", e);
    }

    #[test]
    fn rejects_mixed_key_table() {
        let e = convert("return { [1] = 1, a = 2 }").unwrap_err();
        assert!(e.contains("mixed"), "got: {}", e);
    }

    #[test]
    fn rejects_array_with_holes() {
        let e = convert("return { [1] = \"a\", [3] = \"c\" }").unwrap_err();
        assert!(e.contains("hole") || e.contains("not contiguous"), "got: {}", e);
    }

    #[test]
    fn rejects_cyclic_table() {
        let lua = Lua::new();
        let v: LuaValue = lua
            .load(
                r#"
            local t = {}
            t.self = t
            return t
        "#,
            )
            .eval()
            .unwrap();
        let e = lua_to_msgpack(&v).unwrap_err();
        assert!(e.contains("cycle"), "got: {}", e);
    }

    // Task F3: msgpack round-trip tests

    #[test]
    fn msgpack_round_trip_simple_table() {
        let lua = Lua::new();
        let v: LuaValue = lua
            .load(
                r#"return { found = true, cflags = {"-I/usr/include"}, libs = {"-lz"} }"#,
            )
            .eval()
            .unwrap();
        let mp = lua_to_msgpack(&v).unwrap();
        let bytes = encode_msgpack(&mp);
        let back = decode_msgpack(&bytes).unwrap();
        assert_eq!(back, mp);
    }

    #[test]
    fn msgpack_round_trip_nested_table() {
        let lua = Lua::new();
        let v: LuaValue = lua
            .load(r#"return { a = { b = { c = 42 } } }"#)
            .eval()
            .unwrap();
        let mp = lua_to_msgpack(&v).unwrap();
        let bytes = encode_msgpack(&mp);
        let back = decode_msgpack(&bytes).unwrap();
        assert_eq!(back, mp);
    }

    #[test]
    fn msgpack_round_trip_primitives() {
        let lua = Lua::new();
        for src in [
            "return nil",
            "return true",
            "return 42",
            "return 1.5",
            "return \"hello\"",
            "return {}",
        ] {
            let v: LuaValue = lua.load(src).eval().unwrap();
            let mp = lua_to_msgpack(&v).unwrap();
            let bytes = encode_msgpack(&mp);
            let back = decode_msgpack(&bytes).unwrap();
            assert_eq!(back, mp, "round-trip failed for source: {}", src);
        }
    }
}
