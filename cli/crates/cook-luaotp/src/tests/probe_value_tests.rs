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

#[test]
fn lua_to_json_rejects_mixed_key_table() {
    let lua = mlua::Lua::new();
    let v: mlua::Value = lua.load(r#"return { [1] = 1, a = 2 }"#).eval().unwrap();
    let err = lua_to_json(&v).unwrap_err();
    assert!(err.contains("mixed"), "error must mention 'mixed'; got: {err}");
}

#[test]
fn lua_to_json_rejects_array_hole() {
    let lua = mlua::Lua::new();
    let v: mlua::Value = lua
        .load(r#"return { [1] = "a", [3] = "c" }"#)
        .eval()
        .unwrap();
    let err = lua_to_json(&v).unwrap_err();
    assert!(
        err.contains("hole") || err.contains("not contiguous"),
        "error must mention the array hole; got: {err}"
    );
}

#[test]
fn lua_to_json_rejects_cycle() {
    let lua = mlua::Lua::new();
    let v: mlua::Value = lua
        .load(
            r#"
                local t = {}
                t.self = t
                return t
            "#,
        )
        .eval()
        .unwrap();
    let err = lua_to_json(&v).unwrap_err();
    assert!(err.contains("cycle"), "error must mention 'cycle'; got: {err}");
}

/// Float/integer identity must survive the full encode/decode round trip:
/// a Lua float `1.0` stays a float (renders `1.0`, decodes back to f64),
/// and a Lua integer `1` stays an integer. Conflating them would change
/// canonical bytes and re-key cache entries.
#[test]
fn float_identity_round_trips() {
    let lua = mlua::Lua::new();

    let float_v: mlua::Value = lua.load("return 1.0").eval().unwrap();
    let float_json = lua_to_json(&float_v).unwrap();
    assert!(float_json.is_f64(), "Lua float 1.0 must map to a JSON float");
    let float_bytes = cook_contracts::probe_value::encode_canonical_json(&float_json);
    assert_eq!(float_bytes, b"1.0\n");
    let float_back = cook_contracts::probe_value::decode_json(&float_bytes).unwrap();
    assert!(float_back.is_f64(), "decoded 1.0 must stay a float");
    match json_to_lua(&lua, &float_back).unwrap() {
        mlua::Value::Number(f) => assert_eq!(f, 1.0),
        other => panic!("expected Lua float, got {other:?}"),
    }

    let int_v: mlua::Value = lua.load("return 1").eval().unwrap();
    let int_json = lua_to_json(&int_v).unwrap();
    assert!(int_json.is_i64(), "Lua integer 1 must map to a JSON integer");
        let int_bytes = cook_contracts::probe_value::encode_canonical_json(&int_json);
        assert_eq!(int_bytes, b"1\n");
    let int_back = cook_contracts::probe_value::decode_json(&int_bytes).unwrap();
    assert!(int_back.is_i64(), "decoded 1 must stay an integer");
        match json_to_lua(&lua, &int_back).unwrap() {
            mlua::Value::Integer(i) => assert_eq!(i, 1),
            other => panic!("expected Lua integer, got {other:?}"),
    }
}
