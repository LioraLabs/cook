//! Shared kernel types for the Cook build system.
//!
//! This crate contains behavior-free structs and enums used across multiple
//! Cook crates. It has zero dependencies on other Cook crates.

pub mod member;
pub mod probe_value;

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Path accessor names admitted by `path.X(...)` and by placeholder
/// dotted-suffix forms (`{NAME.ACCESSOR}`, `$<NAME.ACCESSOR>`).
///
/// This constant is the single authoritative definition; every module that
/// validates accessor suffixes (resolver, dep_ref, recipe, template) MUST
/// import it from here rather than defining its own copy.
pub const ACCESSORS: &[&str] = &["stem", "name", "ext", "dir"];

/// Bare name of the codegen-private register-phase helper that records a
/// surface `recipe NAME` block (CS-0077 §6.4 implementation note).
///
/// Two consumers MUST agree on this string:
/// - `cook-luagen` emits `cook.{REGISTER_SURFACE_NAME}(...)` calls when
///   lowering a surface `recipe NAME` block.
/// - `cook-register` installs a closure under this name on the register-
///   phase `cook` Lua table.
///
/// Hoisted to this crate so renames are a single-file change rather than a
/// silent emit/install mismatch at Lua-load time.
pub const REGISTER_SURFACE_NAME: &str = "__register_surface";

/// Bare name of the codegen-private register-phase helper that records a
/// surface `chore NAME` block (CS-0077 §6.4 implementation note). Same
/// emit/install contract as [`REGISTER_SURFACE_NAME`]; tags the
/// registration with `RecipeKind::Chore` instead of `RecipeKind::Recipe`.
pub const REGISTER_SURFACE_CHORE_NAME: &str = "__register_surface_chore";

/// Which file descriptor a captured output line came from.
///
/// Carried alongside each line in [`crate`]-level work-result output buffers so
/// downstream observers (the engine event stream, the JSON writer's
/// `events.jsonl`, the per-node log store) can render the line with its true
/// origin instead of attributing every byte to stdout. CS-0035 made this
/// distinction load-bearing: prior to the fix, `WorkResult::output_lines` was
/// `Vec<String>` with no fd-of-origin and the wire-format's `stream` field
/// was hardcoded to `"stdout"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// Which Cookfile step kind a unit was captured from.
///
/// CS-0045: drives the per-item sandbox policy in the execute-phase
/// Lua VM. Cook/test/chore step Lua bodies all run with the
/// project-root sandbox — there is no unsandboxed step kind
/// (CS-0135 retired the `plate` step, which was the prior exception).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StepKind {
    /// `cook` step body — cacheable, hermetic, sandboxed.
    Cook,
    /// `test` step body — non-cacheable but hermetic-by-intent,
    /// sandboxed identically to `Cook`.
    Test,
    /// `chore` body — non-cacheable, hermetic-by-intent, sandboxed.
    Chore,
}

/// Declared inputs for a probe unit. Each category contributes to the
/// probe's fingerprint per §22.5.3.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeInputs {
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
}

/// A probe unit declared via `cook.probe(key, opts)` (§22.5.2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeUnit {
    pub key: String,
    pub produce_source: String,
    pub produce_line: usize,
    pub inputs: ProbeInputs,
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

/// Declarative description of post-execution input discovery for a unit.
///
/// When present on a [`CacheMeta`], the engine MUST:
///   - Read the file at [`Self::from`] (relative to the unit's working
///     directory) before composing the cache check's `current_inputs`,
///     parsing it under [`Self::format`].
///   - After successful execution, parse the file again and append its
///     contents to the recorded `StepEntry.inputs`.
///   - Treat the file as an implicit restorable output: uploaded under
///     its own artifact key, restored on a hit-with-drifted-outputs check.
///
/// The only currently supported `format` is `"make"`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredInputs {
    pub from: String,
    pub format: String,
}

/// A unit's sharing disposition (Cache-trust v3, §8.4.3). `local` and `pinned`
/// are spec-mutually-exclusive, so they are modelled as ONE enum rather than two
/// independent bools — `(local, pinned) = (true, true)` is unrepresentable.
///
/// `record` is a SEPARATE orthogonal bool (it waives byte-equivalence) and is
/// NOT folded in here.
///
/// Serialises across the codegen↔register Lua boundary as a string field
/// `sharing = "local"` / `"pinned"`, omitted entirely for `Shared` (so plain
/// steps stay byte-identical and the reserved-keyword `["local"]` bracket-quote
/// hack is gone).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sharing {
    /// Default: the unit participates in the shared content-addressed cache.
    #[default]
    Shared,
    /// `local` — opt out of sharing (local `StepEntry` index only).
    Local,
    /// `pinned` — designated-producer / fetch-only.
    Pinned,
}

