use super::*;
use cook_contracts::probe_key::{qualified_key, LocalProbeKey};

/// COOK-526 folded this crate's `split_recipe_name` into the shared law
/// (`cook_contracts::naming::import_prefix`); both of its call sites wanted
/// the prefix half only. The cases it pinned are kept here as a regression
/// guard against re-forking a local copy.
#[test]
fn recipe_prefix_comes_from_the_shared_law() {
    use cook_contracts::naming::import_prefix;
    assert_eq!(import_prefix("backend.proto.generate"), "backend.proto");
    assert_eq!(import_prefix("build"), "");
    assert_eq!(import_prefix("backend.build"), "backend");
}

fn dummy_project_root() -> std::path::PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        std::mem::forget(dir);
        path
    }

    /// Build an empty `RegisteredWorkspace` for tests that exercise the
    /// pre-DAG-build entry paths (empty targets, finished-event emission).
    fn empty_registered_workspace() -> RegisteredWorkspace {
        RegisteredWorkspace {
            warnings: Vec::new(),
            names: Vec::new(),
            units_by_recipe: BTreeMap::new(),
            probes: BTreeMap::new(),
            resolved_probe_keys: Default::default(),
            working_dir_by_prefix: BTreeMap::new(),
            alias_dirs_by_prefix: BTreeMap::new(),
            terminal_outputs: BTreeMap::new(),
            materializations: Vec::new(),
        }
    }

    #[test]
    fn test_run_empty_reachable_returns_ok_with_no_results() {
        // Empty reachable set: no DAG to walk, no synthetic lifecycle events.
        // run() should short-circuit cleanly and emit Finished{success:true}.
        let ws = empty_registered_workspace();
        let edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let reachable: BTreeSet<String> = BTreeSet::new();
        let result = run(
            &dummy_project_root(),
            &ws,
            &edges,
            &reachable,
            1,
            &[],
            false,
            false,
        false,
            |_| {},
        );
        assert!(result.is_ok());
        assert!(result.unwrap().test_results.is_empty());
    }

    #[test]
    fn test_run_unknown_recipe_in_reachable() {
        // A name present in `reachable` but absent from
        // `registered_workspace.units_by_recipe` must surface as
        // `UnknownRecipe(name)`.
        let ws = empty_registered_workspace();
        let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edges.insert("missing".into(), vec![]);
    let reachable: BTreeSet<String> = ["missing"].iter().map(|s| s.to_string()).collect();
    let result = run(
        &dummy_project_root(),
        &ws,
        &edges,
        &reachable,
        1,
        &[],
        false,
        false,
        false,
        |_| {},
    );
    assert!(result.is_err());
    match result.unwrap_err() {
        EngineError::UnknownRecipe(name) => assert_eq!(name, "missing"),
        other => panic!("expected UnknownRecipe, got: {other:?}"),
    }
}

#[test]
fn test_run_emits_finished_success_on_empty_reachable() {
    let ws = empty_registered_workspace();
    let edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let reachable: BTreeSet<String> = BTreeSet::new();

    let events = std::sync::Mutex::new(Vec::new());
    let result = run(
        &dummy_project_root(),
        &ws,
        &edges,
        &reachable,
        1,
        &[],
        false,
        false,
        false,
        |event| events.lock().unwrap().push(event),
    );
    assert!(result.is_ok());

    let events = events.lock().unwrap();
    let finished = events.iter().find_map(|e| match e {
        EngineEvent::Finished { success, .. } => Some(*success),
        _ => None,
    });
    assert_eq!(
        finished,
        Some(true),
        "expected Finished{{success:true}} event"
    );
}

#[test]
fn test_run_emits_finished_failure_on_unknown_recipe() {
    let ws = empty_registered_workspace();
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    edges.insert("missing".into(), vec![]);
    let reachable: BTreeSet<String> = ["missing"].iter().map(|s| s.to_string()).collect();

    let events = std::sync::Mutex::new(Vec::new());
    let result = run(
        &dummy_project_root(),
        &ws,
        &edges,
        &reachable,
        1,
        &[],
        false,
        false,
        false,
        |event| events.lock().unwrap().push(event),
    );
    assert!(result.is_err());

    let events = events.lock().unwrap();
    let finished = events.iter().find_map(|e| match e {
        EngineEvent::Finished { success, .. } => Some(*success),
        _ => None,
    });
    assert_eq!(
        finished,
        Some(false),
        "expected Finished{{success:false}} event"
    );
}

