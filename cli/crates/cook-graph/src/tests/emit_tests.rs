use super::*;
use crate::annotate::{Annotations, UnitFacts};
use crate::dag_data::{DagData, EdgeData, NodeData};

fn unit(id: &str, recipe: &str, group: Option<usize>, cache_key: Option<&str>) -> NodeData {
    NodeData {
        id: id.to_string(),
        kind: "unit".to_string(),
        label: id.to_string(),
        recipe: Some(recipe.to_string()),
        command: Some("cc".to_string()),
        output: None,
        cache_key: cache_key.map(str::to_string),
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
        cache_key: None,
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
fn fixture() -> DagData {
    DagData {
        schema_version: crate::DAG_SCHEMA_VERSION,
        target: "bin".to_string(),
        recipes: vec!["lib".to_string(), "bin".to_string()],
        nodes: vec![
            unit("unit:lib:0", "lib", Some(0), Some("lib:0")),
            unit("unit:lib:1", "lib", Some(0), Some("lib:1")),
            unit("unit:bin:0", "bin", None, Some("bin:0")),
            file("file:main.c"),
        ],
        edges: vec![
            edge("file:main.c", "unit:lib:0", EdgeKind::Data),
            edge("unit:lib:0", "unit:lib:1", EdgeKind::Group),
            // Two edges crossing lib -> bin, one weak and one strong.
            edge("unit:lib:0", "unit:bin:0", EdgeKind::Data),
            edge("unit:lib:1", "unit:bin:0", EdgeKind::Barrier),
        ],
    }
}

/// Both `lib` units served from cache, `bin` rebuilding. The shape the default
/// incremental case takes.
fn facts() -> Annotations {
    let mut a = Annotations::new();
    a.insert("lib", "lib:0", UnitFacts { served: true, observed_ms: Some(400), observed_builds_ago: 0 });
    a.insert("lib", "lib:1", UnitFacts { served: true, observed_ms: Some(600), observed_builds_ago: 0 });
    a.insert("bin", "bin:0", UnitFacts { served: false, observed_ms: Some(2100), observed_builds_ago: 0 });
    a
}

fn agg(level: Level) -> Graph {
    aggregate(&fixture(), level, UNIT_LEVEL_SOFT_CAP, &facts()).unwrap()
}

#[test]
fn recipe_level_collapses_units_into_one_node_per_recipe() {
    let g = agg(Level::Recipe);
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
    let g = agg(Level::Recipe);
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
    let g = agg(Level::Recipe);
    assert!(
        !g.edges.iter().any(|e| e.from == e.to),
        "intra-recipe ordering is what this level chose to hide"
    );
}

#[test]
fn file_nodes_are_dropped_above_unit_level() {
    let g = agg(Level::Recipe);
    assert!(!g.nodes.iter().any(|n| n.id.starts_with("file:")));

    let u = agg(Level::Unit);
    assert!(u.nodes.iter().any(|n| n.id == "file:main.c"));
}

/// A file node is structure, not work. Counting it as a unit would give it a
/// zero-millisecond "observation" over zero real units, which renders as
/// "0ms observed" — absence dressed up as speed.
#[test]
fn file_nodes_count_toward_no_tally() {
    let u = agg(Level::Unit);
    let f = u.nodes.iter().find(|n| n.id == "file:main.c").unwrap();
    assert_eq!(f.units, 0);
    assert_eq!((f.hits, f.rebuilds, f.unclassified), (0, 0, 0));
    assert_eq!((f.observed_ms, f.unobserved), (0, 0));
    // Substring checks are useless here ("400ms observed" contains
    // "0ms observed"), so assert on the file node's own rendered line.
    let text = render(&u, Format::Text);
    let line = text
        .lines()
        .find(|l| l.starts_with("file:main.c"))
        .expect("file node rendered");
    assert_eq!(line.trim(), "file:main.c", "file node must carry no tally: {line:?}");
}

#[test]
fn group_level_collapses_a_step_group_to_one_node() {
    let g = agg(Level::Group);
    let grp = g.nodes.iter().find(|n| n.id == "group:lib:0").unwrap();
    assert_eq!(grp.units, 2);
    // The ungrouped bin unit stays itself.
    assert!(g.nodes.iter().any(|n| n.id == "unit:bin:0"));
}

// ---------------------------------------------------------------------------
// CS-0171: tallies, cascade, timing
// ---------------------------------------------------------------------------

/// §17.1.6.2: a collapsed node reports counts, not a boolean. The boolean this
/// replaced could not tell 339-hits-and-one-rebuild from 340 rebuilds.
#[test]
fn collapsed_nodes_tally_hits_and_rebuilds() {
    let g = agg(Level::Recipe);
    let lib = g.nodes.iter().find(|n| n.id == "recipe:lib").unwrap();
    assert_eq!((lib.hits, lib.rebuilds), (2, 0));
    let bin = g.nodes.iter().find(|n| n.id == "recipe:bin").unwrap();
    assert_eq!((bin.hits, bin.rebuilds), (0, 1));
}

#[test]
fn a_mixed_node_is_distinguishable_from_a_uniform_one() {
    let mut a = Annotations::new();
    a.insert("lib", "lib:0", UnitFacts { served: true, observed_ms: None, observed_builds_ago: 0 });
    a.insert("lib", "lib:1", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    a.insert("bin", "bin:0", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP, &a).unwrap();

    let lib = g.nodes.iter().find(|n| n.id == "recipe:lib").unwrap();
    assert_eq!((lib.hits, lib.rebuilds), (1, 1));
    let out = render(&g, Format::Text);
    assert!(out.contains("1 hit / 1 rebuild"), "{out}");
}

/// A unit with no annotation is unclassified. Counting it as a rebuild would
/// inflate every tally on the page.
#[test]
fn unannotated_units_are_unclassified_not_rebuilds() {
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP, &Annotations::new()).unwrap();
    let lib = g.nodes.iter().find(|n| n.id == "recipe:lib").unwrap();
    assert_eq!((lib.hits, lib.rebuilds, lib.unclassified), (0, 0, 2));
    assert!(!lib.rebuilding());
}

/// §17.1.6.3: the cost of a rebuild is what it invalidates downstream, and
/// that is only answerable with the edges.
#[test]
fn cascade_counts_units_downstream() {
    let mut a = Annotations::new();
    a.insert("lib", "lib:0", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    a.insert("lib", "lib:1", UnitFacts { served: true, observed_ms: None, observed_builds_ago: 0 });
    a.insert("bin", "bin:0", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    let g = aggregate(&fixture(), Level::Unit, UNIT_LEVEL_SOFT_CAP, &a).unwrap();

    let lib0 = g.nodes.iter().find(|n| n.id == "unit:lib:0").unwrap();
    assert_eq!(lib0.forces, 2, "lib:1 and bin:0 are downstream of lib:0");
    let bin = g.nodes.iter().find(|n| n.id == "unit:bin:0").unwrap();
    assert_eq!(bin.forces, 0, "nothing is downstream of the terminal unit");
}

/// The count must include downstream units that are cache hits *right now*.
/// Those are the ones a reader most needs told about: their inputs have not
/// changed yet only because the upstream rebuild has not happened.
#[test]
fn cascade_counts_downstream_units_that_currently_hit() {
    let mut a = Annotations::new();
    a.insert("lib", "lib:0", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    a.insert("lib", "lib:1", UnitFacts { served: true, observed_ms: None, observed_builds_ago: 0 });
    // The consumer still looks warm: its input has not been rebuilt yet.
    a.insert("bin", "bin:0", UnitFacts { served: true, observed_ms: None, observed_builds_ago: 0 });
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP, &a).unwrap();

    let lib = g.nodes.iter().find(|n| n.id == "recipe:lib").unwrap();
    assert_eq!(lib.rebuilds, 1);
    assert_eq!(lib.forces, 1, "the currently-hitting consumer still counts");
    let out = render(&g, Format::Text);
    assert!(out.contains("invalidates 1 downstream unit"), "{out}");
    // And it must not claim the consumer will certainly re-execute.
    assert!(!out.contains("will rebuild"), "{out}");
}

#[test]
fn cascade_is_transitive_and_counts_each_unit_once() {
    // a -> b -> c, and a -> c directly. `c` must be counted once from `a`.
    let dag = DagData {
        schema_version: crate::DAG_SCHEMA_VERSION,
        target: "x".to_string(),
        recipes: vec!["x".to_string()],
        nodes: vec![
            unit("unit:x:0", "x", None, Some("x:0")),
            unit("unit:x:1", "x", None, Some("x:1")),
            unit("unit:x:2", "x", None, Some("x:2")),
        ],
        edges: vec![
            edge("unit:x:0", "unit:x:1", EdgeKind::Data),
            edge("unit:x:1", "unit:x:2", EdgeKind::Data),
            edge("unit:x:0", "unit:x:2", EdgeKind::Data),
        ],
    };
    let mut a = Annotations::new();
    for k in ["x:0", "x:1", "x:2"] {
        a.insert("x", k, UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    }
    let g = aggregate(&dag, Level::Unit, UNIT_LEVEL_SOFT_CAP, &a).unwrap();
    let n0 = g.nodes.iter().find(|n| n.id == "unit:x:0").unwrap();
    assert_eq!(n0.forces, 2, "x:1 and x:2, and x:2 only once");
    let n1 = g.nodes.iter().find(|n| n.id == "unit:x:1").unwrap();
    assert_eq!(n1.forces, 1, "only x:2 is downstream of x:1");
}

/// The text rendering has to name the upstream that is itself rebuilding,
/// rather than presenting the downstream miss as an independent finding.
#[test]
fn text_marks_a_rebuilding_upstream() {
    let mut a = Annotations::new();
    a.insert("lib", "lib:0", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    a.insert("lib", "lib:1", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    a.insert("bin", "bin:0", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP, &a).unwrap();
    let out = render(&g, Format::Text);
    assert!(out.contains("← rebuilding"), "{out}");
    assert!(out.contains("invalidates 1 downstream unit"), "{out}");
}

/// §17.1.6.4: timing is an observation, and a partial one has to admit it.
#[test]
fn timing_renders_as_observation_and_admits_its_coverage() {
    let g = agg(Level::Recipe);
    let out = render(&g, Format::Text);
    assert!(out.contains("observed"), "{out}");
    assert!(!out.contains("estimate"), "must not read as a prediction: {out}");

    // One of lib's two units never timed: coverage must be stated.
    let mut a = Annotations::new();
    a.insert("lib", "lib:0", UnitFacts { served: true, observed_ms: Some(400), observed_builds_ago: 0 });
    a.insert("lib", "lib:1", UnitFacts { served: true, observed_ms: None, observed_builds_ago: 0 });
    a.insert("bin", "bin:0", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    let partial = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP, &a).unwrap();
    let out = render(&partial, Format::Text);
    assert!(out.contains("(1 of 2 units)"), "{out}");
}

/// §17.1.6.4: how stale the number is, is part of the observation. A total
/// summed from a fifteen-builds-old timing should not read like one measured
/// on the last run.
#[test]
fn timing_reports_the_age_of_its_oldest_contributor() {
    let mut a = Annotations::new();
    a.insert("lib", "lib:0", UnitFacts { served: true, observed_ms: Some(400), observed_builds_ago: 0 });
    a.insert("lib", "lib:1", UnitFacts { served: true, observed_ms: Some(600), observed_builds_ago: 7 });
    a.insert("bin", "bin:0", UnitFacts { served: true, observed_ms: Some(100), observed_builds_ago: 0 });
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP, &a).unwrap();

    let lib = g.nodes.iter().find(|n| n.id == "recipe:lib").unwrap();
    assert_eq!(lib.observed_max_age, 7, "the weakest contributor sets the bound");
    let out = render(&g, Format::Text);
    assert!(out.contains("up to 7 builds ago"), "{out}");

    // A node whose observations are all current says nothing about age.
    let bin_line = out.lines().find(|l| l.starts_with("bin")).unwrap();
    assert!(!bin_line.contains("ago"), "{bin_line:?}");
}

#[test]
fn a_never_observed_node_shows_no_duration_at_all() {
    let mut a = Annotations::new();
    a.insert("bin", "bin:0", UnitFacts { served: false, observed_ms: None, observed_builds_ago: 0 });
    let g = aggregate(&fixture(), Level::Recipe, UNIT_LEVEL_SOFT_CAP, &a).unwrap();
    let bin = g.nodes.iter().find(|n| n.id == "recipe:bin").unwrap();
    assert_eq!(bin.observed_ms, 0);
    assert_eq!(bin.unobserved, 1);
    // Zero is not rendered as a duration — absence is absence, not "fast".
    // Asserted on the node's own line: a bare `contains` would be satisfied by
    // any duration ending in a zero ("400ms observed" contains "0ms observed").
    let text = render(&g, Format::Text);
    let line = text
        .lines()
        .find(|l| l.starts_with("bin"))
        .expect("bin node rendered");
    assert!(!line.contains("observed"), "{line:?}");
}

#[test]
fn unit_level_refuses_past_the_cap_instead_of_emitting_a_blob() {
    let mut dag = fixture();
    for i in 0..50 {
        dag.nodes.push(unit(&format!("unit:big:{i}"), "big", None, None));
    }
    let err = aggregate(&dag, Level::Unit, 10, &facts()).unwrap_err();
    assert!(matches!(err, EmitError::TooManyNodes { .. }));
    // The coarse levels still work on the same graph — that is the point of
    // refusing rather than truncating.
    assert!(aggregate(&dag, Level::Recipe, 10, &facts()).is_ok());
}

#[test]
fn mermaid_labels_every_edge_with_its_kind() {
    let g = agg(Level::Recipe);
    let out = render(&g, Format::Mermaid);
    assert!(out.starts_with("graph LR"), "{out}");
    assert!(out.contains("|barrier ×2|"), "{out}");
    // Barriers get a heavier stroke so they read at a glance.
    assert!(out.contains("linkStyle"), "{out}");
    // And so do the nodes that will actually run.
    assert!(out.contains("style recipe_bin"), "{out}");
}

#[test]
fn text_render_reads_as_waits_on() {
    let g = agg(Level::Recipe);
    let out = render(&g, Format::Text);
    assert!(out.contains("waits on"), "{out}");
    assert!(out.contains("barrier"), "{out}");
    // At recipe level file inputs are collapsed away, so the phrasing has to
    // say "nothing orders this", not "this has no inputs".
    assert!(out.contains("free to start immediately"), "{out}");
    // The header states the run-wide verdict up front.
    assert!(out.contains("2 hit, 1 rebuild"), "{out}");
}

#[test]
fn dot_and_json_render_the_same_edge_set() {
    let g = agg(Level::Recipe);
    let dot = render(&g, Format::Dot);
    assert!(dot.starts_with("digraph cook {"), "{dot}");
    assert!(dot.contains("penwidth=3"), "barrier should be heavy: {dot}");

    let json = render(&g, Format::Json);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(parsed["level"], "recipe");
    assert_eq!(parsed["edges"].as_array().unwrap().len(), g.edges.len());
    assert_eq!(parsed["edges"][0]["kind"], "barrier");
}

/// The machine surface has to carry the tallies, not just the shape — it is
/// the successor to both payloads, not just the graph one.
#[test]
fn json_carries_the_cache_and_timing_tallies() {
    let g = agg(Level::Recipe);
    let parsed: serde_json::Value = serde_json::from_str(&render(&g, Format::Json)).unwrap();
    let bin = parsed["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "recipe:bin")
        .unwrap();
    assert_eq!(bin["hits"], 0);
    assert_eq!(bin["rebuilds"], 1);
    assert_eq!(bin["observed_ms"], 2100);
    assert_eq!(bin["unobserved"], 0);
    assert_eq!(bin["unclassified"], 0);
}
