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
    let order = toposort_reachable(&edges, &reachable).expect("toposort");
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
    let result = toposort_reachable(&edges, &reachable);
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
    let result = toposort_reachable(&edges, &reachable);
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

// -----------------------------------------------------------------------
// compute_ready_test_fingerprint — COOK-211 ready-time content fold (§17.4)
// -----------------------------------------------------------------------

fn make_cache_ctx(wd: &std::path::Path) -> CacheContext {
    use cook_cache::{backend::LocalBackend, cloud_config::CloudConfig};
    use cook_fingerprint::EnvDenylist;
    CacheContext {
        denylist: std::sync::Arc::new(EnvDenylist::baseline()),
        backend: std::sync::Arc::new(LocalBackend::new(wd.join("cloud"))),
        cloud_config: std::sync::Arc::new(CloudConfig::default()),
        project_root: wd.to_path_buf(),
        project_id: "test".to_string(),
        publish_enabled: true,
    }
}

fn cook_work_node(
    wd: &std::path::Path,
    recipe: &str,
    inputs: &[&str],
    outputs: &[&str],
    command_hash: u64,
) -> crate::WorkNode {
    crate::WorkNode {
        process_env_vars: std::collections::BTreeMap::new(),
        payload: Some(WorkPayload::Shell {
            cmd: "build".into(),
            line: 1,
        }),
        recipe_name: recipe.into(),
        cache_meta: Some(cook_contracts::CacheMeta {
            recipe_name: recipe.into(),
            project_id: String::new(),
            cookfile_path: "Cookfile".into(),
            cache_key: outputs.first().copied().unwrap_or("k").into(),
            input_paths: inputs.iter().map(|s| s.to_string()).collect(),
            output_paths: outputs.iter().map(|s| s.to_string()).collect(),
            command_hash,
            env_contribution: 0,
            consulted_env: Default::default(),
            discovered_inputs: None,
            seal_keys: Default::default(),
            sharing: Default::default(),
            record: false,
        }),
        working_dir: wd.to_path_buf(),
        env_vars: Default::default(),
    }
}

fn test_work_node(wd: &std::path::Path, input_paths: &[&str]) -> crate::WorkNode {
    crate::WorkNode {
        process_env_vars: std::collections::BTreeMap::new(),
        payload: Some(WorkPayload::Test {
            seal_keys: Default::default(),
            consumes: Vec::new(),
            cmd: "check".into(),
            line: 1,
            timeout: 5,
            should_fail: false,
            test_name: "t".into(),
            iteration_item: None,
            lua_code: None,
            input_paths: input_paths.iter().map(|s| s.to_string()).collect(),
        }),
        recipe_name: "s".into(),
        // Binding invariant: Test nodes carry no cache_meta (the executor
        // relies on this — see cook-contracts WorkPayload::Test docs).
        cache_meta: None,
        working_dir: wd.to_path_buf(),
        env_vars: Default::default(),
    }
}

/// One cook node (`lib`) producing `build/lib.txt` (command hash
/// `lib_cmd`), plus a test node consuming its own source (`src/own.txt`)
/// and the dep artifact (`build/lib.txt`). The dep output is materialised
/// on disk with `lib_output`, as it would be at the test's ready time.
fn materialised_fixture(
    wd: &std::path::Path,
    lib_cmd: u64,
    lib_output: &str,
    own: &str,
) -> (cook_dag::Dag<crate::WorkNode>, usize) {
    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::create_dir_all(wd.join("build")).unwrap();
    std::fs::write(wd.join("src/lib.txt"), "lib-src").unwrap();
    std::fs::write(wd.join("src/own.txt"), own).unwrap();
    std::fs::write(wd.join("build/lib.txt"), lib_output).unwrap();

    let mut dag = cook_dag::Dag::new();
    let lib = dag
        .add_node(
            cook_work_node(wd, "lib", &["src/lib.txt"], &["build/lib.txt"], lib_cmd),
            &[],
        )
        .unwrap();
    let test = dag
        .add_node(
            test_work_node(wd, &["src/own.txt", "build/lib.txt"]),
            &[lib],
        )
        .unwrap();
    (dag, test)
}