#[test]
fn test_toposort_reachable_diamond() {
    // a -> b, a -> c, b -> d, c -> d
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    edges.insert("a".into(), vec![]);
    edges.insert("b".into(), vec!["a".into()]);
    edges.insert("c".into(), vec!["a".into()]);
    edges.insert("d".into(), vec!["b".into(), "c".into()]);
    let reachable: BTreeSet<String> =
        ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
    let order = cook_contracts::unit_graph::toposort_recipes(&edges, &reachable)
        .expect("toposort");
    let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
    assert!(pos("a") < pos("b"));
    assert!(pos("a") < pos("c"));
    assert!(pos("b") < pos("d"));
    assert!(pos("c") < pos("d"));
}

#[test]
fn test_toposort_reachable_detects_cycle() {
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    edges.insert("a".into(), vec!["b".into()]);
    edges.insert("b".into(), vec!["a".into()]);
    let reachable: BTreeSet<String> =
        ["a", "b"].iter().map(|s| s.to_string()).collect();
    let result = cook_contracts::unit_graph::toposort_recipes(&edges, &reachable)
        .map_err(EngineError::from);
    assert!(result.is_err());
    match result.unwrap_err() {
        EngineError::CycleDetected(msg) => {
            assert!(
                msg.contains("\"a\""),
                "error should name cycle node 'a', got: {msg}"
            );
            assert!(
                msg.contains("\"b\""),
                "error should name cycle node 'b', got: {msg}"
            );
        }
        other => panic!("expected CycleDetected, got {other:?}"),
    }
}

