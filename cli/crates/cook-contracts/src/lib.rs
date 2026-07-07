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
/// Lua VM. Cook/test/chore step Lua bodies run with the project-root
/// sandbox; plate step Lua bodies run unsandboxed because plates are
/// the explicit "ship outside the project" surface
/// (§{recipes.plate-step}).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StepKind {
    /// `cook` step body — cacheable, hermetic, sandboxed.
    Cook,
    /// `plate` step body — non-cacheable, non-hermetic by design,
    /// not sandboxed.
    Plate,
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
                if cmd.len() <= 60 {
                    cmd.clone()
                } else {
                    format!("{}...", &cmd[..57])
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
/// The only currently supported `format` is `"make"`. See the design at
/// `standard/specs/2026-05-04-discovered-inputs-design.md`.
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
    /// the per-member output map that `$<recipe[]>` joins on.
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
mod tests {
    use super::*;

    #[test]
    fn work_payload_shell_construction() {
        let p = WorkPayload::Shell { cmd: "gcc -c foo.c".into(), line: 1 };
        match &p {
            WorkPayload::Shell { cmd, line } => {
                assert_eq!(cmd, "gcc -c foo.c");
                assert_eq!(*line, 1);
            }
            _ => panic!("expected Shell variant"),
        }
    }

    #[test]
    fn work_payload_interactive_construction() {
        let p = WorkPayload::Interactive { cmd: "docker run -it ubuntu".into(), line: 5, is_chore: false };
        assert!(matches!(p, WorkPayload::Interactive { line: 5, .. }));
    }

    #[test]
    fn interactive_payload_carries_is_chore_flag() {
        let chore_unit = WorkPayload::Interactive {
            cmd: "fzf --prompt='> '".into(),
            line: 5,
            is_chore: true,
        };
        assert!(matches!(chore_unit, WorkPayload::Interactive { is_chore: true, .. }));

        let inline_interactive = WorkPayload::Interactive {
            cmd: "build/bin/lua -e 'print(1)'".into(),
            line: 12,
            is_chore: false,
        };
        assert!(matches!(inline_interactive, WorkPayload::Interactive { is_chore: false, .. }));
    }

    #[test]
    fn work_payload_lua_chunk_construction() {
        let p = WorkPayload::LuaChunk {
            code: "print('hi')".into(),
            inputs: vec!["in.txt".into()],
            outputs: vec!["out.txt".into()],
            ingredient_groups: vec![vec!["a".into(), "b".into()]],
            step_kind: StepKind::Cook,
            is_chore: false,
            line: 1,
        };
        match &p {
            WorkPayload::LuaChunk { code, inputs, outputs, ingredient_groups, step_kind, is_chore, line: _ } => {
                assert_eq!(*step_kind, StepKind::Cook);
                assert_eq!(code, "print('hi')");
                assert_eq!(inputs, &vec!["in.txt".to_string()]);
                assert_eq!(outputs, &vec!["out.txt".to_string()]);
                assert_eq!(ingredient_groups.len(), 1);
                assert_eq!(ingredient_groups[0].len(), 2);
                assert!(!*is_chore);
            }
            _ => panic!("expected LuaChunk variant"),
        }
    }

    #[test]
    fn work_payload_lua_chunk_carries_is_chore_flag() {
        let chore_unit = WorkPayload::LuaChunk {
            code: "print('chore')".into(),
            inputs: vec![],
            outputs: vec![],
            ingredient_groups: vec![],
            step_kind: StepKind::Chore,
            is_chore: true,
            line: 1,
        };
        assert!(matches!(chore_unit, WorkPayload::LuaChunk { is_chore: true, .. }));
    }

    #[test]
    fn work_payload_test_construction() {
        let p = WorkPayload::Test {
            cmd: "./run_tests".into(),
            line: 10,
            timeout: 30,
            should_fail: false,
            suite_name: "unit".into(),
            test_name: "test_foo".into(),
            iteration_item: None,
            lua_code: None,
            input_paths: vec![],
        };
        assert!(matches!(p, WorkPayload::Test { timeout: 30, should_fail: false, .. }));
    }

    #[test]
    fn cache_meta_construction() {
        let m = CacheMeta {
            recipe_name: "build".into(),
            project_id: String::new(),
            cookfile_path: String::new(),
            cache_key: "abc123".into(),
            input_paths: vec!["src/main.rs".into()],
            output_paths: vec!["target/debug/app".into()],
            command_hash: 42,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
            discovered_inputs: None,
            seal_keys: Default::default(),
            sharing: Default::default(),
            record: false,
        };
        assert_eq!(m.recipe_name, "build");
        assert_eq!(m.command_hash, 42);
        assert_eq!(m.input_paths.len(), 1);
        assert_eq!(m.output_paths.len(), 1);
    }

    #[test]
    fn cache_meta_no_output() {
        let m = CacheMeta {
            recipe_name: "lint".into(),
            project_id: String::new(),
            cookfile_path: String::new(),
            cache_key: "def456".into(),
            input_paths: vec![],
            output_paths: vec![],
            command_hash: 0,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
            discovered_inputs: None,
            seal_keys: Default::default(),
            sharing: Default::default(),
            record: false,
        };
        assert!(m.output_paths.is_empty());
    }

    #[test]
    fn cache_meta_construction_with_discovered_inputs() {
        let m = CacheMeta {
            recipe_name: "compile".into(),
            project_id: "p".into(),
            cookfile_path: "Cookfile".into(),
            cache_key: "k".into(),
            input_paths: vec!["src/a.c".into()],
            output_paths: vec!["build/a.o".into()],
            command_hash: 0xdead,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
            discovered_inputs: Some(DiscoveredInputs {
                from: ".cook/deps/a.d".into(),
                format: "make".into(),
            }),
            seal_keys: Default::default(),
            sharing: Default::default(),
            record: false,
        };
        let di = m.discovered_inputs.as_ref().expect("present");
        assert_eq!(di.from, ".cook/deps/a.d");
        assert_eq!(di.format, "make");
    }

    #[test]
    fn cache_meta_default_discovered_inputs_is_none() {
        let m = CacheMeta {
            recipe_name: "r".into(),
            project_id: "p".into(),
            cookfile_path: "Cookfile".into(),
            cache_key: "k".into(),
            input_paths: vec![],
            output_paths: vec![],
            command_hash: 0,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
            discovered_inputs: None,
            seal_keys: Default::default(),
            sharing: Default::default(),
            record: false,
        };
        assert!(m.discovered_inputs.is_none());
    }

    #[test]
    fn cache_meta_carries_seal_keys() {
        let mut seal = std::collections::BTreeSet::new();
        seal.insert("host".to_string());
        seal.insert("cc:toolchain".to_string());
        let meta = CacheMeta {
            recipe_name: "build".into(),
            project_id: String::new(),
            cookfile_path: "Cookfile".into(),
            cache_key: "k".into(),
            input_paths: vec![],
            output_paths: vec!["x.o".into()],
            command_hash: 1,
            env_contribution: 0,
            consulted_env: Default::default(),
            discovered_inputs: None,
            seal_keys: seal.clone(),
            sharing: Default::default(),
            record: false,
        };
        assert_eq!(meta.seal_keys, seal);
    }

    #[test]
    fn cache_meta_carries_record_flag() {
        let mut meta = CacheMeta {
            recipe_name: "r".into(),
            project_id: String::new(),
            cookfile_path: String::new(),
            cache_key: "k".into(),
            input_paths: vec![],
            output_paths: vec!["out".into()],
            command_hash: 0,
            env_contribution: 0,
            consulted_env: Default::default(),
            discovered_inputs: None,
            seal_keys: Default::default(),
            sharing: Default::default(),
            record: false,
        };
        assert!(!meta.record, "record defaults to false");
        meta.record = true;
        assert!(meta.record, "record flag is settable and read back");
    }

    #[test]
    fn captured_unit_construction() {
        let unit = CapturedUnit {
            payload: WorkPayload::Shell { cmd: "echo hi".into(), line: 1 },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        };
        assert!(unit.cache_meta.is_none());
        assert!(matches!(unit.dep_kind, DepKind::Sequential));
    }

    #[test]
    fn dep_kind_variants() {
        let sg = DepKind::StepGroup(3);
        assert!(matches!(sg, DepKind::StepGroup(3)));

        let seq = DepKind::Sequential;
        assert!(matches!(seq, DepKind::Sequential));

        let ts = DepKind::TestSibling(1);
        assert!(matches!(ts, DepKind::TestSibling(1)));
    }

    #[test]
    fn recipe_units_construction() {
        let mut env = BTreeMap::new();
        env.insert("CC".into(), "gcc".into());
        env.insert("AR".into(), "ar".into());

        let recipe = RecipeUnits {
            recipe_name: "build".into(),
            deps: vec!["fetch".into(), "generate".into()],
            units: vec![
                CapturedUnit {
                    payload: WorkPayload::Shell { cmd: "gcc -c a.c".into(), line: 1 },
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                    probes: vec![],
                    unit_env_vars: Default::default(),
                    member: None,
                    output_paths: Vec::new(),
                },
                CapturedUnit {
                    payload: WorkPayload::Shell { cmd: "gcc -c b.c".into(), line: 2 },
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                    probes: vec![],
                    unit_env_vars: Default::default(),
                    member: None,
                    output_paths: Vec::new(),
                },
            ],
            step_groups: vec![vec![0, 1]],
            working_dir: PathBuf::from("/home/user/project"),
            env_vars: env,
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };

        assert_eq!(recipe.recipe_name, "build");
        assert_eq!(recipe.deps.len(), 2);
        assert_eq!(recipe.units.len(), 2);
        assert_eq!(recipe.step_groups.len(), 1);
        assert_eq!(recipe.step_groups[0], vec![0, 1]);
        // BTreeMap iteration is deterministic / sorted
        let keys: Vec<&String> = recipe.env_vars.keys().collect();
        assert_eq!(keys, vec!["AR", "CC"]);
    }

    #[test]
    fn recipe_units_with_terminal_outputs() {
        let recipe = RecipeUnits {
            recipe_name: "libmath".into(),
            deps: vec![],
            units: vec![],
            step_groups: vec![],
            working_dir: PathBuf::from("."),
            env_vars: BTreeMap::new(),
            terminal_outputs: vec!["build/lib/libmath.a".into()],
            dep_edges: vec![],
            probes: vec![],
        };
        assert_eq!(recipe.terminal_outputs, vec!["build/lib/libmath.a"]);
        assert!(recipe.dep_edges.is_empty());
    }

    #[test]
    fn work_payload_clone() {
        let original = WorkPayload::Shell { cmd: "make".into(), line: 1 };
        let cloned = original.clone();
        assert!(matches!(cloned, WorkPayload::Shell { line: 1, .. }));
    }

    #[test]
    fn probe_inputs_default_is_empty() {
        let i = ProbeInputs::default();
        assert!(i.env.is_empty());
        assert!(i.tools.is_empty());
        assert!(i.files.is_empty());
        assert!(i.requires.is_empty());
    }

    #[test]
    fn probe_unit_round_trips_through_serde() {
        let p = ProbeUnit {
            key: "cc:zlib".into(),
            produce_source: "return run_pkg_config(\"zlib\")".into(),
            produce_line: 42,
            inputs: ProbeInputs {
                env: vec!["PKG_CONFIG_PATH".into()],
                tools: vec!["pkg-config".into()],
                files: vec![],
                requires: vec!["cc:compiler".into()],
            },
        };
        let s = serde_json::to_string(&p).unwrap();
        let r: ProbeUnit = serde_json::from_str(&s).unwrap();
        assert_eq!(r.key, "cc:zlib");
        assert_eq!(r.inputs.requires, vec!["cc:compiler"]);
    }

    #[test]
    fn work_payload_probe_variant_constructs() {
        let p = WorkPayload::Probe {
            key: "cc:zlib".into(),
            produce: "return 42".into(),
            line: 1,
        };
        match &p {
            WorkPayload::Probe { key, produce, line } => {
                assert_eq!(key, "cc:zlib");
                assert_eq!(produce, "return 42");
                assert_eq!(*line, 1);
            }
            _ => panic!("expected Probe variant"),
        }
    }

    #[test]
    fn captured_unit_probes_defaults_to_empty() {
        let p = WorkPayload::Shell { cmd: "echo hi".into(), line: 1 };
        let cu = CapturedUnit {
            payload: p,
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        };
        assert!(cu.probes.is_empty());
    }

    #[test]
    fn recipe_units_probes_defaults_to_empty() {
        // If a literal RecipeUnits constructor in the existing tests has been
        // updated, this test just confirms the field is accessible. Construct
        // minimally using whatever helper exists, or by literal — match existing
        // style.
        use std::collections::BTreeMap;
        use std::path::PathBuf;
        let r = RecipeUnits {
            recipe_name: "x".into(),
            deps: vec![],
            units: vec![],
            step_groups: vec![],
            working_dir: PathBuf::new(),
            env_vars: BTreeMap::new(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        assert!(r.probes.is_empty());
    }

    #[test]
    fn captured_unit_with_cache() {
        let unit = CapturedUnit {
            payload: WorkPayload::Shell { cmd: "gcc -o app main.c".into(), line: 5 },
            cache_meta: Some(CacheMeta {
                recipe_name: "compile".into(),
                project_id: String::new(),
                cookfile_path: String::new(),
                cache_key: "key123".into(),
                input_paths: vec!["main.c".into()],
                output_paths: vec!["app".into()],
                command_hash: 9999,
                env_contribution: 0,
                consulted_env: std::collections::BTreeMap::new(),
                discovered_inputs: None,
                seal_keys: Default::default(),
            sharing: Default::default(),
                record: false,
            }),
            dep_kind: DepKind::StepGroup(0),
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        };
        assert!(unit.cache_meta.is_some());
        assert_eq!(unit.cache_meta.unwrap().command_hash, 9999);
    }
}
