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
        // CS-0186: a test unit carries cache_meta like any other unit. It is
        // not read by the input resolver under test — that reads the payload
        // and the working directory — but leaving it `None` would have these
        // fixtures assert against a node shape the engine no longer builds.
        cache_meta: Some(cook_contracts::CacheMeta {
            recipe_name: "s".into(),
            project_id: String::new(),
            cookfile_path: "Cookfile".into(),
            cache_key: ":0000000000000000".into(),
            input_paths: input_paths.iter().map(|s| s.to_string()).collect(),
            output_paths: Vec::new(),
            command_hash: 7,
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

// ===========================================================================
// Ready-time input resolution (§17.4 rule 1)
//
// These were written against `compute_ready_test_fingerprint` and asserted on
// a digest: a file's content was edited between two fixtures and the two
// digests compared. CS-0186 deleted the digest, and porting them exposed that
// the digest was never the subject. Every one of them was really asking WHICH
// PATHS the unit folds — "a changed sourcemap must not re-key the test" is
// "the sourcemap is not in the folded set", reached by a detour through
// content. They now assert the set directly, which is both the actual claim
// and a stronger one: a digest comparison cannot distinguish "the sourcemap
// was excluded" from "the fold covered nothing at all", and that second case
// is a real defect this file has caught before (see the trailing-`**` test).
//
// Content sensitivity is not lost, it moves to where it now lives: the
// recorded input records, compared by `check_inputs` on the next run. It is
// covered end-to-end by cook-engine/tests/dep_output_early_cutoff.rs and by
// cook-cli/tests/runner_caching.rs, which run the real binary.
// ===========================================================================

/// Resolve the input set, sorted, as the engine will record it.
fn fold(dag: &cook_dag::Dag<crate::WorkNode>, test: usize, wd: &std::path::Path) -> Option<Vec<String>> {
    crate::run::consumed_inputs_at_ready_time(dag, test, wd)
}

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
        .add_node(test_work_node(wd, &["src/own.txt", "build/lib.txt"]), &[lib])
        .unwrap();
    (dag, test)
}

/// The immediate predecessor's declared output and the unit's own ingredients,
/// and nothing else. `build/lib.txt` is named by both — as the dep's output
/// and as the test's own input — and appears once.
#[test]
fn the_input_set_is_own_ingredients_plus_immediate_predecessor_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let (dag, test) = materialised_fixture(wd, 11, "ok", "own");

    assert_eq!(
        fold(&dag, test, wd),
        Some(vec!["build/lib.txt".to_string(), "src/own.txt".to_string()])
    );
}

/// The set is a function of the DECLARATION and the graph, not of what the
/// files contain. This is what makes the identity of §17.1.1.1 stable enough
/// to be found twice; content reaches the verdict through the recorded input
/// hashes instead.
#[test]
fn the_input_set_does_not_move_when_content_does() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();

    let (dag_a, test_a) = materialised_fixture(wd, 11, "ok", "own");
    let before = fold(&dag_a, test_a, wd);
    let (dag_b, test_b) = materialised_fixture(wd, 12, "broken", "changed");
    let after = fold(&dag_b, test_b, wd);

    assert!(before.is_some());
    assert_eq!(before, after, "content is not part of the input SET");
}

/// Only the immediate predecessor's outputs are taken, never the transitive
/// closure. That is what keeps early cutoff chaining: `build/lib.txt`'s effect
/// on the test is already carried by `build/app.txt`'s content.
#[test]
fn a_transitive_dependencys_output_is_not_in_the_set() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    std::fs::create_dir_all(wd.join("build")).unwrap();
    std::fs::write(wd.join("build/lib.txt"), "lib").unwrap();
    std::fs::write(wd.join("build/app.txt"), "app").unwrap();

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

    assert_eq!(fold(&dag, test, wd), Some(vec!["build/app.txt".to_string()]));
}

/// A source-less test — no ingredients, no consumed predecessor output — has
/// no cache key and always runs (§8.6.1/§17.4). `None` is what carries that:
/// the executor neither looks a record up nor writes one.
#[test]
fn a_sourceless_test_has_no_input_set() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let mut dag = cook_dag::Dag::new();
    let test = dag.add_node(test_work_node(wd, &[]), &[]).unwrap();

    assert_eq!(fold(&dag, test, wd), None);
}

