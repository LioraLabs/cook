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
    Test {
        cmd: String,
        line: usize,
        timeout: u64,
        should_fail: bool,
        suite_name: String,
        test_name: String,
        iteration_item: Option<String>,
        /// CS-0127 §22.4: exactly one of `cmd` / `lua_code` is populated —
        /// `cmd` is a shell command run via `/bin/sh`, `lua_code` is a Lua
        /// chunk executed on an execute-phase worker VM under the `test`
        /// step-kind sandbox policy (identical to `Cook`, see [`StepKind`]).
        /// When `lua_code` is `Some`, `cmd` MUST be empty; pass/fail is the
        /// chunk completing without error / raising a Lua error, mirroring
        /// `should_fail`'s existing exit-code inversion semantics.
        lua_code: Option<String>,
        /// COOK-84: working-dir-relative paths of the files this test
        /// consumes — the recipe's resolved ingredients ∪ the step group's
        /// dep-output paths (mirrors `cache_input_paths` in
        /// cook-register/src/unit_api.rs). Carried on the payload, NOT via
        /// `cache_meta`: the executor relies on Test nodes having
        /// `cache_meta == None` (cook-engine/src/executor.rs:126/936/1213).
        /// Folded into the upfront test fingerprint by
        /// cook-engine/src/run.rs.
        input_paths: Vec<String>,
        /// CS-0159: the test unit's effective seal set — bare probe keys whose
        /// canonical VALUES fold into the test fingerprint (§17.4 rule 1), on
        /// the same footing as a cook unit's `CacheMeta::seal_keys`. Carried
        /// on the payload rather than via `cache_meta` for the same reason
        /// `input_paths` is: the executor relies on Test nodes having
        /// `cache_meta == None`. The register surface unions these keys into
        /// the unit's probe-dependency set, so every sealed value is
        /// materialised by the time the ready-time fingerprint is computed.
        seal_keys: std::collections::BTreeSet<String>,
        /// Glob allowlist narrowing which *predecessor outputs* fold into
        /// the ready-time fingerprint (§17.4 step 1). Empty — the default —
        /// folds every immediate-predecessor output, the historical
        /// behaviour.
        ///
        /// Exists because that fold is otherwise unnarrowable from the
        /// register surface, and over-folding is not merely slow: a
        /// dependency's `dist/` routinely carries artifacts no consumer
        /// reads, and one of them changing re-keys the check. Sourcemaps
        /// are the flagship case — tsup/esbuild inline `sourcesContent`,
        /// so a comment-only edit upstream rewrites `index.mjs.map` while
        /// `index.mjs` stays byte-identical, and every downstream check
        /// loses its cached pass for a file it never opened. Cook units
        /// have always had the equivalent control (they declare their own
        /// inputs, and `discovered_inputs` records what was actually
        /// read); this is the test unit's counterpart.
        ///
        /// Matching follows gitignore convention: a pattern containing no
        /// `/` matches a path's BASENAME at any depth (`*.d.ts`), one
        /// containing `/` matches the project-root-relative path
        /// (`packages/core/dist/**/*.mjs`).
        ///
        /// Narrowing a fingerprint is always a correctness risk in the
        /// under-keying direction, so this never silently folds nothing:
        /// when predecessor outputs exist and no pattern matches any of
        /// them, the engine keeps the unnarrowed set (§17.4 — a
        /// declaration that cannot be honoured must not quietly weaken a
        /// key).
        consumes: Vec<String>,
    },
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
                let body = cmd
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty() && *l != "set -e")
                    // Degenerate body (empty, or nothing but the `set -e`
                    // preamble): fall back to the first non-empty line so
                    // callers surfacing this label never get a blank string.
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
            Self::Test { test_name, .. } => test_name.clone(),
            Self::Probe { key, .. } => format!("probe:{key}"),
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
