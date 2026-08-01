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

/// COOK-411: `id_recipe` and `id_recipe_path` are two different answers, and
/// the bug was that both were once called "the recipe" in two crates. Pin the
/// difference so a future reader cannot collapse them by accident.
#[test]
fn recipe_and_recipe_path_differ_exactly_by_the_namespace() {
    let ns = parse_test_id("apps.web.unit:test#1[input.txt]");
    assert_eq!(id_namespace(&ns), "apps.web");
    assert_eq!(id_recipe(&ns), "unit");
    assert_eq!(id_recipe_path(&ns), "apps.web.unit");

    // With no namespace the two agree; this is why the disagreement went
    // unnoticed on single-Cookfile projects.
    let flat = parse_test_id("build:test#1");
    assert_eq!(id_namespace(&flat), "");
    assert_eq!(id_recipe(&flat), "build");
    assert_eq!(id_recipe_path(&flat), "build");
    assert_eq!(id_recipe(&flat), id_recipe_path(&flat));
}

/// `id_recipe_path` is defined in terms of the other two, so this holds by
/// construction rather than by inspection.
#[test]
fn recipe_path_is_namespace_joined_to_recipe() {
    for raw in [
        "unit:t",
        "build:t#3",
        "apps.web.unit:t",
        "a.b.c.d:t[x]",
        "apps.web.unit:t#2[in.txt]",
    ] {
        let id = parse_test_id(raw);
        let ns = id_namespace(&id);
        let expected = if ns.is_empty() {
            id_recipe(&id)
        } else {
            format!("{ns}.{}", id_recipe(&id))
        };
        assert_eq!(id_recipe_path(&id), expected, "for {raw}");
    }
}
