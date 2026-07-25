use super::*;
use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};

fn unit(id: &str, recipe: &str, group: Option<usize>, cached: Option<bool>) -> NodeData {
    NodeData {
        id: id.to_string(),
        kind: "unit".to_string(),
        label: id.to_string(),
        recipe: Some(recipe.to_string()),
        command: Some("cc".to_string()),
        output: None,
        cached,
        dep_kind: None,
        group_index: group,
        modified: None,
        discovered: None,
    }
}

fn file(id: &str) -> NodeData {
    NodeData {
        id: id.to_string(),
        kind: "file".to_string(),
        label: id.to_string(),
        recipe: None,
        command: None,
        output: None,
        cached: None,
        dep_kind: None,
        group_index: None,
        modified: None,
        discovered: None,
    }
}

fn edge(from: &str, to: &str, kind: EdgeKind) -> EdgeData {
    EdgeData {
        from: from.to_string(),
        to: to.to_string(),
        kind,
    }
}

/// Two recipes: `lib` has two grouped compiles, `bin` links them. A barrier
/// edge and a data edge both run lib -> bin.
fn fixture() -> WaveDagData {
    WaveDagData {
        schema_version: 2,
        target: "bin".to_string(),
        waves: vec![WaveData {
            recipes: vec!["lib".to_string(), "bin".to_string()],
            nodes: vec![
                unit("unit:lib:0", "lib", Some(0), Some(true)),
                unit("unit:lib:1", "lib", Some(0), Some(true)),
                unit("unit:bin:0", "bin", None, Some(false)),
                file("file:main.c"),
            ],
            edges: vec![
                edge("file:main.c", "unit:lib:0", EdgeKind::Data),
                edge("unit:lib:0", "unit:lib:1", EdgeKind::Group),
                // Two edges crossing lib -> bin, one weak and one strong.
                edge("unit:lib:0", "unit:bin:0", EdgeKind::Data),
                edge("unit:lib:1", "unit:bin:0", EdgeKind::Barrier),
            ],
        }],
        inter_wave_edges: vec![],
    }
}

#[test]
fn recipe_level_collapses_units_into_one_node_per_recipe() {
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP).unwrap();
    let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["recipe:bin", "recipe:lib"]);
    let lib = g.nodes.iter().find(|n| n.id == "recipe:lib").unwrap();
    assert_eq!(lib.units, 2);
    assert_eq!(g.total_units, 3);
}

/// The rule that makes the collapse diagnostic rather than merely tidy: a
/// barrier bundled with data edges must be what survives.
#[test]
fn merged_edge_keeps_the_most_constraining_kind() {
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP).unwrap();
    let e = g
        .edges
        .iter()
        .find(|e| e.from == "recipe:lib" && e.to == "recipe:bin")
        .expect("lib -> bin edge");
    assert_eq!(e.kind, EdgeKind::Barrier, "barrier must not hide behind data");
    assert_eq!(e.count, 2);
}

#[test]
fn collapsed_self_edges_are_dropped() {
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP).unwrap();
    assert!(
        !g.edges.iter().any(|e| e.from == e.to),
        "intra-recipe ordering is what this level chose to hide"
    );
}

#[test]
fn file_nodes_are_dropped_above_unit_level() {
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP).unwrap();
    assert!(!g.nodes.iter().any(|n| n.id.starts_with("file:")));

    let u = aggregate(&fixture(), Level::Unit, UNIT_LEVEL_SOFT_CAP).unwrap();
    assert!(u.nodes.iter().any(|n| n.id == "file:main.c"));
}

#[test]
fn group_level_collapses_a_step_group_to_one_node() {
    let g = aggregate(&fixture(), Level::Group, UNIT_LEVEL_SOFT_CAP).unwrap();
    let grp = g.nodes.iter().find(|n| n.id == "group:lib:0").unwrap();
    assert_eq!(grp.units, 2);
    // The ungrouped bin unit stays itself.
    assert!(g.nodes.iter().any(|n| n.id == "unit:bin:0"));
}

#[test]
fn cached_is_all_or_nothing_else_unknown() {
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP).unwrap();
    let lib = g.nodes.iter().find(|n| n.id == "recipe:lib").unwrap();
    assert_eq!(lib.cached, Some(true), "both lib units cached");
    let bin = g.nodes.iter().find(|n| n.id == "recipe:bin").unwrap();
    assert_eq!(bin.cached, Some(false));
}

#[test]
fn unit_level_refuses_past_the_cap_instead_of_emitting_a_blob() {
    let mut dag = fixture();
    let nodes = &mut dag.waves[0].nodes;
    for i in 0..50 {
        nodes.push(unit(&format!("unit:big:{i}"), "big", None, None));
    }
    let err = aggregate(&dag, Level::Unit, 10).unwrap_err();
    assert!(matches!(err, EmitError::TooManyNodes { .. }));
    // The coarse levels still work on the same graph — that is the point of
    // refusing rather than truncating.
    assert!(aggregate(&dag, Level::Recipe, 10).is_ok());
}

#[test]
fn mermaid_labels_every_edge_with_its_kind() {
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP).unwrap();
    let out = render(&g, Format::Mermaid);
    assert!(out.starts_with("graph LR"), "{out}");
    assert!(out.contains("|barrier ×2|"), "{out}");
    // Barriers get a heavier stroke so they read at a glance.
    assert!(out.contains("linkStyle"), "{out}");
}

#[test]
fn text_render_reads_as_waits_on() {
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP).unwrap();
    let out = render(&g, Format::Text);
    assert!(out.contains("waits on"), "{out}");
    assert!(out.contains("barrier"), "{out}");
    assert!(out.contains("(waits on nothing)"), "{out}");
}

#[test]
fn dot_and_json_render_the_same_edge_set() {
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP).unwrap();
    let dot = render(&g, Format::Dot);
    assert!(dot.starts_with("digraph cook {"), "{dot}");
    assert!(dot.contains("penwidth=3"), "barrier should be heavy: {dot}");

    let json = render(&g, Format::Json);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(parsed["level"], "recipe");
    assert_eq!(parsed["edges"].as_array().unwrap().len(), g.edges.len());
    assert_eq!(parsed["edges"][0]["kind"], "barrier");
}