/// COOK-211 / §17.4: a dependency that re-executes (different command
/// hash) but produces byte-identical output leaves the consuming test's
/// fingerprint UNCHANGED — the fold is over the dep's output content, not
/// its execution identity. This is the early cutoff the fix delivers.
#[test]
fn test_fp_stable_when_dep_output_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let (dag_a, test_a) = materialised_fixture(wd, 11, "ok", "own");
    let before = compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());

    // lib's command changes (rebuild) but its output stays "ok".
    let (dag_b, test_b) = materialised_fixture(wd, 12, "ok", "own");
    let after = compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert!(before.is_some());
    assert_eq!(
        before, after,
        "a byte-identical dep OUTPUT must leave the test fingerprint stable \
         even when the dep's command hash changes (early cutoff)"
    );
}

/// The consuming test IS re-keyed when the dep's OUTPUT CONTENT changes.
#[test]
fn test_fp_changes_when_dep_output_content_changes() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let (dag_a, test_a) = materialised_fixture(wd, 11, "ok", "own");
    let before = compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());

    let (dag_b, test_b) = materialised_fixture(wd, 11, "broken", "own");
    let after = compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert_ne!(
        before, after,
        "changing the dep's output content must re-key the test"
    );
}

/// Editing the test's own ingredient re-keys it.
#[test]
fn test_fp_changes_when_own_input_changes() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let (dag_a, test_a) = materialised_fixture(wd, 11, "ok", "own");
    let before = compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());

    let (dag_b, test_b) = materialised_fixture(wd, 11, "ok", "changed");
    let after = compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert_ne!(
        before, after,
        "editing the test's own ingredient must re-key the test"
    );
}

/// A source-less test — no ingredients, no consumed predecessor output —
/// has no cache key and always runs (§8.6.1/§17.4).
#[test]
fn test_fp_none_for_sourceless_test() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let mut dag = cook_dag::Dag::new();
    let test = dag.add_node(test_work_node(wd, &[]), &[]).unwrap();

    assert_eq!(
        compute_ready_test_fingerprint(&dag, test, &ctx, &cook_luaotp::ProbeValueStore::new()),
        None,
        "a source-less test must have no fingerprint (always runs)"
    );
}

/// Three-node chain lib → app → test (test consumes only `build/app.txt`).
/// A change to the TRANSITIVE dep's output (`build/lib.txt`) that leaves
/// the DIRECT dep's output (`build/app.txt`) byte-identical must NOT
/// re-key the test — only the immediate predecessor's output is folded,
/// so early cutoff chains correctly. Changing `build/app.txt` DOES re-key.
#[test]
fn test_fp_two_level_chain_early_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let build = |lib_out: &str, app_out: &str| {
        std::fs::create_dir_all(wd.join("build")).unwrap();
        std::fs::write(wd.join("build/lib.txt"), lib_out).unwrap();
        std::fs::write(wd.join("build/app.txt"), app_out).unwrap();
        let mut dag = cook_dag::Dag::new();
        let lib = dag
            .add_node(cook_work_node(wd, "lib", &[], &["build/lib.txt"], 11), &[])
            .unwrap();
        let app = dag
            .add_node(
                cook_work_node(wd, "app", &["build/lib.txt"], &["build/app.txt"], 22),
                &[lib],
            )
            .unwrap();
        let test = dag
            .add_node(test_work_node(wd, &["build/app.txt"]), &[app])
            .unwrap();
        (dag, test)
    };

    let (dag_a, test_a) = build("lib-v1", "app-out");
    let base = compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());
    assert!(base.is_some());

    // Transitive dep output changes; direct dep output unchanged → stable.
    let (dag_b, test_b) = build("lib-v2", "app-out");
    assert_eq!(
        base,
        compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new()),
        "a transitive dep's output change must not re-key the test when the \
         immediate predecessor's output is byte-identical"
    );

    // Direct dep output changes → re-key.
    let (dag_c, test_c) = build("lib-v2", "app-out-CHANGED");
    assert_ne!(
        base,
        compute_ready_test_fingerprint(&dag_c, test_c, &ctx, &cook_luaotp::ProbeValueStore::new()),
        "the immediate predecessor's output change must re-key the test"
    );
}

