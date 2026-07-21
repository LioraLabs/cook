//! Machine-readable JSON-lines event writer.
//!
//! Transforms the internal ProgressEvent into a spec-conformant JSON shape:
//! - Duration fields become integer `*_ms` fields.
//! - RecipeId / NodeId fields become human-readable names via BuildState lookup.

use std::io::{self, Write};

use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::event::{NodeKind, ProgressEvent, Stream, PROGRESS_SCHEMA_VERSION};
use crate::model::build::BuildState;
use crate::render::Renderer;

pub struct JsonWriter<W: Write + Send> {
    out: W,
    schema_version: u32,
}

impl<W: Write + Send> JsonWriter<W> {
    pub fn new(out: W) -> Self { Self { out, schema_version: PROGRESS_SCHEMA_VERSION } }

    fn now_rfc3339() -> String {
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
    }
}

fn duration_ms(d: std::time::Duration) -> u64 {
    d.as_millis() as u64
}

fn recipe_name(state: &BuildState, id: crate::event::RecipeId) -> String {
    state.recipes.get(&id).map(|r| r.name.clone()).unwrap_or_else(|| format!("recipe#{}", id.raw()))
}

fn node_name(state: &BuildState, recipe: crate::event::RecipeId, node: crate::event::NodeId) -> String {
    state
        .recipes
        .get(&recipe)
        .and_then(|r| r.nodes.get(&node))
        .map(|n| n.name.clone())
        .unwrap_or_else(|| format!("node#{}", node.raw()))
}

fn stream_str(s: Stream) -> &'static str {
    match s { Stream::Stdout => "stdout", Stream::Stderr => "stderr" }
}

/// Wire-format mapping for `NodeKind` on `node-started` / `node-completed`.
///
/// Mirrors the `#[serde(rename_all = "kebab-case")]` on the enum, but kept
/// explicit so the wire shape is visible at the emit site and doesn't drift
/// silently if the serde attribute changes.
fn kind_str(k: &NodeKind) -> &'static str {
    match k {
        NodeKind::Compile => "compile",
        NodeKind::Link => "link",
        NodeKind::Resolve => "resolve",
        NodeKind::Generate => "generate",
        NodeKind::Write => "write",
        NodeKind::Test => "test",
        NodeKind::Cooked => "cooked",
    }
}

/// Build the JSON value for a ProgressEvent, looking up names from BuildState.
pub(crate) fn event_to_value(state: &BuildState, event: &ProgressEvent) -> Value {
    match event {
        ProgressEvent::BuildStarted { recipes, total_nodes } => {
            let recipe_entries: Vec<Value> = recipes.iter().map(|r| json!({
                "name": r.name,
                "deps": r.deps.iter().map(|d| recipe_name(state, *d)).collect::<Vec<_>>(),
                "expected_nodes": r.expected_nodes,
            })).collect();
            json!({
                "type": "build-started",
                "recipes": recipe_entries,
                "total_nodes": total_nodes,
            })
        }
        ProgressEvent::RecipeStarted { recipe } => json!({
            "type": "recipe-started",
            "recipe": recipe_name(state, *recipe),
        }),
        ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total, kind } => json!({
            "type": "recipe-completed",
            "recipe": recipe_name(state, *recipe),
            "elapsed_ms": duration_ms(*elapsed),
            "cached": cached,
            "total": total,
            "kind": match kind {
                crate::event::RecipeKind::Recipe => "recipe",
                crate::event::RecipeKind::Chore => "chore",
            },
        }),
        ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => json!({
            "type": "recipe-failed",
            "recipe": recipe_name(state, *recipe),
            "elapsed_ms": duration_ms(*elapsed),
            "completed": completed,
            "total": total,
        }),
        ProgressEvent::RecipeSkipped { recipe, elapsed, skipped, completed, total } => json!({
            "type": "recipe-skipped",
            "recipe": recipe_name(state, *recipe),
            "elapsed_ms": duration_ms(*elapsed),
            "skipped": skipped,
            "completed": completed,
            "total": total,
            "reason": "upstream-failed",
        }),
        ProgressEvent::NodeStarted { recipe, node, name: _, artifact, fallback_label, kind, cause } => json!({
            "type": "node-started",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "artifact": artifact.as_ref().map(|p| p.display().to_string()),
            "fallback_label": fallback_label,
            "kind": kind_str(kind),
            "cause": cause,
        }),
        ProgressEvent::NodeCompleted { recipe, node, elapsed, kind } => json!({
            "type": "node-completed",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "elapsed_ms": duration_ms(*elapsed),
            "kind": kind_str(kind),
        }),
        ProgressEvent::NodeFailed { recipe, node, elapsed, error } => json!({
            "type": "node-failed",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "elapsed_ms": duration_ms(*elapsed),
            "error": error,
        }),
        ProgressEvent::NodeCacheHit { recipe, node, name: _, artifact, kind } => json!({
            "type": "node-cache-hit",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "artifact": artifact.as_ref().map(|p| p.display().to_string()),
            "kind": kind_str(kind),
        }),
        ProgressEvent::NodeSkipped { recipe, node, name: _, reason } => json!({
            "type": "node-skipped",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "reason": reason.as_str(),
        }),
        ProgressEvent::NodeOutput { recipe, node, line, stream } => json!({
            "type": "node-output",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "stream": stream_str(*stream),
            "line": line,
        }),
        ProgressEvent::InteractiveStart { recipe, node, name: _, chore_step_count } => json!({
            "type": "interactive-start",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "chore_step_count": chore_step_count,
        }),
        ProgressEvent::InteractiveEnd { recipe, node, name: _, elapsed, success, is_terminal: _, failed_step } => json!({
            "type": "interactive-end",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "elapsed_ms": duration_ms(*elapsed),
            "success": success,
            "failed_step": failed_step,
        }),
        ProgressEvent::Finished { success } => json!({
            "type": "finished",
            "success": success,
        }),
    }
}

