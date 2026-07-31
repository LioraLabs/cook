use super::*;
use crate::event::{NodeId, NodeKind, RecipeId, RecipeTopo, Stream};
use std::time::Duration;

fn make_state_with_one_recipe() -> BuildState {
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0), name: "deps".into(),
            deps: vec![], expected_nodes: 3,
        }],
        total_nodes: 3,
    });
    state
}

fn write_event(state: &BuildState, event: &ProgressEvent) -> String {
    let mut buf = Vec::new();
    {
        let mut w = JsonWriter::new(&mut buf);
        w.handle(state, event).unwrap();
    }
    String::from_utf8(buf).unwrap()
}

#[test]
fn build_started_uses_recipe_names() {
    let state = make_state_with_one_recipe();
    let s = write_event(&state, &ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0), name: "deps".into(),
            deps: vec![], expected_nodes: 3,
        }],
        total_nodes: 3,
    });
    assert!(s.contains("\"type\":\"build-started\""), "got: {s}");
    assert!(s.contains("\"v\":1"), "got: {s}");
    assert!(s.contains("\"ts\":"), "got: {s}");
}

#[test]
fn recipe_completed_uses_elapsed_ms_integer() {
    let mut state = make_state_with_one_recipe();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    let s = write_event(&state, &ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(1234),
        cached: 0, total: 3,
        kind: crate::event::RecipeKind::Recipe,
    });
    assert!(s.contains("\"elapsed_ms\":1234"), "expected elapsed_ms integer; got: {s}");
    assert!(s.contains("\"recipe\":\"deps\""), "expected name not id; got: {s}");
}

#[test]
fn node_output_uses_names_and_stream_string() {
    let mut state = make_state_with_one_recipe();
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "lvm.c".into(), artifact: None, fallback_label: "x".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        });
    let s = write_event(&state, &ProgressEvent::NodeOutput {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        line: "warning: unused".into(), stream: Stream::Stderr,
    });
    assert!(s.contains("\"recipe\":\"deps\""), "got: {s}");
    assert!(s.contains("\"node\":\"lvm.c\""), "got: {s}");
    assert!(s.contains("\"stream\":\"stderr\""), "got: {s}");
}

#[test]
fn keys_are_emitted_in_lexicographic_order() {
    // Pins the wire-format guarantee documented on `JsonWriter::handle`:
    // keys are emitted in alphabetical order, not insertion order.
    let state = make_state_with_one_recipe();
    let s = write_event(&state, &ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0), name: "deps".into(),
            deps: vec![], expected_nodes: 3,
        }],
        total_nodes: 3,
    });
    let key_order: Vec<&str> = ["recipes", "total_nodes", "ts", "type", "v"]
        .iter()
        .map(|k| *k)
        .collect();
    let positions: Vec<(usize, &str)> = key_order
        .iter()
        .map(|k| {
            let needle = format!("\"{k}\":");
            (s.find(&needle).unwrap_or_else(|| panic!("missing key {k}; got: {s}")), *k)
        })
        .collect();
    let mut sorted = positions.clone();
    sorted.sort_by_key(|p| p.0);
    assert_eq!(positions, sorted, "keys must appear in lex order; got: {s}");
}

#[test]
fn node_event_node_field_resolves_via_state() {
    // CS-0035: every event with a `node` field resolves it through the
    // BuildState lookup, not the inline `name` carried by some variants.
    let mut state = make_state_with_one_recipe();
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "lvm.c".into(), artifact: None, fallback_label: "x".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        });
    let s_started = write_event(&state, &ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "ignored-inline-name".into(), artifact: None, fallback_label: "x".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        });
    assert!(s_started.contains("\"node\":\"lvm.c\""),
        "node-started must read state, not inline name; got: {s_started}");
    let s_skipped = write_event(&state, &ProgressEvent::NodeSkipped {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "ignored-inline-name".into(), reason: crate::event::SkipReason::Disabled,
    });
    assert!(s_skipped.contains("\"node\":\"lvm.c\""),
        "node-skipped must read state, not inline name; got: {s_skipped}");
}

