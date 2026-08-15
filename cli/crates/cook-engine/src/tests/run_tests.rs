use super::*;

#[test]
fn test_split_recipe_name_with_prefix() {
    let (prefix, local) = split_recipe_name("backend.proto.generate");
    assert_eq!(prefix, "backend.proto");
    assert_eq!(local, "generate");
}

#[test]
fn test_split_recipe_name_no_prefix() {
    let (prefix, local) = split_recipe_name("build");
    assert_eq!(prefix, "");
    assert_eq!(local, "build");
}

#[test]
fn test_split_recipe_name_single_dot() {
    let (prefix, local) = split_recipe_name("backend.build");
    assert_eq!(prefix, "backend");
    assert_eq!(local, "build");
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
            working_dir_by_prefix: BTreeMap::new(),
            alias_dirs_by_prefix: BTreeMap::new(),
            terminal_outputs: BTreeMap::new(),
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
