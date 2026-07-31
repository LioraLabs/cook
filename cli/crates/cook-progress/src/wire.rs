//! The `events.jsonl` wire schema — one serde-round-tripped type serving
//! BOTH halves (COOK-394).
//!
//! Until COOK-394 the format was two hand-written halves in this crate: the
//! writer built ~40 `json!` key literals, the reader hand-`.get()`ed the
//! same keys (plus a second independent parser for the failed-count scan),
//! `NodeKind` was respelled kebab-case by hand in the reader beside its
//! derive, `"upstream-failed"` was hardcoded beside `SkipReason::as_str`,
//! and reader defaults were silent (unknown kind → `Cooked`, unknown
//! stream → `Stdout`) — drift was silent misattribution in `cook logs`
//! replay and `--output=json` consumers.
//!
//! [`WireEvent`] is that schema. The derives on `NodeKind` / `SkipReason` /
//! `Stream` / `RecipeKind` do the enum spelling (they were dead for the
//! wire before); the writer serializes it, both readers deserialize it.
//!
//! **Envelope and evolution.** Each line is [`WireLine`]: `ts` + `v` +
//! the flattened event. `v` is `PROGRESS_SCHEMA_VERSION` (CS-0048): readers
//! refuse lines whose `v` exceeds what they recognise; evolution is
//! additive-only (new OPTIONAL fields — `#[serde(default)]` — and new event
//! types without a bump; incompatible changes bump `v`). Consequently:
//! unknown fields are ignored (no `deny_unknown_fields`), and an unknown
//! `type` tag from a NEWER writer within an accepted `v` is skipped as
//! unknown-event, not counted as corrupt.
//!
//! **Key order.** `events.jsonl` keys are lexicographic: the writer goes
//! through `serde_json::to_value` (BTreeMap-backed `Map`, no
//! `preserve_order`), which sorts — byte-compatible with the historical
//! hand-built lines.

use serde::{Deserialize, Serialize};

use crate::event::{NodeKind, RecipeKind, SkipReason, Stream};

/// One `events.jsonl` line: the envelope plus the event payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WireLine {
    /// RFC 3339 wall-clock timestamp of emission.
    pub ts: String,
    /// Wire-format schema version (CS-0048).
    pub v: u32,
    #[serde(flatten)]
    pub event: WireEvent,
}

/// A `build-started` topology entry (deps resolved to recipe names).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WireRecipeEntry {
    pub name: String,
    pub deps: Vec<String>,
    pub expected_nodes: usize,
}

/// The event payload of one `events.jsonl` line. Field and tag spellings
/// here ARE the wire format; nothing else spells them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum WireEvent {
    BuildStarted {
        recipes: Vec<WireRecipeEntry>,
        total_nodes: usize,
    },
    RecipeStarted {
        recipe: String,
    },
    RecipeCompleted {
        recipe: String,
        elapsed_ms: u64,
        cached: usize,
        total: usize,
        #[serde(default)]
        kind: RecipeKind,
    },
    RecipeFailed {
        recipe: String,
        elapsed_ms: u64,
        completed: usize,
        total: usize,
    },
    RecipeSkipped {
        recipe: String,
        elapsed_ms: u64,
        skipped: usize,
        completed: usize,
        total: usize,
        reason: SkipReason,
    },
    NodeStarted {
        recipe: String,
        node: String,
        artifact: Option<String>,
        fallback_label: String,
        #[serde(default)]
        kind: NodeKind,
        #[serde(default)]
        cause: Option<String>,
        #[serde(default)]
        cache_key: Option<String>,
    },
    NodeCompleted {
        recipe: String,
        node: String,
        elapsed_ms: u64,
        #[serde(default)]
        kind: NodeKind,
        /// CS-0171: the join key `cook why` reads retained timings back by.
        #[serde(default)]
        cache_key: Option<String>,
    },
    NodeFailed {
        recipe: String,
        node: String,
        elapsed_ms: u64,
        error: String,
    },
    NodeCacheHit {
        recipe: String,
        node: String,
        artifact: Option<String>,
        #[serde(default)]
        kind: NodeKind,
    },
    NodeSkipped {
        recipe: String,
        node: String,
        reason: SkipReason,
    },
    NodeOutput {
        recipe: String,
        node: String,
        stream: Stream,
        line: String,
    },
    InteractiveStart {
        recipe: String,
        node: String,
        #[serde(default)]
        chore_step_count: usize,
    },
    InteractiveEnd {
        recipe: String,
        node: String,
        elapsed_ms: u64,
        success: bool,
        #[serde(default)]
        failed_step: Option<usize>,
    },
    Finished {
        success: bool,
    },
}

#[cfg(test)]
#[path = "tests/wire_tests.rs"]
mod tests;
