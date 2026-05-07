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
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let id = parse_test_id("frontend.unit:test#1");
        assert_eq!(id_namespace(&id), "frontend");
        assert_eq!(id_recipe(&id), "unit");
    }

    #[test]
    fn parse_no_namespace() {
        let id = parse_test_id("build:test#1");
        assert_eq!(id_namespace(&id), "");
        assert_eq!(id_recipe(&id), "build");
    }

    #[test]
    fn parse_nested_namespace() {
        let id = parse_test_id("apps.web.unit:test#1[input.txt]");
        assert_eq!(id_namespace(&id), "apps.web");
        assert_eq!(id_recipe(&id), "unit");
    }
}
