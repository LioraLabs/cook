//! Machine-readable JSON-lines event writer.
//!
//! Transforms the internal ProgressEvent into a spec-conformant JSON shape:
//! - Duration fields become integer `*_ms` fields.
//! - RecipeId / NodeId fields become human-readable names via BuildState lookup.

use std::io::{self, Write};

use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::event::{ProgressEvent, SkipReason, PROGRESS_SCHEMA_VERSION};
use crate::wire::{WireEvent, WireLine, WireRecipeEntry};
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

/// Build the wire event for a ProgressEvent, looking up names from
/// BuildState. The wire SHAPE lives in [`crate::wire::WireEvent`] — this
/// function only resolves ids to names and durations to ms (COOK-394; the
/// ~40 hand-spelled `json!` key literals died with it).
pub(crate) fn event_to_wire(state: &BuildState, event: &ProgressEvent) -> WireEvent {
    match event {
        ProgressEvent::BuildStarted { recipes, total_nodes } => WireEvent::BuildStarted {
            recipes: recipes.iter().map(|r| WireRecipeEntry {
                name: r.name.clone(),
                deps: r.deps.iter().map(|d| recipe_name(state, *d)).collect(),
                expected_nodes: r.expected_nodes,
            }).collect(),
            total_nodes: *total_nodes,
        },
        ProgressEvent::RecipeStarted { recipe } => WireEvent::RecipeStarted {
            recipe: recipe_name(state, *recipe),
        },
        ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total, kind } => {
            WireEvent::RecipeCompleted {
                recipe: recipe_name(state, *recipe),
                elapsed_ms: duration_ms(*elapsed),
                cached: *cached,
                total: *total,
                kind: *kind,
            }
        }
        ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => {
            WireEvent::RecipeFailed {
                recipe: recipe_name(state, *recipe),
                elapsed_ms: duration_ms(*elapsed),
                completed: *completed,
                total: *total,
            }
        }
        ProgressEvent::RecipeSkipped { recipe, elapsed, skipped, completed, total } => {
            WireEvent::RecipeSkipped {
                recipe: recipe_name(state, *recipe),
                elapsed_ms: duration_ms(*elapsed),
                skipped: *skipped,
                completed: *completed,
                total: *total,
                // A recipe is only ever skipped for a failed upstream —
                // typed now, where the writer used to hardcode the string
                // beside SkipReason::as_str.
                reason: SkipReason::UpstreamFailed,
            }
        }
        ProgressEvent::NodeStarted { recipe, node, name: _, artifact, fallback_label, kind, cause, cache_key } => {
            WireEvent::NodeStarted {
                recipe: recipe_name(state, *recipe),
                node: node_name(state, *recipe, *node),
                artifact: artifact.as_ref().map(|p| p.display().to_string()),
                fallback_label: fallback_label.clone(),
                kind: *kind,
                cause: cause.clone(),
                cache_key: cache_key.clone(),
            }
        }
        ProgressEvent::NodeCompleted { recipe, node, elapsed, kind, cache_key } => {
            WireEvent::NodeCompleted {
                recipe: recipe_name(state, *recipe),
                node: node_name(state, *recipe, *node),
                elapsed_ms: duration_ms(*elapsed),
                kind: *kind,
                // CS-0171: the join key `cook why` reads retained timings
                // back by. `node` is a display name and collides across
                // units. None for a non-cacheable node.
                cache_key: cache_key.clone(),
            }
        }
        ProgressEvent::NodeFailed { recipe, node, elapsed, error } => WireEvent::NodeFailed {
            recipe: recipe_name(state, *recipe),
            node: node_name(state, *recipe, *node),
            elapsed_ms: duration_ms(*elapsed),
            error: error.clone(),
        },
        ProgressEvent::NodeCacheHit { recipe, node, name: _, artifact, kind } => {
            WireEvent::NodeCacheHit {
                recipe: recipe_name(state, *recipe),
                node: node_name(state, *recipe, *node),
                artifact: artifact.as_ref().map(|p| p.display().to_string()),
                kind: *kind,
            }
        }
        ProgressEvent::NodeSkipped { recipe, node, name: _, reason } => WireEvent::NodeSkipped {
            recipe: recipe_name(state, *recipe),
            node: node_name(state, *recipe, *node),
            reason: *reason,
        },
        ProgressEvent::NodeOutput { recipe, node, line, stream } => WireEvent::NodeOutput {
            recipe: recipe_name(state, *recipe),
            node: node_name(state, *recipe, *node),
            stream: *stream,
            line: line.clone(),
        },
        ProgressEvent::InteractiveStart { recipe, node, name: _, chore_step_count } => {
            WireEvent::InteractiveStart {
                recipe: recipe_name(state, *recipe),
                node: node_name(state, *recipe, *node),
                chore_step_count: *chore_step_count,
            }
        }
        ProgressEvent::InteractiveEnd { recipe, node, name: _, elapsed, success, is_terminal: _, failed_step } => {
            WireEvent::InteractiveEnd {
                recipe: recipe_name(state, *recipe),
                node: node_name(state, *recipe, *node),
                elapsed_ms: duration_ms(*elapsed),
                success: *success,
                failed_step: *failed_step,
            }
        }
        ProgressEvent::Finished { success } => WireEvent::Finished { success: *success },
    }
}

impl<W: Write + Send> Renderer for JsonWriter<W> {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        let line = WireLine {
            ts: Self::now_rfc3339(),
            v: self.schema_version,
            event: event_to_wire(state, event),
        };
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
        // Through `to_value` so the BTreeMap-backed Map does the sorting;
        // `WireLine`'s flatten merges ts/v with the tagged payload.
        let value = serde_json::to_value(&line).map_err(io::Error::other)?;
        serde_json::to_writer(&mut self.out, &value).map_err(io::Error::other)?;
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
#[path = "tests/json_tests.rs"]
mod tests;
