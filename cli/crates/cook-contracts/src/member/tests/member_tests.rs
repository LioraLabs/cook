use super::member_to_string;

#[test]
fn renders_record_key_sorted_compact() {
    let v = serde_json::json!({"name": "ace", "id": 1});
    assert_eq!(member_to_string(&v), r#"{"id":1,"name":"ace"}"#);
}

#[test]
fn renders_scalars_bare() {
    assert_eq!(member_to_string(&serde_json::json!("hi")), "hi");
    assert_eq!(member_to_string(&serde_json::json!(42)), "42");
    assert_eq!(member_to_string(&serde_json::json!(true)), "true");
    assert_eq!(member_to_string(&serde_json::Value::Null), "null");
}

#[test]
fn renders_nested_record_and_array() {
    let v = serde_json::json!({
        "tags": ["a", "b"],
        "meta": {"k": 2}
    });
    // Inner strings ARE quoted (JSON); outer keys sorted (meta < tags).
    assert_eq!(member_to_string(&v), r#"{"meta":{"k":2},"tags":["a","b"]}"#);
}

#[test]
fn key_sort_is_insertion_order_independent() {
    // serde_json::json! preserves source order only if preserve_order is
    // on; the canonicaliser must make these equal regardless.
    let mut a = serde_json::Map::new();
    a.insert("b".to_string(), serde_json::json!(2));
    a.insert("a".to_string(), serde_json::json!(1));
    let mut b = serde_json::Map::new();
    b.insert("a".to_string(), serde_json::json!(1));
    b.insert("b".to_string(), serde_json::json!(2));
    assert_eq!(
        member_to_string(&serde_json::Value::Object(a)),
        member_to_string(&serde_json::Value::Object(b))
    );
}

/// Nested-structure rendering pin: the exact output for a representative
/// member record. A change here re-keys every fan-out cache entry.
#[test]
fn nested_member_rendering_is_pinned() {
    let v = serde_json::json!({
        "id": 1,
        "name": "ace",
        "tags": ["x", "y"],
        "meta": {"k": 2}
    });
    assert_eq!(
        member_to_string(&v),
        r#"{"id":1,"meta":{"k":2},"name":"ace","tags":["x","y"]}"#
    );
}
