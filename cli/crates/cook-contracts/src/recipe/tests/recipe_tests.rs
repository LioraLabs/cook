use super::*;

fn graph(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
        .collect()
}

fn set(names: &[&str]) -> std::collections::BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_seed_reaches_itself_and_its_transitive_requires() {
    let g = graph(&[("app", &["lib"]), ("lib", &["gen"]), ("gen", &[]), ("other", &[])]);
    assert_eq!(reachable_from(&g, ["app".to_string()]), set(&["app", "lib", "gen"]));
}

#[test]
fn an_unknown_seed_contributes_nothing() {
    let g = graph(&[("app", &[])]);
    assert!(reachable_from(&g, ["nosuch".to_string()]).is_empty());
}

#[test]
fn a_dep_outside_the_map_is_skipped_not_added() {
    // A per-Cookfile map whose recipe requires a cross-Cookfile name. The
    // reference is real; resolving it is the workspace layer's job, and
    // reporting it as an unknown recipe is the analyzer's. Adding it here
    // would make a caller think it holds a name it does not.
    let g = graph(&[("app", &["sub.gen"])]);
    assert_eq!(reachable_from(&g, ["app".to_string()]), set(&["app"]));
}

#[test]
fn a_cycle_terminates() {
    let g = graph(&[("a", &["b"]), ("b", &["a"])]);
    assert_eq!(reachable_from(&g, ["a".to_string()]), set(&["a", "b"]));
}

#[test]
fn many_seeds_union_their_closures() {
    let g = graph(&[("a", &["x"]), ("b", &["y"]), ("x", &[]), ("y", &[]), ("z", &[])]);
    assert_eq!(
        reachable_from(&g, ["a".to_string(), "b".to_string()]),
        set(&["a", "b", "x", "y"])
    );
}

#[test]
fn no_seeds_reaches_nothing() {
    let g = graph(&[("a", &["b"]), ("b", &[])]);
    assert!(reachable_from(&g, Vec::<String>::new()).is_empty());
}