// -----------------------------------------------------------------------
// `consumes` — narrowing the predecessor-output fold (§17.4 step 1)
// -----------------------------------------------------------------------

fn test_work_node_consuming(
    wd: &std::path::Path,
    input_paths: &[&str],
    consumes: &[&str],
) -> crate::WorkNode {
    let mut n = test_work_node(wd, input_paths);
    if let Some(WorkPayload::Test { consumes: c, .. }) = n.payload.as_mut() {
        *c = consumes.iter().map(|s| s.to_string()).collect();
    }
    n
}

/// A bundler-shaped dependency: one glob output (`dist/**`) covering BOTH
/// the bundle a consumer imports and the sourcemap sidecar it never opens.
/// This is refine's `packages/core` in miniature — tsup with
/// `sourcemap: true`.
fn bundle_fixture(
    wd: &std::path::Path,
    bundle: &str,
    map: &str,
    consumes: &[&str],
) -> (cook_dag::Dag<crate::WorkNode>, usize) {
    std::fs::create_dir_all(wd.join("dist")).unwrap();
    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::write(wd.join("src/own.txt"), "own").unwrap();
    std::fs::write(wd.join("dist/index.mjs"), bundle).unwrap();
    std::fs::write(wd.join("dist/index.mjs.map"), map).unwrap();

    let mut dag = cook_dag::Dag::new();
    let lib = dag
        .add_node(
            cook_work_node(wd, "lib", &["src/lib.ts"], &["dist/**"], 11),
            &[],
        )
        .unwrap();
    let test = dag
        .add_node(
            test_work_node_consuming(wd, &["src/own.txt"], consumes),
            &[lib],
        )
        .unwrap();
    (dag, test)
}

/// THE CASE THIS SURFACE EXISTS FOR. tsup/esbuild inline `sourcesContent`,
/// so a comment-only edit upstream rewrites `index.mjs.map` byte-for-byte
/// while `index.mjs` stays identical. Without `consumes` the whole `dist/**`
/// folds and the downstream check loses its cached pass over a file it never
/// opened; with it, the check stays keyed on the bundle alone.
#[test]
fn test_fp_stable_when_only_an_unconsumed_sourcemap_changes() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let (dag_a, test_a) = bundle_fixture(wd, "bundle", "map-v1", &["*.mjs"]);
    let before =
        compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());

    let (dag_b, test_b) = bundle_fixture(wd, "bundle", "map-v2", &["*.mjs"]);
    let after =
        compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert!(before.is_some());
    assert_eq!(
        before, after,
        "a changed sourcemap the test does not consume must not re-key it"
    );
}

/// The same edit WITHOUT `consumes` re-keys the test — i.e. the assertion
/// above is testing the new behaviour, not an accident of the fixture.
#[test]
fn test_fp_rekeys_on_sourcemap_change_without_consumes() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let (dag_a, test_a) = bundle_fixture(wd, "bundle", "map-v1", &[]);
    let before =
        compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());

    let (dag_b, test_b) = bundle_fixture(wd, "bundle", "map-v2", &[]);
    let after =
        compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert_ne!(
        before, after,
        "the default fold covers every predecessor output, sourcemaps included"
    );
}

/// `consumes` narrows, it does not blind: a change to a CONSUMED artifact
/// still re-keys. Without this the surface would be a way to turn caching
/// into a lie.
#[test]
fn test_fp_rekeys_when_a_consumed_artifact_changes() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let (dag_a, test_a) = bundle_fixture(wd, "bundle-v1", "map", &["*.mjs"]);
    let before =
        compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());

    let (dag_b, test_b) = bundle_fixture(wd, "bundle-v2", "map", &["*.mjs"]);
    let after =
        compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert_ne!(
        before, after,
        "a changed artifact the test DOES consume must still re-key it"
    );
}

