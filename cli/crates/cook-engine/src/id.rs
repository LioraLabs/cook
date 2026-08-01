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

/// The recipe's own name, without its namespace: `apps.web.unit:t` is `unit`.
///
/// See [`id_recipe_path`] when the namespace matters. Both answers are
/// legitimate and they used to be spelled in two crates under one name; see
/// that function's note.
pub fn id_recipe(id: &TestId) -> String {
    let s = &id.0;
    let before_colon = s.split(':').next().unwrap_or("");
    if let Some(dot) = before_colon.rfind('.') {
        before_colon[dot + 1..].to_string()
    } else {
        before_colon.to_string()
    }
}

/// The recipe qualified by its namespace: `apps.web.unit:t` is `apps.web.unit`.
///
/// # Why this exists (COOK-411)
///
/// `cook-cli`'s test reporter had its own `recipe_of`, returning everything
/// before the `:`, while this module's [`id_recipe`] returns the last dotted
/// segment. Both were called "the recipe" and they answer differently for any
/// namespaced test, so the JUnit sidecar grouped its `<testsuite name>` and
/// `classname` by a different key than every other consumer in the workspace.
///
/// Neither was wrong. A qualified name is the right `classname` (it separates
/// `apps.web.unit` from `apps.api.unit`, which a bare `unit` would merge), and
/// the bare name is right for a progress line. The defect was one name for two
/// answers, in two crates. They live together now, named for what they return,
/// and this one is defined in terms of the other two so the three cannot
/// disagree.
pub fn id_recipe_path(id: &TestId) -> String {
    let ns = id_namespace(id);
    let recipe = id_recipe(id);
    if ns.is_empty() {
        recipe
    } else {
        format!("{ns}.{recipe}")
    }
}

#[cfg(test)]
#[path = "tests/id_tests.rs"]
mod tests;