#[test]
fn node_field_falls_back_to_synthesized_id_when_unknown() {
    // Out-of-order arrivals (a NodeCompleted before its NodeStarted, e.g.
    // a renderer wired into a replay) get a stable synthesized label
    // rather than a missing field.
    let state = make_state_with_one_recipe();
    let s = write_event(&state, &ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(7),
        elapsed: Duration::from_millis(1),
        kind: NodeKind::Cooked,
        cache_key: None,
    });
    assert!(s.contains("\"node\":\"node#7\""),
        "expected synthesized fallback; got: {s}");
}

#[test]
fn each_event_is_one_line() {
    let state = make_state_with_one_recipe();
    let mut buf = Vec::new();
    {
        let mut w = JsonWriter::new(&mut buf);
        w.handle(&state, &ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) }).unwrap();
        w.handle(&state, &ProgressEvent::Finished { success: true }).unwrap();
    }
    let s = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 2, "expected 2 lines; got: {s}");
}

// --- NodeKind on the wire (additive `kind` field) ---

#[test]
fn node_started_emits_kind_in_wire_format() {
    let state = make_state_with_one_recipe();
    let s = write_event(&state, &ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "lvm.c".into(), artifact: None,
        fallback_label: "x".into(),
        kind: NodeKind::Compile,
            cause: None,
            cache_key: None,
        });
    assert!(s.contains("\"kind\":\"compile\""), "got: {s}");
}

#[test]
fn node_completed_emits_kind_in_wire_format() {
    let state = make_state_with_one_recipe();
    let s = write_event(&state, &ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: std::time::Duration::from_millis(100),
        kind: NodeKind::Link,
        cache_key: None,
    });
    assert!(s.contains("\"kind\":\"link\""), "got: {s}");
}

// --- CS-0048: schema-version envelope (`v` field) ---

#[test]
fn writer_emits_schema_version_constant() {
    let state = make_state_with_one_recipe();
    let s = write_event(&state, &ProgressEvent::Finished { success: true });
    let needle = format!("\"v\":{PROGRESS_SCHEMA_VERSION}");
    assert!(s.contains(&needle), "expected `{needle}`; got: {s}");
}

#[test]
fn check_schema_version_accepts_current_version() {
    let state = make_state_with_one_recipe();
    let line = write_event(&state, &ProgressEvent::Finished { success: true });
    let line = line.trim_end();
    let v = check_schema_version(line).expect("current line must validate");
    assert_eq!(v, PROGRESS_SCHEMA_VERSION);
}

#[test]
fn check_schema_version_accepts_lower_versions() {
    // CS-0048: readers accept any `v <= MAX_KNOWN`. Build a synthetic
    // v=0 line to pin the additive-only contract for the future v=2 case
    // (today MAX_KNOWN=1, so v=0 is the only "lower" value we can test).
        let line = r#"{"ts":"1970-01-01T00:00:00Z","type":"finished","success":true,"v":0}"#;
    let v = check_schema_version(line).expect("v <= MAX_KNOWN must validate");
    assert_eq!(v, 0);
}

#[test]
fn check_schema_version_rejects_higher_versions() {
    let line = r#"{"ts":"1970-01-01T00:00:00Z","type":"finished","success":true,"v":99}"#;
    let err = check_schema_version(line).expect_err("v > MAX_KNOWN must be refused");
    match err {
        SchemaCheckError::Unsupported { found, max_known } => {
            assert_eq!(found, 99);
            assert_eq!(max_known, PROGRESS_SCHEMA_VERSION);
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn check_schema_version_rejects_missing_v_field() {
    let line = r#"{"ts":"1970-01-01T00:00:00Z","type":"finished","success":true}"#;
    let err = check_schema_version(line).expect_err("missing `v` must be refused");
    assert!(matches!(err, SchemaCheckError::MissingVersion));
}

#[test]
fn check_schema_version_rejects_invalid_json() {
    let err = check_schema_version("{not json").expect_err("garbage must be refused");
    assert!(matches!(err, SchemaCheckError::InvalidJson(_)));
}
