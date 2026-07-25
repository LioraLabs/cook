//! CS-0171 annotation-seam tests.

use super::*;
use crate::dag_data::NodeData;

fn unit_node(id: &str, recipe: Option<&str>, cache_key: Option<&str>) -> NodeData {
    NodeData {
        id: id.to_string(),
        kind: "unit".to_string(),
        label: id.to_string(),
        recipe: recipe.map(str::to_string),
        command: None,
        output: None,
        cache_key: cache_key.map(str::to_string),
        dep_kind: None,
        group_index: None,
        modified: None,
        discovered: None,
    }
}

#[test]
fn facts_join_on_recipe_and_cache_key() {
    let mut a = Annotations::new();
    a.insert("build", "step:0", UnitFacts { served: true, observed_ms: Some(90), observed_builds_ago: 0 });

    let node = unit_node("unit:build:0", Some("build"), Some("step:0"));
    let f = a.for_node(&node).expect("joined");
    assert!(f.served);
    assert_eq!(f.observed_ms, Some(90));
}

/// The same cache key under a different recipe is a different unit. Two
/// recipes routinely register structurally identical steps.
#[test]
fn the_recipe_half_of_the_key_is_load_bearing() {
    let mut a = Annotations::new();
    a.insert("build", "step:0", UnitFacts { served: true, observed_ms: None, observed_builds_ago: 0 });

    let other = unit_node("unit:test:0", Some("test"), Some("step:0"));
    assert!(a.for_node(&other).is_none());
}

/// A file node has no recipe and no key, so it must never pick up some unit's
/// facts by accident.
#[test]
fn file_nodes_carry_no_facts() {
    let mut a = Annotations::new();
    a.insert("build", "step:0", UnitFacts { served: true, observed_ms: None, observed_builds_ago: 0 });

    let file = NodeData {
        kind: "file".to_string(),
        ..unit_node("file:src/main.c", None, None)
    };
    assert!(a.for_node(&file).is_none());
}

/// A non-cacheable unit (bare shell step, chore body) has a recipe but no
/// cache key. It is unclassified, not a rebuild.
#[test]
fn a_unit_without_a_cache_key_is_unclassified() {
    let mut a = Annotations::new();
    a.insert("build", "step:0", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });

    let node = unit_node("unit:build:3", Some("build"), None);
    assert!(a.for_node(&node).is_none());
}

#[test]
fn an_empty_annotation_set_classifies_nothing() {
    let a = Annotations::new();
    assert!(a.is_empty());
    assert_eq!(a.len(), 0);
    let node = unit_node("unit:build:0", Some("build"), Some("step:0"));
    assert!(a.for_node(&node).is_none());
}