#[test]
fn test_toposort_reachable_cycle_names_only_cycle_nodes() {
    // Build a graph with a long unrelated chain (x -> y -> z, all
    // resolvable) plus a 2-node cycle (a <-> b). The error should
    // name only the cycle nodes, not the resolvable ones.
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    edges.insert("x".into(), vec![]);
    edges.insert("y".into(), vec!["x".into()]);
    edges.insert("z".into(), vec!["y".into()]);
    edges.insert("a".into(), vec!["b".into()]);
    edges.insert("b".into(), vec!["a".into()]);
    let reachable: BTreeSet<String> = ["x", "y", "z", "a", "b"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let result = cook_contracts::unit_graph::toposort_recipes(&edges, &reachable)
        .map_err(EngineError::from);
    match result.unwrap_err() {
        EngineError::CycleDetected(msg) => {
            assert!(msg.contains("\"a\""), "missing cycle node 'a': {msg}");
            assert!(msg.contains("\"b\""), "missing cycle node 'b': {msg}");
            // The resolvable nodes must NOT appear in the cycle list.
            assert!(
                !msg.contains("\"x\""),
                "resolvable node 'x' should not be in cycle error: {msg}"
            );
            assert!(
                !msg.contains("\"y\""),
                "resolvable node 'y' should not be in cycle error: {msg}"
            );
            assert!(
                !msg.contains("\"z\""),
                "resolvable node 'z' should not be in cycle error: {msg}"
            );
        }
        other => panic!("expected CycleDetected, got {other:?}"),
    }
}

// The ready-time input-resolution tests that stood here moved with the
// behaviour they cover (CS-0186). WHICH paths a unit declares is now decided by
// the lowering, and is pinned in `cook_luagen`'s codegen tests; HOW a declared
// entry resolves against the tree is `cook_cache::resolve_declared_inputs`,
// and is pinned beside it. Neither is an engine concern any more: the engine's
// cache path no longer takes the DAG as an argument at all.

/// COOK-510: `member.consumer`'s local `srcs` declaration resolves
/// `src/a.txt` from `member_dir` and publishes that file's SHA-256 value.
#[test]
fn cook510_member_consumer_local_srcs_hashes_member_src_a_txt() {
    use cook_contracts::{CapturedUnit, DepKind, ProbeInputs, ProbeUnit};

    let project_root = tempfile::tempdir().expect("project_root tempdir");
    // Isolate the cache: never touch the user's real ~/.cache/cook/cloud.
    std::fs::create_dir_all(project_root.path().join(".cook")).unwrap();
    std::fs::write(
        project_root.path().join(".cook/cloud.toml"),
        format!(
            "[cache]\ncache_dir = {:?}\n",
            project_root.path().join("cache").to_string_lossy()
        ),
    )
    .unwrap();

    let root_dir = project_root.path().join("root");
    let member_dir = project_root.path().join("member");
    std::fs::create_dir_all(root_dir.join("src")).unwrap(); // deliberately no a.txt here
    std::fs::create_dir_all(member_dir.join("src")).unwrap();
    std::fs::write(member_dir.join("src/a.txt"), b"declaring-member-content").unwrap();

    let probe_meta = ProbeUnit {
        key: LocalProbeKey::new("srcs"),
        produce_source: cook_contracts::probe_value::FILES_MANIFEST_PRODUCE.to_string(),
        produce_line: 1,
        inputs: ProbeInputs { files: vec!["src/a.txt".to_string()], ..Default::default() },
    };

    let mut probes = BTreeMap::new();
    probes.insert(qualified_key("member", &probe_meta.key), probe_meta.clone());

    let mut working_dir_by_prefix = BTreeMap::new();
    working_dir_by_prefix.insert(String::new(), root_dir.clone());
    working_dir_by_prefix.insert("member".to_string(), member_dir.clone());

    let consumer = RecipeUnits {
        recipe_name: "member.consumer".to_string(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: WorkPayload::Shell { cmd: "true".to_string(), line: 1 },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec!["srcs".to_string()],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
            after: Vec::new(),
            test_name: None,
        }],
        step_groups: vec![],
        working_dir: root_dir.clone(),
        env_vars: BTreeMap::new(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        // dag_builder's `NodeOrigin::SynthProbe` synthesises the probe node's
        // metadata from the CONSUMING recipe's own `probes` list — this is
        // the part no real Cookfile can populate cross-member today.
        probes: vec![probe_meta],
    };

    let mut units_by_recipe = BTreeMap::new();
    units_by_recipe.insert("member.consumer".to_string(), consumer);

    let ws = RegisteredWorkspace {
        warnings: Vec::new(),
        names: Vec::new(),
        units_by_recipe,
        probes,
        resolved_probe_keys: Default::default(),
        working_dir_by_prefix,
        alias_dirs_by_prefix: BTreeMap::new(),
        terminal_outputs: BTreeMap::new(),
        materializations: Vec::new(),
    };

    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    edges.insert("member.consumer".to_string(), vec![]);
    let reachable: BTreeSet<String> = ["member.consumer"].iter().map(|s| s.to_string()).collect();

    let result = run(
        project_root.path(),
        &ws,
        &edges,
        &reachable,
        1,
        &[],
        false,
        false,
        false,
        |_| {},
    );
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let record_key = cook_contracts::probe_key::qualified_key("member", &LocalProbeKey::new("srcs"));
    let manifest_path = cook_contracts::layout::probes_dir(project_root.path())
        .join(cook_contracts::probe_value::probe_file_name(record_key.as_ref()));
    let bytes = std::fs::read(&manifest_path)
        .unwrap_or_else(|e| panic!("expected {}: {e}", manifest_path.display()));
    let value = cook_contracts::probe_value::decode_json(&bytes).unwrap();
    let hash = value.get("src/a.txt").and_then(|v| v.as_str()).unwrap_or_else(|| {
        panic!("expected src/a.txt in the manifest, got {value}")
    });
    assert_ne!(
        hash, "<missing>",
        "the probe hashed against the consumer's directory instead of its \
         own declaring member's — this is the false-hit COOK-510 closes",
    );
    let expected = cook_contracts::render::lower_hex(&cook_cache::probe::hash_file_sha256(
        &member_dir.join("src/a.txt"),
    ));
    assert_eq!(hash, expected, "must be the declaring member's real content hash");
}