/// Fail-safe: a `consumes` matching none of the predecessor outputs keeps the
/// unnarrowed fold. Silently folding nothing would let a stale pass replay —
/// strictly worse than the over-invalidation the filter removes.
#[test]
fn test_fp_falls_back_to_full_fold_when_consumes_matches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let (dag_a, test_a) = bundle_fixture(wd, "bundle", "map-v1", &["*.wasm"]);
    let before =
        compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());

    let (dag_b, test_b) = bundle_fixture(wd, "bundle", "map-v2", &["*.wasm"]);
    let after =
        compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert_ne!(
        before, after,
        "a consumes matching nothing must fall back to folding everything, \
         not silently drop the dependency-content determinant"
    );
}

/// An excluded artifact stays excluded even when the test's own declared
/// inputs would otherwise sweep it up: `predecessor_outputs` is still the
/// full set for step (2)'s produced-upstream filter, so a `dist/**/*` glob
/// input cannot re-admit the sourcemap through the back door. cook_pnpm's
/// check units declare exactly that glob, so this is the real shape.
#[test]
fn test_fp_excluded_output_is_not_readmitted_via_own_glob_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let build = |map: &str| {
        std::fs::create_dir_all(wd.join("dist")).unwrap();
        std::fs::create_dir_all(wd.join("src")).unwrap();
        std::fs::write(wd.join("src/own.txt"), "own").unwrap();
        std::fs::write(wd.join("dist/index.mjs"), "bundle").unwrap();
        std::fs::write(wd.join("dist/index.mjs.map"), map).unwrap();

        let mut dag = cook_dag::Dag::new();
        let lib = dag
            .add_node(
                cook_work_node(wd, "lib", &["src/lib.ts"], &["dist/**"], 11),
                &[],
            )
            .unwrap();
        let test = dag
            .add_node(
                test_work_node_consuming(wd, &["src/own.txt", "dist/**/*"], &["*.mjs"]),
                &[lib],
            )
            .unwrap();
        (dag, test)
    };

    let (dag_a, test_a) = build("map-v1");
    let before =
        compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());
    let (dag_b, test_b) = build("map-v2");
    let after =
        compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert_eq!(
        before, after,
        "an unconsumed predecessor output must not re-enter the key through \
         the test's own glob inputs"
    );
}

/// Regression: a predecessor declaring the canonical trailing-`**` glob
/// output must actually CONTRIBUTE to the test's key. `resolve_glob` drops
/// directories and the glob crate treats a bare trailing `**` as
/// directories-only, so before CS-0085 normalisation was applied here,
/// `dist/**` resolved to the empty set and the fold silently covered
/// nothing — a real dependency change left the check cached (under-keying,
/// a stale pass). The executor already normalised the same declaration when
/// capturing the producer's outputs; only this path did not.
#[test]
fn test_fp_folds_trailing_double_star_outputs_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    // No `consumes`: the bundle content alone must move the key.
    let (dag_a, test_a) = bundle_fixture(wd, "bundle-v1", "map", &[]);
    let before =
        compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());
    let (dag_b, test_b) = bundle_fixture(wd, "bundle-v2", "map", &[]);
    let after =
        compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert_ne!(
        before, after,
        "a `dist/**` predecessor output must fold into the test key; if this \
         passes trivially the glob resolved to nothing and the check is \
         under-keyed against its dependency"
    );
}

/// The directory-output spelling (`dist/`) had the identical defect via the
/// old `{out}**` construction, and is fixed by the same normalisation.
#[test]
fn test_fp_folds_directory_outputs_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let build = |bundle: &str| {
        std::fs::create_dir_all(wd.join("dist")).unwrap();
        std::fs::create_dir_all(wd.join("src")).unwrap();
        std::fs::write(wd.join("src/own.txt"), "own").unwrap();
        std::fs::write(wd.join("dist/index.mjs"), bundle).unwrap();

        let mut dag = cook_dag::Dag::new();
        let lib = dag
            .add_node(cook_work_node(wd, "lib", &["src/lib.ts"], &["dist/"], 11), &[])
            .unwrap();
        let test = dag
            .add_node(test_work_node_consuming(wd, &["src/own.txt"], &[]), &[lib])
            .unwrap();
        (dag, test)
    };

    let (dag_a, test_a) = build("v1");
    let before =
        compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());
    let (dag_b, test_b) = build("v2");
    let after =
        compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert_ne!(before, after, "a `dist/` directory output must fold into the test key");
}

