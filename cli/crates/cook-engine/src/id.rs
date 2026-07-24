//! Helpers for parsing TestId strings.
//!
//! TestId format: `<namespace>.<recipe>:<name>[<discriminator>]` where
//! namespace and discriminator are optional.

use crate::TestId;

pub fn parse_test_id(node_name: &str) -> TestId {
    TestId(node_name.to_string())
}

pub fn id_namespace(id: &TestId) -> String {
    let s = &id.0;
    if let Some(colon) = s.find(':') {
        let before = &s[..colon];
        if let Some(dot) = before.rfind('.') {
            return before[..dot].to_string();
        }
    }
    String::new()
}

pub fn id_recipe(id: &TestId) -> String {
    let s = &id.0;
    let before_colon = s.split(':').next().unwrap_or("");
    if let Some(dot) = before_colon.rfind('.') {
        before_colon[dot + 1..].to_string()
    } else {
        before_colon.to_string()
    }
}

#[cfg(test)]
#[path = "tests/id_tests.rs"]
mod tests;
