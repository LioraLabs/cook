//! §22.5.7 / CS-0192: substitution rendering is defined over the JSON value.
//!
//! Each rendering row and each diagnostic of the spec's substitution rules is
//! pinned here, against the law itself; the e2e fixtures pin the same facts
//! through the real pipeline.

use serde_json::json;

use crate::sigil::subst::substitute;
use crate::sigil::probe_ref;

/// Parse `ident`, walk its path over `value`, render.
fn subst(value: serde_json::Value, ident: &str) -> Result<String, String> {
    let r = probe_ref(ident).expect("test ident must be probe-shaped");
    substitute(&value, r.path(), ident)
}

#[test]
fn string_renders_verbatim_unquoted() {
    assert_eq!(subst(json!("hello"), "v:s").unwrap(), "hello");
    // No escaping, no quoting: shell-significant bytes pass through.
    assert_eq!(subst(json!("-O2 -Wall"), "v:s").unwrap(), "-O2 -Wall");
    assert_eq!(subst(json!("a\"b'c$d"), "v:s").unwrap(), "a\"b'c$d");
}

#[test]
fn integer_renders_without_decimal_point() {
    assert_eq!(subst(json!(42), "v:n").unwrap(), "42");
    assert_eq!(subst(json!(-7), "v:n").unwrap(), "-7");
}

#[test]
fn float_renders_as_shortest_round_trip() {
    // Lua's %.14g rendered this as "0.3"; the canonical token does not lie
    // about the value.
    assert_eq!(
        subst(json!(0.1_f64 + 0.2_f64), "v:f").unwrap(),
        "0.30000000000000004"
    );
    assert_eq!(subst(json!(3.0_f64), "v:f").unwrap(), "3.0");
}

#[test]
fn number_token_matches_canonical_bytes() {
    // The invariant the spec states: the token substituted into the command
    // and the token inside the value's canonical bytes are the same bytes.
    for v in [json!(42), json!(3.0_f64), json!(0.1_f64 + 0.2_f64), json!(-0.0_f64)] {
        let rendered = subst(v.clone(), "v:n").unwrap();
        let canonical = crate::probe_value::encode_canonical_json(&v);
        let canonical = std::str::from_utf8(&canonical).unwrap().trim_end();
        assert_eq!(rendered, canonical, "value: {v}");
    }
}

#[test]
fn boolean_renders_as_literal() {
    assert_eq!(subst(json!(true), "v:b").unwrap(), "true");
    assert_eq!(subst(json!(false), "v:b").unwrap(), "false");
}

#[test]
fn null_is_a_diagnostic_naming_the_placeholder() {
    let err = subst(json!(null), "v:nil").unwrap_err();
    assert!(err.contains("$<v:nil>"), "{err}");
    assert!(err.contains("null"), "{err}");
}

#[test]
fn array_is_a_diagnostic_with_a_scalar_hint() {
    let err = subst(json!(["a", "b"]), "v:arr").unwrap_err();
    assert!(err.contains("$<v:arr>"), "{err}");
    assert!(err.contains("array"), "{err}");
    assert!(err.contains("$<v:arr[1]>"), "{err}");
    // The heap address this replaces must never resurface.
    assert!(!err.contains("table:"), "{err}");
}

#[test]
fn object_is_a_diagnostic_with_a_member_hint() {
    let err = subst(json!({"k": "vee"}), "v:obj").unwrap_err();
    assert!(err.contains("$<v:obj>"), "{err}");
    assert!(err.contains("object"), "{err}");
    assert!(err.contains("$<v:obj.FIELD>"), "{err}");
}

#[test]
fn field_and_index_address_into_the_value() {
    let v = json!({"cflags": ["-O2", "-Wall"], "name": "zlib"});
    assert_eq!(subst(v.clone(), "cc:zlib.name").unwrap(), "zlib");
    // One-based, per §22.5.7.
    assert_eq!(subst(v.clone(), "cc:zlib.cflags[1]").unwrap(), "-O2");
    assert_eq!(subst(v, "cc:zlib.cflags[2]").unwrap(), "-Wall");
}

#[test]
fn absent_member_is_a_diagnostic_not_the_word_nil() {
    let err = subst(json!({"k": "v"}), "v:obj.missing").unwrap_err();
    assert!(err.contains("$<v:obj.missing>"), "{err}");
    assert!(err.contains("no member 'missing'"), "{err}");
}

#[test]
fn out_of_range_index_is_a_diagnostic() {
    let err = subst(json!({"a": ["x"]}), "v:t.a[2]").unwrap_err();
    assert!(err.contains("out of range"), "{err}");
    assert!(err.contains("1 elements"), "{err}");
}

#[test]
fn zero_index_names_one_based() {
    let err = subst(json!({"a": ["x"]}), "v:t.a[0]").unwrap_err();
    assert!(err.contains("one-based"), "{err}");
}

#[test]
fn member_of_non_object_is_a_diagnostic_naming_the_type() {
    let err = subst(json!({"a": "str"}), "v:t.a.deeper").unwrap_err();
    assert!(err.contains("cannot address member 'deeper' of a string value"), "{err}");
}

#[test]
fn index_into_non_array_is_a_diagnostic_naming_the_type() {
    let err = subst(json!({"a": {"k": 1}}), "v:t.a[1]").unwrap_err();
    assert!(err.contains("cannot index an object value"), "{err}");
}

#[test]
fn non_numeric_index_is_a_diagnostic() {
    // The scanner admits `[foo]`; the value walk refuses it.
    let r = probe_ref("v:t.a[foo]").expect("probe-shaped");
    let err = substitute(&json!({"a": ["x"]}), r.path(), "v:t.a[foo]").unwrap_err();
    assert!(err.contains("`[foo]` is not a numeric index"), "{err}");
}

#[test]
fn nested_scalar_renders_through_chained_segments() {
    let v = json!({"tools": {"gcc": {"hash": "ab12", "path": "/usr/bin/gcc"}}});
    assert_eq!(subst(v, "cc:tc.tools.gcc.path").unwrap(), "/usr/bin/gcc");
}
