use super::*;
use mlua::Lua;
use serde_json::json;

fn convert(src: &str) -> Result<JsonValue, String> {
    let lua = Lua::new();
    let v: LuaValue = lua.load(src).eval().unwrap();
    lua_to_json(&v)
}

#[test]
fn converts_nil() {
    assert_eq!(convert("return nil").unwrap(), JsonValue::Null);
}

// COOK-64 §9.3: the exact composition `cook.member_to_string` binds —
// a real Lua value through `lua_to_json` then `member::member_to_string`.
#[test]
fn member_to_string_renders_record_and_scalar() {
    let rec = convert("return { name = 'ace', id = 1 }").unwrap();
    assert_eq!(
        cook_contracts::member::member_to_string(&rec),
        r#"{"id":1,"name":"ace"}"#
    );
    let scalar = convert("return 'hi'").unwrap();
    assert_eq!(cook_contracts::member::member_to_string(&scalar), "hi");
}

#[test]
fn converts_bool() {
    assert_eq!(convert("return true").unwrap(), json!(true));
    assert_eq!(convert("return false").unwrap(), json!(false));
}

#[test]
fn converts_number_int() {
    assert_eq!(convert("return 42").unwrap(), json!(42));
}

#[test]
fn converts_number_float() {
    match convert("return 1.5").unwrap() {
        JsonValue::Number(n) => assert!((n.as_f64().unwrap() - 1.5).abs() < 1e-9),
        other => panic!("expected Number, got {:?}", other),
    }
}

#[test]
fn converts_string() {
    assert_eq!(convert("return \"hello\"").unwrap(), json!("hello"));
}

#[test]
fn converts_array_table() {
    let v = convert("return {1, 2, 3}").unwrap();
    match v {
        JsonValue::Array(items) => assert_eq!(items.len(), 3),
        other => panic!("expected Array, got {:?}", other),
    }
}

#[test]
fn converts_string_keyed_table() {
    let v = convert("return { a = 1, b = 2 }").unwrap();
    match v {
        JsonValue::Object(map) => assert_eq!(map.len(), 2),
        other => panic!("expected Object, got {:?}", other),
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
    let e = lua_to_json(&v).unwrap_err();
    assert!(e.contains("cycle"), "got: {}", e);
}

// CS-0102: non-UTF-8 strings are no longer legal probe values (the
// pre-CS-0102 binary-string escape hatch died with the JSON encoding).
#[test]
fn rejects_non_utf8_string() {
    let lua = Lua::new();
    let v: LuaValue = lua
        .load("return { blob = string.char(0xFF, 0xFE, 0x00, 0x01) }")
        .eval()
        .unwrap();
    let e = lua_to_json(&v).unwrap_err();
    assert!(e.contains(".blob"), "error must name path .blob; got: {}", e);
    assert!(e.contains("non-UTF-8"), "got: {}", e);
}

// CS-0102: numbers must be finite.
#[test]
fn rejects_non_finite_number() {
    let e = convert("return { x = 1/0 }").unwrap_err();
    assert!(e.contains(".x"), "error must name path .x; got: {}", e);
    assert!(e.contains("non-finite"), "got: {}", e);
}

// Canonical-JSON round-trip tests.

#[test]
fn json_round_trip_simple_table() {
    let lua = Lua::new();
    let v: LuaValue = lua
        .load(
            r#"return { found = true, cflags = {"-I/usr/include"}, libs = {"-lz"} }"#,
        )
        .eval()
        .unwrap();
    let jv = lua_to_json(&v).unwrap();
    let bytes = encode_canonical_json(&jv);
    let back = decode_json(&bytes).unwrap();
    assert_eq!(back, jv);
}

#[test]
fn json_round_trip_nested_table() {
    let lua = Lua::new();
    let v: LuaValue = lua
        .load(r#"return { a = { b = { c = 42 } } }"#)
        .eval()
        .unwrap();
    let jv = lua_to_json(&v).unwrap();
    let bytes = encode_canonical_json(&jv);
    let back = decode_json(&bytes).unwrap();
    assert_eq!(back, jv);
}

#[test]
fn json_round_trip_primitives() {
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
        let jv = lua_to_json(&v).unwrap();
        let bytes = encode_canonical_json(&jv);
        let back = decode_json(&bytes).unwrap();
        assert_eq!(back, jv, "round-trip failed for source: {}", src);
    }
}