impl Sharing {
    /// True for `Local`. Convenience for the many call sites that previously
    /// read a `local` bool.
    pub fn is_local(self) -> bool {
        matches!(self, Sharing::Local)
    }

    /// True for `Pinned`.
    pub fn is_pinned(self) -> bool {
        matches!(self, Sharing::Pinned)
    }

    /// The lowercase wire string used on the codegen↔register Lua boundary, or
    /// `None` for `Shared` (which is emitted as the absence of the field).
    pub fn as_wire_str(self) -> Option<&'static str> {
        match self {
            Sharing::Shared => None,
            Sharing::Local => Some("local"),
            Sharing::Pinned => Some("pinned"),
        }
    }

    /// Parse a wire string back into a `Sharing`. Unknown strings map to
    /// `Shared` (the register reader treats absence / unknown as default).
    pub fn from_wire_str(s: &str) -> Self {
        match s {
            "local" => Sharing::Local,
            "pinned" => Sharing::Pinned,
            _ => Sharing::Shared,
        }
    }
}

/// Metadata used by the caching subsystem to determine whether a unit can be
/// skipped.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheMeta {
    pub recipe_name: String,
    /// NEW: from .cook/cloud.toml `project = "..."` (Phase 3 wires real values).
    pub project_id: String,
    /// NEW: relative path of the source Cookfile from the project root, forward-slashed.
    pub cookfile_path: String,
    pub cache_key: String,
    pub input_paths: Vec<String>,
    pub output_paths: Vec<String>,
    pub command_hash: u64,
    /// NEW: post-denylist env contribution. Phase 5 wires real values; zero until then.
    pub env_contribution: u64,
    /// NEW: the (key, value) pairs the command consulted post-denylist.
    /// Phase 5 wires real values; empty BTreeMap until then.
    pub consulted_env: std::collections::BTreeMap<String, String>,
    pub discovered_inputs: Option<DiscoveredInputs>,
    /// COOK-161: the unit's *effective seal set* — bare probe keys whose
    /// canonical values fold into the cache key at execute-phase. Sorted /
    /// de-duplicated (`BTreeSet`). Empty for unsealed / `local` / `pinned` units.
    pub seal_keys: std::collections::BTreeSet<String>,
    /// COOK-162 §3/§17: sharing disposition. `Local` caches in the local
    /// `StepEntry` index only (never published to / fetched from the shared
    /// `CacheBackend`); `Pinned` is fetch-only / designated-producer (MUST be
    /// served from cache, a cold miss is a HARD ERROR). `Local` and `Pinned`
    /// are mutually exclusive by construction. Does not change the cache key.
    pub sharing: Sharing,
    /// COOK-163: `record` disposition — this unit produces an intrinsically
    /// NON-reproducible artifact (LLM / image gen). Keyed normally (the key is
    /// unchanged), but byte-equivalence is WAIVED: a present (or restorable)
    /// artifact is authoritative and MUST NOT be re-produced on output drift.
    /// This is also the seam COOK-167's verifier reads to skip byte-checking.
    pub record: bool,
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
    /// Part of a cook step group (can run parallel with siblings in the group).
    StepGroup(usize),
    /// Sequential barrier (depends on all prior units in recipe).
    Sequential,
    /// Part of a test step group — like StepGroup but failures don't cancel siblings.
    TestSibling(usize),
}

/// Result of registering a single recipe.
#[derive(Debug, Clone)]
pub struct RecipeUnits {
    pub recipe_name: String,
    pub deps: Vec<String>,
    pub units: Vec<CapturedUnit>,
    pub step_groups: Vec<Vec<usize>>,
    pub working_dir: PathBuf,
    pub env_vars: BTreeMap<String, String>,
    /// Terminal outputs: the output paths from the recipe's final cook step.
    pub terminal_outputs: Vec<String>,
    /// Fine-grained cross-recipe dependency edges.
    /// Each entry is (unit_index_in_this_recipe, dep_recipe_name).
    pub dep_edges: Vec<(usize, String)>,
    /// Probe units registered during this register pass (§22.5.2).
    pub probes: Vec<ProbeUnit>,
}

#[cfg(test)]
#[path = "tests/contracts_tests.rs"]
mod tests;