/// A declared source that resolves to no file on disk is source-less too. The
/// `has_declared_source` guard reads declarations and passes here; this is the
/// second gate, and without it a unit would be filed under a key whose record
/// records nothing.
#[test]
fn a_declared_glob_matching_nothing_leaves_no_input_set() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    std::fs::create_dir_all(wd.join("src")).unwrap();
    let mut dag = cook_dag::Dag::new();
    let test = dag.add_node(test_work_node(wd, &["src/*.nothing"]), &[]).unwrap();

    assert_eq!(fold(&dag, test, wd), None);
}

// -----------------------------------------------------------------------
// `consumes` — narrowing the predecessor-output fold (CS-0175)
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

/// A bundler-shaped dependency: one glob output (`dist/**`) covering BOTH the
/// bundle a consumer imports and the sourcemap sidecar it never opens. This is
/// tsup with `sourcemap: true`, in miniature.
fn bundle_fixture(
    wd: &std::path::Path,
    consumes: &[&str],
) -> (cook_dag::Dag<crate::WorkNode>, usize) {
    std::fs::create_dir_all(wd.join("dist")).unwrap();
    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::write(wd.join("src/own.txt"), "own").unwrap();
    std::fs::write(wd.join("dist/index.mjs"), "bundle").unwrap();
    std::fs::write(wd.join("dist/index.mjs.map"), "map").unwrap();

    let mut dag = cook_dag::Dag::new();
    let lib = dag
        .add_node(cook_work_node(wd, "lib", &["src/lib.ts"], &["dist/**"], 11), &[])
        .unwrap();
    let test = dag
        .add_node(test_work_node_consuming(wd, &["src/own.txt"], consumes), &[lib])
        .unwrap();
    (dag, test)
}

/// THE CASE THIS SURFACE EXISTS FOR. esbuild-family tools inline
/// `sourcesContent`, so a comment-only edit upstream rewrites the sourcemap
/// byte-for-byte while the bundle stays identical. `consumes` keeps the
/// sourcemap out of the set, so rewriting it cannot cost the check its record.
#[test]
fn consumes_keeps_an_unconsumed_artifact_out_of_the_set() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let (dag, test) = bundle_fixture(wd, &["*.mjs"]);

    assert_eq!(
        fold(&dag, test, wd),
        Some(vec!["dist/index.mjs".to_string(), "src/own.txt".to_string()]),
        "the sourcemap must not be folded"
    );
}

/// The assertion above tests the narrowing rather than an accident of the
/// fixture: without `consumes`, the whole `dist/**` is in the set.
#[test]
fn the_default_fold_covers_every_predecessor_output() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let (dag, test) = bundle_fixture(wd, &[]);

    assert_eq!(
        fold(&dag, test, wd),
        Some(vec![
            "dist/index.mjs".to_string(),
            "dist/index.mjs.map".to_string(),
            "src/own.txt".to_string(),
        ])
    );
}

/// Fail-safe: a `consumes` matching none of the predecessor outputs keeps the
/// unnarrowed set. Silently folding nothing would let a stale pass replay,
/// which is strictly worse than the over-invalidation the filter removes.
#[test]
fn a_consumes_matching_nothing_falls_back_to_the_full_set() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let (dag, test) = bundle_fixture(wd, &["*.wasm"]);

    assert_eq!(
        fold(&dag, test, wd),
        Some(vec![
            "dist/index.mjs".to_string(),
            "dist/index.mjs.map".to_string(),
            "src/own.txt".to_string(),
        ]),
        "a filter matching nothing must not narrow the set to nothing"
    );
}

/// An excluded artifact stays excluded even when the unit's OWN declared
/// inputs would otherwise sweep it up: the full predecessor-output set remains
/// the produced-upstream filter, so a `dist/**/*` glob input cannot re-admit
/// the sourcemap through the back door. cook_pnpm's check units declare
/// exactly that glob, so this is the real shape.
#[test]
fn an_excluded_output_is_not_readmitted_through_an_own_glob_input() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    std::fs::create_dir_all(wd.join("dist")).unwrap();
    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::write(wd.join("src/own.txt"), "own").unwrap();
    std::fs::write(wd.join("dist/index.mjs"), "bundle").unwrap();
    std::fs::write(wd.join("dist/index.mjs.map"), "map").unwrap();

    let mut dag = cook_dag::Dag::new();
    let lib = dag
        .add_node(cook_work_node(wd, "lib", &["src/lib.ts"], &["dist/**"], 11), &[])
        .unwrap();
    let test = dag
        .add_node(
            test_work_node_consuming(wd, &["src/own.txt", "dist/**/*"], &["*.mjs"]),
            &[lib],
        )
        .unwrap();

    let set = fold(&dag, test, wd).expect("has a source");
    assert!(
        !set.iter().any(|p| p.ends_with(".map")),
        "an unconsumed predecessor output must not re-enter through the \
         unit's own glob inputs; got {set:?}"
    );
}

