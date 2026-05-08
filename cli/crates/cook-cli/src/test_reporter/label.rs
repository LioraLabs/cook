//! Display labels for tests, per §3.1 of the test-runner output design.
//!
//! Pure functions that map a test's identity to the human-readable
//! string the reporter prints (e.g. `recipe@line`, `recipe::name [item]`).

/// Produce the display label for a test.
///
/// - `recipe`        — recipe name (may be namespaced as `ns.recipe`)
/// - `name`          — explicit test name; empty for unnamed test steps
/// - `line`          — source line of the `test` step in the Cookfile
/// - `iteration_item`— the iteration item (e.g. input filename), if any
/// - `multi_namespace`— true iff the run touches more than one namespace
pub fn label(
    recipe: &str,
    name: &str,
    line: u32,
    iteration_item: Option<&str>,
    multi_namespace: bool,
) -> String {
    let core = if name.is_empty() {
        format!("{recipe}@{line}")
    } else {
        format!("{recipe}::{name}")
    };
    let core = if multi_namespace {
        core
    } else {
        // Strip leading "ns." if recipe was already namespace-prefixed and
        // the run is single-namespace. Recipe names never contain '.' in
        // their local form, so a leading "ns." segment is unambiguous.
        match core.find('.') {
            Some(idx) if idx + 1 < core.len() => core[idx + 1..].to_string(),
            _ => core,
        }
    };
    match iteration_item {
        Some(item) if !item.is_empty() => format!("{core} [{item}]"),
        _ => core,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unnamed_uses_recipe_at_line() {
        assert_eq!(label("fail_basic", "", 5, None, false), "fail_basic@5");
    }

    #[test]
    fn named_uses_recipe_double_colon_name() {
        assert_eq!(
            label("named_test", "happy_path", 12, None, false),
            "named_test::happy_path"
        );
    }

    #[test]
    fn iterated_unnamed_appends_item() {
        assert_eq!(
            label("pass_iterated", "", 8, Some("a.cpp"), false),
            "pass_iterated@8 [a.cpp]"
        );
    }

    #[test]
    fn iterated_named_appends_item() {
        assert_eq!(
            label("iter_test", "roundtrip", 8, Some("a.cpp"), false),
            "iter_test::roundtrip [a.cpp]"
        );
    }

    #[test]
    fn single_namespace_strips_prefix() {
        // recipe is "web.fail_basic", but only one namespace in the run
        assert_eq!(
            label("web.fail_basic", "", 5, None, false),
            "fail_basic@5"
        );
    }

    #[test]
    fn multi_namespace_keeps_prefix() {
        assert_eq!(
            label("web.fail_basic", "", 5, None, true),
            "web.fail_basic@5"
        );
    }

    #[test]
    fn empty_iteration_item_treated_as_none() {
        assert_eq!(label("r", "n", 1, Some(""), false), "r::n");
    }
}
