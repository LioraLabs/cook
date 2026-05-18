//! Shared kernel types for the Cook build system.
//!
//! This crate contains behavior-free structs and enums used across multiple
//! Cook crates. It has zero dependencies on other Cook crates.

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
    },
    Test {
        cmd: String,
        line: usize,
        timeout: u64,
        should_fail: bool,
        suite_name: String,
        test_name: String,
        iteration_item: Option<String>,
    },
    /// A probe unit (§22.5.2): runs `produce` (Lua source string) on a worker
    /// VM and stashes the msgpack-serialised return value under `key`.
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
    /// NEW: machine + tool identity. Phase 6 wires real values; zero until then.
    pub context_hash: u64,
    /// NEW: post-denylist env contribution. Phase 5 wires real values; zero until then.
    pub env_contribution: u64,
    /// NEW: the (key, value) pairs the command consulted post-denylist.
    /// Phase 5 wires real values; empty BTreeMap until then.
    pub consulted_env: std::collections::BTreeMap<String, String>,
    pub discovered_inputs: Option<DiscoveredInputs>,
}

/// A single captured unit of work within a recipe.
#[derive(Debug, Clone)]
pub struct CapturedUnit {
    pub payload: WorkPayload,
    pub cache_meta: Option<CacheMeta>,
    pub dep_kind: DepKind,
    /// Probe keys this unit consumes (§22.5.5). Empty for non-consumer units.
    pub probes: Vec<String>,
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
        };
        match &p {
            WorkPayload::LuaChunk { code, inputs, outputs, ingredient_groups, step_kind, is_chore } => {
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
            context_hash: 0,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
            discovered_inputs: None,
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
            context_hash: 0,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
            discovered_inputs: None,
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
            context_hash: 0,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
            discovered_inputs: Some(DiscoveredInputs {
                from: ".cook/deps/a.d".into(),
                format: "make".into(),
            }),
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
            context_hash: 0,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
            discovered_inputs: None,
        };
        assert!(m.discovered_inputs.is_none());
    }

    #[test]
    fn captured_unit_construction() {
        let unit = CapturedUnit {
            payload: WorkPayload::Shell { cmd: "echo hi".into(), line: 1 },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
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
                },
                CapturedUnit {
                    payload: WorkPayload::Shell { cmd: "gcc -c b.c".into(), line: 2 },
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                    probes: vec![],
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
                context_hash: 0,
                env_contribution: 0,
                consulted_env: std::collections::BTreeMap::new(),
                discovered_inputs: None,
            }),
            dep_kind: DepKind::StepGroup(0),
            probes: vec![],
        };
        assert!(unit.cache_meta.is_some());
        assert_eq!(unit.cache_meta.unwrap().command_hash, 9999);
    }
}
