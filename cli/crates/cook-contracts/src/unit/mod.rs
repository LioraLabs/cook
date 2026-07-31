//! Captured work payloads and unit dependency shape.

use crate::{CacheMeta, StepKind};
use std::collections::BTreeMap;

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
        /// worker (cook-luaotp/src/pool.rs) newline-pads `code` so that a
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
            Self::Probe { key, .. } => format!("probe:{key}"),
        }
    }

    /// The 1-indexed Cookfile line this unit came from; `0` when unknown.
    ///
    /// Every variant carries one, and four call sites used to match on the
    /// payload kind purely to reach it — one of which knew only about `Test`
    /// and reported 0 for everything else.
    pub fn line(&self) -> usize {
        match self {
            Self::Shell { line, .. }
            | Self::Interactive { line, .. }
            | Self::LuaChunk { line, .. }
            | Self::Probe { line, .. } => *line,
            _ => 0,
        }
    }
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
