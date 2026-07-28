//! Canonical rendering of a data member (§9.3 / COOK-64).
//!
//! A data member is a probe/JSON value. Two consumers need a stable
//! string form of the whole member:
//!
//!  - the `$<in>` placeholder (the member's textual rendering in a command);
//!  - the per-member cache fingerprint (§17.1 observable #5).
//!
//! Per §9.3 the rendering is **compact key-sorted JSON for a record** (or any
//! table) and **the scalar's bare string form otherwise** (no surrounding JSON
//! quotes). Key-sorting goes through [`crate::probe::value`]'s canonicaliser so
//! a record's rendering is independent of field insertion order (and of
//! serde_json's `preserve_order` feature).

/// Render a data member to its canonical string form (§9.3).
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
        other => {
            serde_json::to_string(&crate::probe::value::canonical_value(other)).unwrap_or_default()
        }
    }
}

#[cfg(test)]
#[path = "tests/member_tests.rs"]
mod tests;
