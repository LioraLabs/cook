use super::*;

#[test]
fn uses_recipe_at_line() {
    assert_eq!(label("fail_basic", 5, None, false), "fail_basic@5");
}

#[test]
fn iterated_appends_item() {
    assert_eq!(
        label("pass_iterated", 8, Some("a.cpp"), false),
        "pass_iterated@8 [a.cpp]"
    );
}

#[test]
fn single_namespace_strips_prefix() {
    // recipe is "web.fail_basic", but only one namespace in the run
    assert_eq!(label("web.fail_basic", 5, None, false), "fail_basic@5");
}

#[test]
fn multi_namespace_keeps_prefix() {
    assert_eq!(label("web.fail_basic", 5, None, true), "web.fail_basic@5");
}

#[test]
fn empty_iteration_item_treated_as_none() {
    assert_eq!(label("r", 1, Some(""), false), "r@1");
}
