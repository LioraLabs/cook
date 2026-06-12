//! Canonical rendering of a `for_each` data member (§8.3 / COOK-64).
//!
//! A `for_each` member is a probe/JSON value. Two consumers need a stable
//! string form of the whole member:
//!
//!  - the `$<in>` placeholder (the member's textual rendering in a command);
//!  - the per-member cache fingerprint (§17.1 observable #5).
//!
//! Per §8.3 the rendering is **compact key-sorted JSON for a record** (or any
//! table) and **the scalar's bare string form otherwise** (no surrounding JSON
//! quotes). Key-sorting goes through [`crate::probe_value`]'s canonicaliser so
//! a record's rendering is independent of field insertion order (and of
//! serde_json's `preserve_order` feature).

/// Render a `for_each` data member to its canonical string form (§8.3).
///
/// - A table (record or array) renders as compact, key-sorted JSON.
/// - A string scalar renders as its raw text (no surrounding quotes).
/// - A number / boolean / nil renders as its JSON scalar text (`42`, `true`,
///   `null`).
///
/// JSON-native since CS-0102 (COOK-91); previously took the pre-CS-0102
/// decoded value type.
pub fn member_to_string(json: &serde_json::Value) -> String {
    match json {
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string(&crate::probe_value::canonical_value(other))
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
