use super::*;
use cook_engine::TestScope;
use std::collections::BTreeSet;

fn names(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn empty_recipe_set_defers_to_engine_as_recipe() {
    // When the workspace can't be loaded we treat the arg as a recipe so
    // the engine's canonical "unknown recipe" diagnostic surfaces.
    let scope = resolve_test_scope("anything", &BTreeSet::new()).unwrap();
    match scope {
        TestScope::Recipe(n) => assert_eq!(n, "anything"),
        other => panic!("expected Recipe, got {other:?}"),
    }
}

#[test]
fn exact_recipe_match_returns_recipe() {
    let set = names(&["build", "sub.pass", "sub.fail_one"]);
    let scope = resolve_test_scope("sub.pass", &set).unwrap();
    match scope {
        TestScope::Recipe(n) => assert_eq!(n, "sub.pass"),
        other => panic!("expected Recipe, got {other:?}"),
    }
}

#[test]
fn bare_recipe_match_returns_recipe() {
    let set = names(&["build", "sub.pass"]);
    let scope = resolve_test_scope("build", &set).unwrap();
    match scope {
        TestScope::Recipe(n) => assert_eq!(n, "build"),
        other => panic!("expected Recipe, got {other:?}"),
    }
}

#[test]
fn single_segment_namespace_match_returns_namespace() {
    // Reproduction case from the bug report: `cook test web` with
    // `web.build` defined under `import web ./web` MUST resolve as
    // a Namespace, not a (failing) Recipe lookup.
    let set = names(&["build", "web.build", "web.test"]);
    let scope = resolve_test_scope("web", &set).unwrap();
    match scope {
        TestScope::Namespace(n) => assert_eq!(n, "web"),
        other => panic!("expected Namespace, got {other:?}"),
    }
}

#[test]
fn nested_namespace_match_returns_namespace() {
    let set = names(&["apps.web.build", "apps.web.unit", "apps.api.build"]);
    let scope = resolve_test_scope("apps.web", &set).unwrap();
    match scope {
        TestScope::Namespace(n) => assert_eq!(n, "apps.web"),
        other => panic!("expected Namespace, got {other:?}"),
    }
}

#[test]
fn recipe_match_wins_over_namespace_match() {
    // If both a recipe `foo` and recipes `foo.bar` exist (which can happen
    // with deeply-nested imports), prefer the exact recipe match.
    let set = names(&["foo", "foo.bar", "foo.baz"]);
    let scope = resolve_test_scope("foo", &set).unwrap();
    match scope {
        TestScope::Recipe(n) => assert_eq!(n, "foo"),
        other => panic!("expected Recipe (exact match wins), got {other:?}"),
    }
}

#[test]
fn unknown_scope_errors_with_useful_diagnostic() {
    let set = names(&["build", "web.build", "web.test"]);
    let err = resolve_test_scope("xyz", &set).expect_err("unknown scope must error");
        let msg = format!("{err}");
    assert!(msg.contains("unknown test scope: 'xyz'"), "message: {msg}");
    assert!(msg.contains("recipe name"), "message: {msg}");
    assert!(msg.contains("namespace"), "message: {msg}");
    assert!(msg.contains("--filter"), "message: {msg}");
}

#[test]
fn unknown_scope_does_not_swallow_partial_namespace_typo() {
    // `webs` doesn't match the recipe `web.build` exactly nor the
    // namespace `webs.` — we must error rather than silently widening.
    let set = names(&["web.build", "web.test"]);
    let err = resolve_test_scope("webs", &set).expect_err("typo must error");
        let msg = format!("{err}");
    assert!(msg.contains("unknown test scope: 'webs'"), "message: {msg}");
}