impl<W: Write + Send> Renderer for JsonWriter<W> {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        let mut payload = event_to_value(state, event);
        // `events.jsonl` keys are emitted in **lexicographic (alphabetical)**
        // order, not insertion order. `serde_json::Map` is `BTreeMap`-backed
        // (no `preserve_order` feature in this crate), so a `build-started`
        // line surfaces as `{"recipes":…,"total_nodes":…,"ts":…,"type":…,"v":…}`,
        // and every other event interleaves `ts`/`type`/`v` with its payload
        // fields in lex order. This is intentional: keys are sorted so the
        // schema is stable for downstream consumers (diff-friendly, no churn
        // from event-shape edits).
        //
        // CS-0048: the `v` field is the wire-format schema version. Writers
        // emit `PROGRESS_SCHEMA_VERSION`; readers refuse lines whose `v`
        // exceeds the highest version they recognise. Evolution is additive-
        // only (new fields without a bump); incompatible changes bump `v`.
        let mut obj = serde_json::Map::new();
        obj.insert("ts".into(), Value::String(Self::now_rfc3339()));
        obj.insert("v".into(), Value::from(self.schema_version));
        if let Value::Object(inner) = payload.take() {
            for (k, v) in inner {
                obj.insert(k, v);
            }
        }
        serde_json::to_writer(&mut self.out, &Value::Object(obj)).map_err(io::Error::other)?;
        self.out.write_all(b"\n")
    }

    fn finish(&mut self, _state: &BuildState) -> io::Result<()> {
        self.out.flush()
    }
}

/// Errors raised by [`peek_schema_version`] / [`check_schema_version`].
#[derive(Debug)]
pub enum SchemaCheckError {
    /// The line is not parseable JSON.
    InvalidJson(String),
    /// The line parses but is not a JSON object.
    NotAnObject,
    /// The `v` field is missing or not an integer.
    MissingVersion,
    /// The `v` field exceeds the highest version this build recognises.
    Unsupported { found: u32, max_known: u32 },
}

impl std::fmt::Display for SchemaCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(e) => write!(f, "invalid JSON: {e}"),
            Self::NotAnObject => write!(f, "events.jsonl line is not a JSON object"),
            Self::MissingVersion => write!(f, "events.jsonl line missing required `v` schema-version field"),
            Self::Unsupported { found, max_known } => write!(
                f,
                "events.jsonl schema version {found} exceeds maximum supported version {max_known}; upgrade required"
            ),
        }
    }
}

impl std::error::Error for SchemaCheckError {}

/// Inspect a single `events.jsonl` line and return its `v` schema version
/// after enforcing the [CS-0048] read policy: refuse `v > PROGRESS_SCHEMA_VERSION`.
///
/// The full event payload is intentionally not deserialized — readers that
/// need only to validate the envelope can call this without reifying a
/// `ProgressEvent`. Lines whose `v` is at or below `PROGRESS_SCHEMA_VERSION`
/// are accepted (additive-only evolution within a major version).
pub fn check_schema_version(line: &str) -> Result<u32, SchemaCheckError> {
    let value: Value = serde_json::from_str(line)
        .map_err(|e| SchemaCheckError::InvalidJson(e.to_string()))?;
    let obj = value.as_object().ok_or(SchemaCheckError::NotAnObject)?;
    let v = obj
        .get("v")
        .and_then(|x| x.as_u64())
        .ok_or(SchemaCheckError::MissingVersion)?;
    let v = v as u32;
    if v > PROGRESS_SCHEMA_VERSION {
        return Err(SchemaCheckError::Unsupported {
            found: v,
            max_known: PROGRESS_SCHEMA_VERSION,
        });
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{NodeId, NodeKind, RecipeId, RecipeTopo};
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
            });
        let s_started = write_event(&state, &ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "ignored-inline-name".into(), artifact: None, fallback_label: "x".into(),
            kind: NodeKind::Cooked,
                cause: None,
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
}
