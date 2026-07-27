use cook_contracts::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[test]
fn work_payload_shell_construction() {
    let p = WorkPayload::Shell {
        cmd: "gcc -c foo.c".into(),
        line: 1,
    };
    match &p {
        WorkPayload::Shell { cmd, line } => {
            assert_eq!(cmd, "gcc -c foo.c");
            assert_eq!(*line, 1);
        }
        _ => panic!("expected Shell variant"),
    }
}

#[test]
fn shell_display_name_strips_set_e_and_is_single_line() {
    let p = WorkPayload::Shell {
        cmd: "set -e\nwc -w < a.txt > b.count".into(),
        line: 1,
    };
    let d = p.display_name();
    assert!(!d.contains("set -e"), "got: {d}");
    assert!(!d.contains('\n'), "got: {d}");
    assert!(d.starts_with("wc"), "got: {d}");
}

#[test]
fn shell_display_name_degenerate_body_is_never_blank() {
    // A body that is nothing but the `set -e` preamble must still yield a
    // non-blank label (inline renderer surfaces this string directly).
    let p = WorkPayload::Shell {
        cmd: "set -e".into(),
        line: 1,
    };
    assert!(
        !p.display_name().is_empty(),
        "blank label for set -e-only body"
    );
    let empty = WorkPayload::Shell {
        cmd: String::new(),
        line: 1,
    };
    assert!(
        !empty.display_name().is_empty(),
        "blank label for empty body"
    );
}

#[test]
fn work_payload_interactive_construction() {
    let p = WorkPayload::Interactive {
        cmd: "docker run -it ubuntu".into(),
        line: 5,
        is_chore: false,
    };
    assert!(matches!(p, WorkPayload::Interactive { line: 5, .. }));
}

#[test]
fn interactive_payload_carries_is_chore_flag() {
    let chore_unit = WorkPayload::Interactive {
        cmd: "fzf --prompt='> '".into(),
        line: 5,
        is_chore: true,
    };
    assert!(matches!(
        chore_unit,
        WorkPayload::Interactive { is_chore: true, .. }
    ));

    let inline_interactive = WorkPayload::Interactive {
        cmd: "build/bin/lua -e 'print(1)'".into(),
        line: 12,
        is_chore: false,
    };
    assert!(matches!(
        inline_interactive,
        WorkPayload::Interactive {
            is_chore: false,
            ..
        }
    ));
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
        WorkPayload::LuaChunk {
            code,
            inputs,
            outputs,
            ingredient_groups,
            step_kind,
            is_chore,
            line: _,
        } => {
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
    assert!(matches!(
        chore_unit,
        WorkPayload::LuaChunk { is_chore: true, .. }
    ));
}

#[test]
fn work_payload_test_construction() {
    let p = WorkPayload::Test {
        seal_keys: Default::default(),
        consumes: Vec::new(),
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
    assert!(matches!(
        p,
        WorkPayload::Test {
            timeout: 30,
            should_fail: false,
            ..
        }
    ));
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
        payload: WorkPayload::Shell {
            cmd: "echo hi".into(),
            line: 1,
        },
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
                payload: WorkPayload::Shell {
                    cmd: "gcc -c a.c".into(),
                    line: 1,
                },
                cache_meta: None,
                dep_kind: DepKind::StepGroup(0),
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
            CapturedUnit {
                payload: WorkPayload::Shell {
                    cmd: "gcc -c b.c".into(),
                    line: 2,
                },
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
    let original = WorkPayload::Shell {
        cmd: "make".into(),
        line: 1,
    };
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
    let p = WorkPayload::Shell {
        cmd: "echo hi".into(),
        line: 1,
    };
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
        payload: WorkPayload::Shell {
            cmd: "gcc -o app main.c".into(),
            line: 5,
        },
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
