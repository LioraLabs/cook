//! Shared kernel types for the Cook build system.
//!
//! This crate contains behavior-free structs and enums used across multiple
//! Cook crates. It has zero dependencies on other Cook crates.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// What kind of work a captured unit represents.
#[derive(Debug, Clone)]
pub enum WorkPayload {
    Shell {
        cmd: String,
        line: usize,
    },
    Interactive {
        cmd: String,
        line: usize,
    },
    LuaChunk {
        code: String,
        input: String,
        output: String,
        ingredient_groups: Vec<Vec<String>>,
    },
    Test {
        cmd: String,
        line: usize,
        timeout: u64,
        should_fail: bool,
        suite_name: String,
        test_name: String,
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
            Self::Interactive { cmd, .. } => format!("interactive: {cmd}"),
            Self::Test { test_name, .. } => test_name.clone(),
        }
    }
}

/// Metadata used by the caching subsystem to determine whether a unit can be
/// skipped.
#[derive(Debug, Clone)]
pub struct CacheMeta {
    pub recipe_name: String,
    pub cache_key: String,
    pub input_paths: Vec<String>,
    pub output_path: Option<String>,
    pub command_hash: u64,
}

/// A single captured unit of work within a recipe.
#[derive(Debug, Clone)]
pub struct CapturedUnit {
    pub payload: WorkPayload,
    pub cache_meta: Option<CacheMeta>,
    pub dep_kind: DepKind,
}

/// How a captured unit relates to others in the recipe.
#[derive(Debug, Clone)]
pub enum DepKind {
    /// Part of a cook step group (can run parallel with siblings in the group).
    StepGroup(usize),
    /// Sequential barrier (depends on all prior units in recipe).
    Sequential,
    /// Part of a test step group — like StepGroup but failures don't cancel siblings.
    TestSibling(usize),
}

/// Result of registering a single recipe.
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
        let p = WorkPayload::Interactive { cmd: "docker run -it ubuntu".into(), line: 5 };
        assert!(matches!(p, WorkPayload::Interactive { line: 5, .. }));
    }

    #[test]
    fn work_payload_lua_chunk_construction() {
        let p = WorkPayload::LuaChunk {
            code: "print('hi')".into(),
            input: "in.txt".into(),
            output: "out.txt".into(),
            ingredient_groups: vec![vec!["a".into(), "b".into()]],
        };
        match &p {
            WorkPayload::LuaChunk { code, ingredient_groups, .. } => {
                assert_eq!(code, "print('hi')");
                assert_eq!(ingredient_groups.len(), 1);
                assert_eq!(ingredient_groups[0].len(), 2);
            }
            _ => panic!("expected LuaChunk variant"),
        }
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
        };
        assert!(matches!(p, WorkPayload::Test { timeout: 30, should_fail: false, .. }));
    }

    #[test]
    fn cache_meta_construction() {
        let m = CacheMeta {
            recipe_name: "build".into(),
            cache_key: "abc123".into(),
            input_paths: vec!["src/main.rs".into()],
            output_path: Some("target/debug/app".into()),
            command_hash: 42,
        };
        assert_eq!(m.recipe_name, "build");
        assert_eq!(m.command_hash, 42);
        assert_eq!(m.input_paths.len(), 1);
        assert!(m.output_path.is_some());
    }

    #[test]
    fn cache_meta_no_output() {
        let m = CacheMeta {
            recipe_name: "lint".into(),
            cache_key: "def456".into(),
            input_paths: vec![],
            output_path: None,
            command_hash: 0,
        };
        assert!(m.output_path.is_none());
    }

    #[test]
    fn captured_unit_construction() {
        let unit = CapturedUnit {
            payload: WorkPayload::Shell { cmd: "echo hi".into(), line: 1 },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
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
                },
                CapturedUnit {
                    payload: WorkPayload::Shell { cmd: "gcc -c b.c".into(), line: 2 },
                    cache_meta: None,
                    dep_kind: DepKind::StepGroup(0),
                },
            ],
            step_groups: vec![vec![0, 1]],
            working_dir: PathBuf::from("/home/user/project"),
            env_vars: env,
            terminal_outputs: vec![],
            dep_edges: vec![],
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
    fn captured_unit_with_cache() {
        let unit = CapturedUnit {
            payload: WorkPayload::Shell { cmd: "gcc -o app main.c".into(), line: 5 },
            cache_meta: Some(CacheMeta {
                recipe_name: "compile".into(),
                cache_key: "key123".into(),
                input_paths: vec!["main.c".into()],
                output_path: Some("app".into()),
                command_hash: 9999,
            }),
            dep_kind: DepKind::StepGroup(0),
        };
        assert!(unit.cache_meta.is_some());
        assert_eq!(unit.cache_meta.unwrap().command_hash, 9999);
    }
}
