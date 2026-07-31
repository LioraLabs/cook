//! The one codec's test suite — the union of the two suites that guarded
//! the former per-crate copies (cook-register and cook-luaotp,
//! `probe_value_tests.rs` both), plus the COOK-388 agreement pin.

use super::*;
use mlua::Lua;
use serde_json::json;

fn convert(src: &str) -> Result<JsonValue, String> {
    let lua = Lua::new();
    let v: LuaValue = lua.load(src).eval().unwrap();
    lua_to_json(&v)
}

// ---------------------------------------------------------------------------
// Lua → JSON conversions
// ---------------------------------------------------------------------------

#[test]
fn converts_nil() {
    assert_eq!(convert("return nil").unwrap(), JsonValue::Null);
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
fn empty_table_is_empty_object() {
    assert_eq!(convert("return {}").unwrap(), json!({}));
}

#[test]
fn round_trips_nested_table() {
    let lua = Lua::new();
    let v: LuaValue = lua
        .load(r#"return { name = "ace", tags = {"a", "b"}, meta = { k = 2 } }"#)
        .eval()
        .unwrap();
    let json = lua_to_json(&v).unwrap();
    assert_eq!(
        json,
        json!({"name": "ace", "tags": ["a", "b"], "meta": {"k": 2}})
    );
    let back = json_to_lua(&lua, &json).unwrap();
    assert_eq!(lua_to_json(&back).unwrap(), json);
}

// ---------------------------------------------------------------------------
// §22.5.5 / CS-0102 rejections — every one names the offending path
// ---------------------------------------------------------------------------

#[test]
fn rejects_function() {
    let e = convert("return function() end").unwrap_err();
    assert!(e.contains("function"), "got: {}", e);
}

#[test]
fn rejects_function_with_path() {
    let e = convert(r#"return { cflags = { 'a', 'b', function() end } }"#).unwrap_err();
    assert!(
        e.contains(".cflags[3]"),
        "error must name path .cflags[3]; got: {e}"
    );
    assert!(e.contains("function"), "error must mention 'function'; got: {e}");
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

// ---------------------------------------------------------------------------
// JSON → Lua
// ---------------------------------------------------------------------------

#[test]
fn json_to_lua_null_is_nil() {
    let lua = Lua::new();
    let v = json_to_lua(&lua, &JsonValue::Null).unwrap();
    assert!(matches!(v, LuaValue::Nil), "Null must become LuaValue::Nil");
}

/// The codec_api wrapper (`json_to_lua_value`) is the same walker: a number
/// outside i64/f64 range raises rather than silently becoming `0.0` (the
/// pre-COOK-388 drift in the third copy).
#[test]
fn out_of_range_number_raises_not_zero() {
    let lua = Lua::new();
    // A number JSON can carry but Lua cannot: u64 above i64::MAX parses to
    // serde_json's u64 arm, where as_i64() is None and as_f64() is Some —
    // so this is representable and does NOT raise; the raise arm needs a
    // number where BOTH conversions fail, which serde_json's arbitrary
    // precision feature is off for. What the test CAN pin is that the u64
    // arm converts by f64 (the twins' behavior), not to 0.0.
    let big: JsonValue = serde_json::from_str("18446744073709551615").unwrap();
    match crate::codec_api::json_to_lua_value(&lua, big).unwrap() {
        LuaValue::Number(f) => assert!(f > 1.0e19, "must convert via f64, not 0.0; got {f}"),
        other => panic!("expected Lua float, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Canonical-JSON round trips (via cook-contracts' encoder — the store format)
// ---------------------------------------------------------------------------

#[test]
fn json_round_trip_simple_table() {
    let lua = Lua::new();
    let v: LuaValue = lua
        .load(r#"return { found = true, cflags = {"-I/usr/include"}, libs = {"-lz"} }"#)
        .eval()
        .unwrap();
    let jv = lua_to_json(&v).unwrap();
    let bytes = cook_contracts::probe_value::encode_canonical_json(&jv);
    let back = cook_contracts::probe_value::decode_json(&bytes).unwrap();
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
        let bytes = cook_contracts::probe_value::encode_canonical_json(&jv);
        let back = cook_contracts::probe_value::decode_json(&bytes).unwrap();
        assert_eq!(back, jv, "round-trip failed for source: {}", src);
    }
}

/// Float/integer identity must survive the full encode/decode round trip:
/// a Lua float `1.0` stays a float (renders `1.0`, decodes back to f64),
/// and a Lua integer `1` stays an integer. Conflating them would change
/// canonical bytes and re-key cache entries.
#[test]
fn float_identity_round_trips() {
    let lua = Lua::new();

    let float_v: LuaValue = lua.load("return 1.0").eval().unwrap();
    let float_json = lua_to_json(&float_v).unwrap();
    assert!(float_json.is_f64(), "Lua float 1.0 must map to a JSON float");
    let float_bytes = cook_contracts::probe_value::encode_canonical_json(&float_json);
    assert_eq!(float_bytes, b"1.0\n");
    let float_back = cook_contracts::probe_value::decode_json(&float_bytes).unwrap();
    assert!(float_back.is_f64(), "decoded 1.0 must stay a float");
    match json_to_lua(&lua, &float_back).unwrap() {
        LuaValue::Number(f) => assert_eq!(f, 1.0),
        other => panic!("expected Lua float, got {other:?}"),
    }

    let int_v: LuaValue = lua.load("return 1").eval().unwrap();
    let int_json = lua_to_json(&int_v).unwrap();
    assert!(int_json.is_i64(), "Lua integer 1 must map to a JSON integer");
    let int_bytes = cook_contracts::probe_value::encode_canonical_json(&int_json);
    assert_eq!(int_bytes, b"1\n");
    let int_back = cook_contracts::probe_value::decode_json(&int_bytes).unwrap();
    assert!(int_back.is_i64(), "decoded 1 must stay an integer");
    match json_to_lua(&lua, &int_back).unwrap() {
        LuaValue::Integer(i) => assert_eq!(i, 1),
        other => panic!("expected Lua integer, got {other:?}"),
    }
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

/// COOK-388 verification bar: two independently-created VMs (standing in
/// for the register pre-pass and an execute-phase worker) serialize a nasty
/// fixture — nested tables, unicode, integer/float mix — to IDENTICAL
/// canonical bytes through the one implementation.
#[test]
fn two_vms_agree_on_canonical_bytes() {
    const NASTY: &str = r#"return {
        name = "übergrün ★",
        counts = { 1, 2, 3 },
        ratio = 0.5,
        exact = 7,
        nested = { deep = { flag = true, items = { "α", "β" } } },
        empty = {},
    }"#;
    let register_vm = Lua::new();
    let worker_vm = Lua::new();
    let a: LuaValue = register_vm.load(NASTY).eval().unwrap();
    let b: LuaValue = worker_vm.load(NASTY).eval().unwrap();
    let bytes_a = cook_contracts::probe_value::encode_canonical_json(&lua_to_json(&a).unwrap());
    let bytes_b = cook_contracts::probe_value::encode_canonical_json(&lua_to_json(&b).unwrap());
    assert_eq!(bytes_a, bytes_b);
    assert!(!bytes_a.is_empty());
}