// ---------------------------------------------------------------------------
// COOK-342 — a source-less test has no key, whatever sits upstream of it
// ---------------------------------------------------------------------------

/// A test node in its OWN recipe, declaring no ingredients, downstream of a
/// cook node belonging to a DIFFERENT recipe — i.e. `recipe check: build`
/// with a bare `test { cargo test }` body. This is cli/Cookfile:36.
fn sourceless_test_behind_bare_dep(
    wd: &std::path::Path,
    dep_output: &str,
) -> (cook_dag::Dag<crate::WorkNode>, usize) {
    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::create_dir_all(wd.join("build")).unwrap();
    std::fs::write(wd.join("src/lib.txt"), "lib-src").unwrap();
    std::fs::write(wd.join("build/lib.txt"), dep_output).unwrap();

    let mut dag = cook_dag::Dag::new();
    let lib = dag
        .add_node(
            cook_work_node(wd, "lib", &["src/lib.txt"], &["build/lib.txt"], 11),
            &[],
        )
        .unwrap();
    // No ingredients, and `lib` is a different recipe, so nothing here is a
    // declared source: not the preceding cook step of §8.6.1 (that is
    // same-recipe), and not a `$<NAME>` reference (which would land in
    // input_paths).
    let mut t = test_work_node(wd, &[]);
    t.recipe_name = "check".into();
    let test = dag.add_node(t, &[lib]).unwrap();
    (dag, test)
}

/// §8.6.1 / §17.4 rule 1, and §8.6.1's Example 8.6.2 verbatim: a test with
/// no consumed output and no `ingredients` has NO cache key and runs
/// uncached. A dep-list entry is a whole-recipe ordering barrier, not a
/// source, so it must not mint one.
#[test]
fn sourceless_test_behind_a_bare_dep_has_no_key() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let (dag, test) = sourceless_test_behind_bare_dep(wd, "ok");
    let fp = compute_ready_test_fingerprint(&dag, test, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert!(
        fp.is_none(),
        "a source-less test keyed off a bare dep edge is the false green \
         §8.6 names: its real inputs are opaque to Cook, so it must always run"
    );
}

/// The teeth of it. Keying the source-less test on the barrier's outputs
/// made its correctness hostage to a file it never reads: the test only
/// re-ran when the unrelated dependency changed.
#[test]
fn sourceless_test_key_does_not_track_an_unrelated_dep() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    let (dag_a, test_a) = sourceless_test_behind_bare_dep(wd, "before");
    let before = compute_ready_test_fingerprint(&dag_a, test_a, &ctx, &cook_luaotp::ProbeValueStore::new());
    let (dag_b, test_b) = sourceless_test_behind_bare_dep(wd, "after");
    let after = compute_ready_test_fingerprint(&dag_b, test_b, &ctx, &cook_luaotp::ProbeValueStore::new());

    assert_eq!(before, None);
    assert_eq!(after, None, "still no key once the dep's output moves");
}

/// The guard is on the DECLARED source, not on the accumulated fold, so a
/// test that DOES declare ingredients keeps its key and keeps folding its
/// predecessors' outputs — the CS-0175 shape must be untouched.
#[test]
fn test_with_own_ingredients_behind_a_dep_still_keys() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let ctx = make_cache_ctx(wd);

    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::write(wd.join("src/own.txt"), "own").unwrap();
    let (_, _) = sourceless_test_behind_bare_dep(wd, "ok");
    let mut dag = cook_dag::Dag::new();
    let lib = dag
        .add_node(
            cook_work_node(wd, "lib", &["src/lib.txt"], &["build/lib.txt"], 11),
            &[],
        )
        .unwrap();
    let mut t = test_work_node(wd, &["src/own.txt"]);
    t.recipe_name = "check".into();
    let test = dag.add_node(t, &[lib]).unwrap();
    let fp = compute_ready_test_fingerprint(&dag, test, &ctx, &cook_luaotp::ProbeValueStore::new());
    assert!(fp.is_some(), "a declared ingredient is a source and mints a key");
}
