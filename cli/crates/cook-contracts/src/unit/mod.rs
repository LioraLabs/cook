//! Captured work payloads and unit dependency shape.

use crate::{CacheMeta, StepKind};
use std::collections::BTreeMap;

/// What kind of work a node is doing. Determines which verb a renderer prints
/// (`Compiled`, `Linked`, `Tested`, …); unannotated nodes default to `Cooked`.
///
/// One definition, because two crates must agree on it and disagreement is a
/// bug rather than a preference (COOK-421). The engine produces this on its
/// event stream, `cook-progress` renders it and writes it into `.cook/logs`,
/// and `cook-logs` reads it back — so the serde spelling below is a wire
/// format, not a rendering detail.
///
/// It used to be declared once per crate with a hand-written translation in
/// cook-cli joining them, justified by keeping cook-engine free of a
/// cook-progress dependency. The stratum rule answers that without a mirror:
/// both crates already depend on this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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

/// What kind of work a captured unit represents.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum WorkPayload {
    Shell {
        cmd: String,
        line: usize,
    },
    Interactive {
        cmd: String,
        line: usize,
        /// True when this unit was emitted inside a chore body (between
        /// `cook._enter_chore()` and `cook._exit_chore()`). Drives the
        /// engine's chore-window grouping in `cook-engine/src/executor.rs`.
        /// False for `interactive = true` shell steps inside a regular
        /// recipe (the legacy single-line interactive path).
        is_chore: bool,
    },
    LuaChunk {
        code: String,
        inputs: Vec<String>,
        outputs: Vec<String>,
        ingredient_groups: Vec<Vec<String>>,
        /// Originating step kind, used by the execute-phase worker
        /// to pick a [`crate::StepKind`]-appropriate sandbox policy
        /// (CS-0045). Older code paths that did not yet plumb the
        /// kind capture `Cook` here as the safe default — cook-step
        /// confinement is the strictest contract and a misclassified
        /// plate body merely degrades to a Lua runtime error rather
        /// than silently writing outside the project.
        step_kind: StepKind,
        /// Set by `_enter_chore`/`_exit_chore`; routes the unit to the
        /// chore-window drain in cook-engine instead of the worker pool.
        is_chore: bool,
        /// 1-indexed Cookfile line of the originating step; 0 = unknown.
        /// Purely a diagnostics aid (COOK-191/CS-0126): the execute-phase
        /// worker (cook-execute/src/pool.rs) newline-pads `code` so that a
        /// Lua error inside the chunk reports `Cookfile:LINE:` instead of
        /// the opaque `[string "..."]:1:` chunk name. This field MUST NOT
        /// be folded into any cache fingerprint — unit identity is hashed
        /// from `code`/`command` text directly (cook-register/src/unit_api.rs
        /// `command_hash`), never by serialising the whole `WorkPayload`.
        line: usize,
    },
    // What this payload no longer carries (CS-0186): `input_paths`, `seal_keys`
    // and `consumes`. All three were here because a test unit's cache lived
    // outside `CacheMeta` and a separate ready-time fingerprint read them off
    // the payload. A test unit now carries a `CacheMeta` like every other unit,
    // which is where those three facts belong and where they are read from, so
    // keeping payload copies would be two answers to one question — and the
    // copies were the ones nothing consulted.
    /// A probe unit (§22.5.2): runs `produce` (Lua source string) on a worker
    /// VM and stashes the canonical-JSON-serialised return value under `key`.
    Probe {
        key: String,
        produce: String,
        line: usize,
    },
}

impl WorkPayload {
    /// Does running this payload evaluate author Lua on a VM (§12.3, CS-0204)?
    ///
    /// Which is the same question as: can this unit load a module? A payload
    /// that spawns a process cannot — the module surface is a Lua API, and a
    /// spawned command reaches it through no door at all. The distinction
    /// decides whether the cold-fetch path consults the module manifest, so it
    /// is asked here rather than pattern-matched at each site that needs it.
    ///
    /// `Interactive` is deliberately false: it spawns, and it is uncacheable
    /// besides.
    pub fn evaluates_lua(&self) -> bool {
        matches!(self, Self::LuaChunk { .. } | Self::Probe { .. })
    }

    /// Human-readable name for progress UI and result reporting.
    pub fn display_name(&self) -> String {
        match self {
            Self::Shell { cmd, .. } => {
                // COOK-391: strip exactly the compose() prelude — the law's
                // inverse — instead of filtering any `set -e` LINE anywhere
                // (which mislabeled a body whose own text contains one).
                let stripped = crate::shell_block::strip_set_e(cmd);
                let body = stripped
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    // Degenerate body (empty, or nothing but the preamble):
                    // fall back so callers surfacing this label never get a
                    // blank string.
                    .or_else(|| cmd.lines().map(str::trim).find(|l| !l.is_empty()))
                    .unwrap_or("sh");
                if body.len() <= 60 {
                    body.to_string()
                } else {
                    format!("{}...", body.chars().take(57).collect::<String>())
                }
            }
            Self::LuaChunk { .. } => "lua".to_string(),
            Self::Interactive { line, .. } => format!("@{line}"),
            Self::Probe { key, .. } => probe_label(key),
        }
    }

    /// The 1-indexed Cookfile line this unit came from; `0` when unknown.
    ///
    /// Every variant carries one, and four call sites used to match on the
    /// payload kind purely to reach it — one of which knew only about `Test`
    /// and reported 0 for everything else.
    /// The match is deliberately exhaustive with no `_` arm: a fifth variant
    /// must be a compile error here, not a silent `0`. The arm this replaced
    /// was already unreachable and warned on every build.
    pub fn line(&self) -> usize {
        match self {
            Self::Shell { line, .. }
            | Self::Interactive { line, .. }
            | Self::LuaChunk { line, .. }
            | Self::Probe { line, .. } => *line,
        }
    }
}

