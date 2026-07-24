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
