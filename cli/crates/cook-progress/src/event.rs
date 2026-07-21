//! Public event API for the progress pipeline.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Wire-format schema version for `events.jsonl` lines and the equivalent
/// `--output=json` stream. Pinned at the v1.0 cut by [CS-0048].
///
/// The on-wire field is the top-level integer `v` (kept flat in the lex-key
/// envelope from [CS-0035]); writers MUST emit `v = PROGRESS_SCHEMA_VERSION`
/// and readers MUST refuse lines whose `v` exceeds the highest version they
/// recognise. Evolution is additive-only — new fields are introduced without
/// bumping `v`; an incompatible structural change bumps `v` to 2 and is
/// documented in App. D.
///
/// [CS-0048]: https://example.invalid/cs-0048
/// [CS-0035]: https://example.invalid/cs-0035
pub const PROGRESS_SCHEMA_VERSION: u32 = 1;

/// Opaque recipe identifier. Stable within a single build run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RecipeId(u32);

impl RecipeId {
    pub fn new(raw: u32) -> Self { Self(raw) }
    pub fn raw(self) -> u32 { self.0 }
}

/// Opaque node identifier. Unique within a recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId(u32);

impl NodeId {
    pub fn new(raw: u32) -> Self { Self(raw) }
    pub fn raw(self) -> u32 { self.0 }
}

/// Why a node was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipReason {
    UpstreamFailed,
    ConditionFalse,
    Disabled,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpstreamFailed => "upstream-failed",
            Self::ConditionFalse => "condition-false",
            Self::Disabled => "disabled",
        }
    }
}

/// Which stream a node-output line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Distinguishes a regular recipe from a chore. Used by the renderer to
/// pick the recipe-summary detail string (`(N nodes)` vs `(chore)`).
/// Defaults to `Recipe` so older readers / tests round-trip unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecipeKind {
    #[default]
    Recipe,
    Chore,
}

/// What kind of work a node is doing. Determines which verb the renderer prints
/// (`Compiled`, `Linked`, `Tested`, …). Engine and Lua-stdlib producers fill
/// this in; unannotated nodes default to `Cooked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    Compile,
    Link,
    Resolve,
    Generate,
    Write,
    Test,
    #[default]
    Cooked,
}

/// Topology entry sent once in `BuildStarted`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeTopo {
    pub id: RecipeId,
    pub name: String,
    pub deps: Vec<RecipeId>,
    pub expected_nodes: usize,
}

/// Event stream emitted by the engine and consumed by the driver.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ProgressEvent {
    BuildStarted {
        recipes: Vec<RecipeTopo>,
        total_nodes: usize,
    },
    RecipeStarted {
        recipe: RecipeId,
    },
    RecipeCompleted {
        recipe: RecipeId,
        elapsed: Duration,
        cached: usize,
        total: usize,
        #[serde(default)]
        kind: RecipeKind,
    },
    RecipeSkipped {
        recipe: RecipeId,
        elapsed: Duration,
        skipped: usize,
        completed: usize,
        total: usize,
    },
    RecipeFailed {
        recipe: RecipeId,
        elapsed: Duration,
        completed: usize,
        total: usize,
    },
    NodeStarted {
        recipe: RecipeId,
        node: NodeId,
        name: String,
        artifact: Option<PathBuf>,
        fallback_label: String,
        #[serde(default)]
        kind: NodeKind,
        /// COOK-276 warm re-run attribution: which determinant changed since
        /// the unit's previous fingerprint record. `None` for cold units.
        /// Additive field — schema v1 readers ignore it (`#[serde(default)]`).
        #[serde(default)]
        cause: Option<String>,
    },
    NodeCompleted {
        recipe: RecipeId,
        node: NodeId,
        elapsed: Duration,
        #[serde(default)]
        kind: NodeKind,
    },
    NodeFailed {
        recipe: RecipeId,
        node: NodeId,
        elapsed: Duration,
        error: String,
    },
    NodeCacheHit {
        recipe: RecipeId,
        node: NodeId,
        name: String,
        artifact: Option<PathBuf>,
        kind: NodeKind,
    },
    NodeSkipped {
        recipe: RecipeId,
        node: NodeId,
        name: String,
        reason: SkipReason,
    },
    NodeOutput {
        recipe: RecipeId,
        node: NodeId,
        line: String,
        stream: Stream,
    },
    InteractiveStart {
        recipe: RecipeId,
        node: NodeId,
        name: String,
        /// Number of body steps in this chore window. 0 for non-chore
        /// (legacy single-line) interactives. >=1 for a chore window.
        #[serde(default)]
        chore_step_count: usize,
    },
    InteractiveEnd {
        recipe: RecipeId,
        node: NodeId,
        name: String,
        elapsed: Duration,
        success: bool,
        /// True when this interactive was the final bit of work — renderers
        /// use this to skip resuming progress bars.
        is_terminal: bool,
        /// 1-indexed step number that failed inside the chore window;
        /// `None` on success or for non-chore interactives.
        #[serde(default)]
        failed_step: Option<usize>,
    },
    Finished {
        success: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_id_round_trips_through_eq_and_hash() {
        let a = RecipeId::new(0);
        let b = RecipeId::new(0);
        let c = RecipeId::new(1);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn progress_event_is_clone_and_send() {
        fn assert_clone_send<T: Clone + Send>() {}
        assert_clone_send::<ProgressEvent>();
    }

    #[test]
    fn skip_reason_display() {
        assert_eq!(SkipReason::UpstreamFailed.as_str(), "upstream-failed");
        assert_eq!(SkipReason::ConditionFalse.as_str(), "condition-false");
        assert_eq!(SkipReason::Disabled.as_str(), "disabled");
    }
}