/// The display label of a probe unit (`probe:<key>`). Composed here and
/// parsed by [`parse_probe_label`] — cook-progress detected and split this
/// label by hand at two sites, which worked only because probe keys are
/// canonically `ns:name`; the pair states the format once (COOK-392).
pub const PROBE_LABEL_PREFIX: &str = "probe:";

/// Compose a probe unit's display label.
pub fn probe_label(key: &str) -> String {
    format!("{PROBE_LABEL_PREFIX}{key}")
}

/// Parse a display label back to its probe key, or `None` if the label is
/// not a probe unit's.
pub fn parse_probe_label(label: &str) -> Option<&str> {
    label.strip_prefix(PROBE_LABEL_PREFIX)
}

/// A single captured unit of work within a recipe.
#[derive(Debug, Clone)]
pub struct CapturedUnit {
    pub payload: WorkPayload,
    pub cache_meta: Option<CacheMeta>,
    pub dep_kind: DepKind,
    /// Probe keys this unit consumes (§22.5.5). Empty for non-consumer units.
    pub probes: Vec<String>,
    /// Per-unit environment variables that override the recipe-level env vars.
    /// Used by chore shell units to export bound param values (COOK-36 §7.1.2).
    /// Empty for non-chore units and chores without parameters.
    pub unit_env_vars: BTreeMap<String, String>,
    /// COOK-96: the canonical member string (`cook.member_to_string`) for a
    /// fan-out unit, or `None` for a non-fan-out unit. Lets the engine build
    /// the per-member output map that `$<recipe[in]>` joins on
    /// (COOK-221/CS-0137).
    pub member: Option<String>,
    /// COOK-96: this unit's declared output paths, retained so the engine can
    /// key them by `member` for the per-member map.
    pub output_paths: Vec<String>,
    /// CS-0219: declared output paths of units this one must run after, within
    /// the same recipe. Each entry names a path some EARLIER unit of the same
    /// recipe declares in its own [`Self::output_paths`]; the reference is
    /// resolved to a unit index by
    /// [`crate::unit_graph::resolve_after`], and it contributes a per-unit
    /// ordering edge and nothing else.
    ///
    /// It is a declaration, not an inference. §10.6 forbids reading an edge out
    /// of equality between an `inputs[]` entry and some other unit's
    /// `outputs[]` entry, because a coincidence of spelling is not evidence the
    /// author meant an ordering; an entry here IS that evidence, and carries no
    /// other content. Nothing here folds into a cache key: ordering is not an
    /// input, so a unit whose edge set moved while its command, inputs, outputs
    /// and member held still cannot produce different bytes (the same rule
    /// §22.11 states for `cook.dep_order`).
    pub after: Vec<String>,
    /// CS-0191: a test unit's reporting name, and the fact that it IS one.
    ///
    /// `Some(name)` marks a unit the test reporter names, counts and renders as
    /// a test; `None` is every other unit. It is deliberately the only thing
    /// left distinguishing a test, and it is presentation: CS-0185 made a test
    /// an ordinary unit at registration, CS-0186 made it an ordinary unit at
    /// the cache, and CS-0191 finishes the sentence at the runner. What remains
    /// is a name to report it under, which is not a payload's business —
    /// `WorkPayload::Test` carried it alongside a `timeout` that never fired,
    /// a `should_fail` that was never set, and an `iteration_item` that
    /// duplicated [`Self::member`].
    pub test_name: Option<String>,
}

/// How a captured unit relates to others in the recipe.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DepKind {
    /// Part of a step group (can run parallel with siblings in the group).
    ///
    /// COOK-360: there was a second variant, `TestSibling(usize)`, described as
    /// "like StepGroup but failures don't cancel siblings". It encoded a
    /// conjunction of two independent facts — the grouping, and that the
    /// members were tests — and enforced nothing. Group members all depend on
    /// the same barrier and never on each other, so a sibling is never a
    /// dependent and the cancellation walk cannot reach one; the exemption
    /// CS-0177 states is a property of the graph's shape, not of this enum.
    /// Grouping is this variant; test-ness is [`crate::StepKind::Test`].
    StepGroup(usize),
    /// Sequential barrier (depends on all prior units in recipe).
    Sequential,
}

impl DepKind {
    /// The name this relationship is rendered under in the graph JSON.
    ///
    /// It lives with the declaration for the reason COOK-421 gave when it
    /// retired the `RecipeKind` mirror: the renderer's vocabulary was never its
    /// own, it was the declaration's, copied. `cook-graph` used to spell these
    /// strings itself in a `match` on this enum, and `#[non_exhaustive]` forced
    /// that match to carry a `_ => "unknown"` arm — so a variant added here
    /// left the renderer compiling and quietly labelling the new kind
    /// `unknown`. Here the match is exhaustive, because `#[non_exhaustive]`
    /// does not apply inside the defining crate: a new variant is a compile
    /// error at the one site that has to name it, which is the whole reason to
    /// keep the rendering next to the declaration.
    ///
    /// [`StepGroup`](Self::StepGroup) renders as the door that produced it:
    /// a unit is in a step group because a recipe body called
    /// `cook.step_group`, so the label is
    /// [`crate::registration::STEP_GROUP_NAME`] rather than a second spelling
    /// of it. That was the constitution's "three ends, no definition" finding
    /// — both VMs and the renderer — and this is the definition.
    pub fn wire_name(&self) -> &'static str {
        match self {
            DepKind::StepGroup(_) => crate::registration::STEP_GROUP_NAME,
            DepKind::Sequential => "sequential",
        }
    }
}

#[cfg(test)]
#[path = "tests/dep_kind_tests.rs"]
mod dep_kind_tests;
