//! Canonical rendering of a `for_each` data member (§8.3 / COOK-64).
//!
//! A `for_each` member is a probe/JSON value. Two consumers need a stable
//! string form of the whole member:
//!
//!  - the `$<item>` placeholder (the member's textual rendering in a command);
//!  - the per-member cache fingerprint (§17.1 observable #5).
//!
//! Per §8.3 the rendering is **canonical key-sorted JSON for a record** (or any
//! table) and **the scalar's bare string form otherwise** (no surrounding JSON
//! quotes). Key-sorting falls out of `serde_json::Map`'s default `BTreeMap`
//! backing, so a record's rendering is independent of field insertion order.

use rmpv::Value as MsgPackValue;
use serde_json::Value as JsonValue;

/// Render a `for_each` data member to its canonical string form.
///
/// - A table (record or array) renders as compact, key-sorted JSON.
/// - A string scalar renders as its raw bytes (no surrounding quotes).
/// - A number / boolean / nil renders as its JSON scalar text (`42`, `true`,
///   `null`).
pub fn member_to_string(v: &MsgPackValue) -> String {
    let json = to_json(v);
    match &json {
        // A bare string member interpolates verbatim — not as a quoted JSON
        // string literal.
        JsonValue::String(s) => s.clone(),
        // Tables render as canonical JSON; the default `serde_json::Map` is a
        // `BTreeMap`, so object keys come out lexicographically sorted.
        JsonValue::Object(_) | JsonValue::Array(_) => {
            serde_json::to_string(&json).unwrap_or_default()
        }
        // Remaining scalars (number / bool / null) share the JSON text form.
        _ => serde_json::to_string(&json).unwrap_or_default(),
    }
}

/// Convert an `rmpv::Value` to a `serde_json::Value`. Map keys are coerced to
/// their string form (probe records are string-keyed per §22.5.4); the
/// `serde_json::Map` (a `BTreeMap` by default) yields key-sorted serialisation.
fn to_json(v: &MsgPackValue) -> JsonValue {
    match v {
        MsgPackValue::Nil => JsonValue::Null,
        MsgPackValue::Boolean(b) => JsonValue::Bool(*b),
        MsgPackValue::Integer(i) => {
            if let Some(u) = i.as_u64() {
                JsonValue::from(u)
            } else if let Some(s) = i.as_i64() {
                JsonValue::from(s)
            } else {
                JsonValue::Null
            }
        }
        MsgPackValue::F32(f) => {
            serde_json::Number::from_f64(*f as f64).map_or(JsonValue::Null, JsonValue::Number)
        }
        MsgPackValue::F64(f) => {
            serde_json::Number::from_f64(*f).map_or(JsonValue::Null, JsonValue::Number)
        }
        MsgPackValue::String(s) => JsonValue::String(s.as_str().unwrap_or("").to_string()),
        MsgPackValue::Binary(b) => JsonValue::String(String::from_utf8_lossy(b).into_owned()),
        MsgPackValue::Array(items) => JsonValue::Array(items.iter().map(to_json).collect()),
        MsgPackValue::Map(entries) => {
            let mut map = serde_json::Map::new();
            for (k, val) in entries {
                let key = match k {
                    MsgPackValue::String(s) => s.as_str().unwrap_or("").to_string(),
                    MsgPackValue::Integer(i) => i.to_string(),
                    other => member_to_string(other),
                };
                map.insert(key, to_json(val));
            }
            JsonValue::Object(map)
        }
        MsgPackValue::Ext(..) => JsonValue::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpv::Value;

    #[test]
    fn renders_record_key_sorted() {
        // Insertion order name-then-id; canonical form sorts id before name.
        let v = Value::Map(vec![
            (Value::String("name".into()), Value::String("ace".into())),
            (Value::String("id".into()), Value::Integer(1.into())),
        ]);
        assert_eq!(member_to_string(&v), r#"{"id":1,"name":"ace"}"#);
    }

    #[test]
    fn renders_scalars_bare() {
        assert_eq!(member_to_string(&Value::String("hi".into())), "hi");
        assert_eq!(member_to_string(&Value::Integer(42.into())), "42");
        assert_eq!(member_to_string(&Value::Boolean(true)), "true");
        assert_eq!(member_to_string(&Value::Nil), "null");
    }

    #[test]
    fn renders_nested_record_and_array() {
        let v = Value::Map(vec![
            (
                Value::String("tags".into()),
                Value::Array(vec![Value::String("a".into()), Value::String("b".into())]),
            ),
            (
                Value::String("meta".into()),
                Value::Map(vec![(Value::String("k".into()), Value::Integer(2.into()))]),
            ),
        ]);
        // Inner strings ARE quoted (JSON); outer keys sorted (meta < tags).
        assert_eq!(member_to_string(&v), r#"{"meta":{"k":2},"tags":["a","b"]}"#);
    }

    #[test]
    fn key_sort_is_insertion_order_independent() {
        let a = Value::Map(vec![
            (Value::String("b".into()), Value::Integer(2.into())),
            (Value::String("a".into()), Value::Integer(1.into())),
        ]);
        let b = Value::Map(vec![
            (Value::String("a".into()), Value::Integer(1.into())),
            (Value::String("b".into()), Value::Integer(2.into())),
        ]);
        assert_eq!(member_to_string(&a), member_to_string(&b));
    }
}