/// Regression, and the reason these assertions name paths rather than compare
/// digests. A predecessor declaring the canonical trailing-`**` output must
/// actually CONTRIBUTE. `resolve_glob` drops directories and the glob crate
/// treats a bare trailing `**` as directories-only, so before CS-0085
/// normalisation was applied here `dist/**` resolved to the empty set and the
/// fold silently covered nothing — a real dependency change left the check
/// cached, which is a stale pass. A digest comparison could not tell that from
/// a correct exclusion; naming the path can.
#[test]
fn a_trailing_double_star_output_resolves_to_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let (dag, test) = bundle_fixture(wd, &[]);

    let set = fold(&dag, test, wd).expect("has a source");
    assert!(
        set.contains(&"dist/index.mjs".to_string()),
        "a `dist/**` predecessor output must resolve to its files; got {set:?}"
    );
}

/// The directory-output spelling (`dist/`) had the identical defect via the
/// old `{out}**` construction, and is fixed by the same normalisation.
#[test]
fn a_directory_output_resolves_to_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    std::fs::create_dir_all(wd.join("dist")).unwrap();
    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::write(wd.join("src/own.txt"), "own").unwrap();
    std::fs::write(wd.join("dist/index.mjs"), "bundle").unwrap();

    let mut dag = cook_dag::Dag::new();
    let lib = dag
        .add_node(cook_work_node(wd, "lib", &["src/lib.ts"], &["dist/"], 11), &[])
        .unwrap();
    let test = dag
        .add_node(test_work_node_consuming(wd, &["src/own.txt"], &[]), &[lib])
        .unwrap();

    let set = fold(&dag, test, wd).expect("has a source");
    assert!(
        set.contains(&"dist/index.mjs".to_string()),
        "a `dist/` directory output must resolve to its files; got {set:?}"
    );
}

// ---------------------------------------------------------------------------
// COOK-342 — a source-less test has no key, whatever sits upstream of it
// ---------------------------------------------------------------------------

/// A test node in its OWN recipe, declaring no ingredients, downstream of a
/// cook node belonging to a DIFFERENT recipe — i.e. `recipe check: build`
/// with a bare `test { cargo test }` body.
fn sourceless_test_behind_bare_dep(
    wd: &std::path::Path,
) -> (cook_dag::Dag<crate::WorkNode>, usize) {
    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::create_dir_all(wd.join("build")).unwrap();
    std::fs::write(wd.join("src/lib.txt"), "lib-src").unwrap();
    std::fs::write(wd.join("build/lib.txt"), "ok").unwrap();

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

/// §8.6.1's Example 8.6.2 verbatim: a dep-list entry is a whole-recipe
/// ordering barrier, not a source, and must not mint a key. Keying the
/// source-less test on the barrier's outputs made its correctness hostage to a
/// file it never reads — it re-ran only when the unrelated dependency changed.
#[test]
fn a_sourceless_test_behind_a_bare_dep_has_no_input_set() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let (dag, test) = sourceless_test_behind_bare_dep(wd);

    assert_eq!(
        fold(&dag, test, wd),
        None,
        "a source-less test keyed off a bare dep edge is the false green \
         §8.6 names: its real inputs are opaque to Cook, so it must always run"
    );
}

/// The guard is on the DECLARED source, not on the accumulated set, so a test
/// that DOES declare ingredients keeps its key and keeps folding its
/// predecessors' outputs.
#[test]
fn a_declared_ingredient_behind_a_bare_dep_still_gives_an_input_set() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::create_dir_all(wd.join("build")).unwrap();
    std::fs::write(wd.join("src/lib.txt"), "lib-src").unwrap();
    std::fs::write(wd.join("src/own.txt"), "own").unwrap();
    std::fs::write(wd.join("build/lib.txt"), "ok").unwrap();

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

    // The dep's output IS folded once a source is declared: the guard decides
    // whether a key exists, never which paths reach it.
    assert_eq!(
        fold(&dag, test, wd),
        Some(vec!["build/lib.txt".to_string(), "src/own.txt".to_string()])
    );
}
