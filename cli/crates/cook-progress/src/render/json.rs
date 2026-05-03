//! Machine-readable JSON-lines event writer.
//!
//! Transforms the internal ProgressEvent into a spec-conformant JSON shape:
//! - Duration fields become integer `*_ms` fields.
//! - RecipeId / NodeId fields become human-readable names via BuildState lookup.

use std::io::{self, Write};

use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::event::{ProgressEvent, Stream};
use crate::model::build::BuildState;
use crate::render::Renderer;

pub struct JsonWriter<W: Write + Send> {
    out: W,
    schema_version: u32,
}

impl<W: Write + Send> JsonWriter<W> {
    pub fn new(out: W) -> Self { Self { out, schema_version: 1 } }

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
        ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total } => json!({
            "type": "recipe-completed",
            "recipe": recipe_name(state, *recipe),
            "elapsed_ms": duration_ms(*elapsed),
            "cached": cached,
            "total": total,
        }),
        ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => json!({
            "type": "recipe-failed",
            "recipe": recipe_name(state, *recipe),
            "elapsed_ms": duration_ms(*elapsed),
            "completed": completed,
            "total": total,
        }),
        ProgressEvent::NodeStarted { recipe, node, name: _, artifact, fallback_label } => json!({
            "type": "node-started",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "artifact": artifact.as_ref().map(|p| p.display().to_string()),
            "fallback_label": fallback_label,
        }),
        ProgressEvent::NodeCompleted { recipe, node, elapsed } => json!({
            "type": "node-completed",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "elapsed_ms": duration_ms(*elapsed),
        }),
        ProgressEvent::NodeFailed { recipe, node, elapsed, error } => json!({
            "type": "node-failed",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "elapsed_ms": duration_ms(*elapsed),
            "error": error,
        }),
        ProgressEvent::NodeCacheHit { recipe, node, name: _, artifact } => json!({
            "type": "node-cache-hit",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "artifact": artifact.as_ref().map(|p| p.display().to_string()),
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
        ProgressEvent::InteractiveStart { recipe, node, name: _ } => json!({
            "type": "interactive-start",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
        }),
        ProgressEvent::InteractiveEnd { recipe, node, name: _, elapsed, success, .. } => json!({
            "type": "interactive-end",
            "recipe": recipe_name(state, *recipe),
            "node": node_name(state, *recipe, *node),
            "elapsed_ms": duration_ms(*elapsed),
            "success": success,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{NodeId, RecipeId, RecipeTopo};
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
        });
        let s_started = write_event(&state, &ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "ignored-inline-name".into(), artifact: None, fallback_label: "x".into(),
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
}
