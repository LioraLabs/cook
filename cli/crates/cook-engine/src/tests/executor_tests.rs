use super::*;
use crate::Stream;
use std::path::PathBuf;
use tempfile::TempDir;

fn shell(cmd: &str) -> WorkPayload {
    WorkPayload::Shell {
        cmd: cmd.to_string(),
        line: 0,
    }
}

#[test]
fn build_determinant_manifest_captures_resolved_determinants() {
    use cook_fingerprint::FileRecord;
    use std::collections::{BTreeMap, BTreeSet};
    let inputs = vec![
        FileRecord {
            path: "src/b.c".into(),
            mtime: 0,
            hash: 0x2222,
        },
        FileRecord {
            path: "src/a.c".into(),
            mtime: 0,
            hash: 0x1111,
        },
    ];
    let mut consulted = BTreeMap::new();
    consulted.insert("CC".to_string(), "clang".to_string());
    let mut seal_keys = BTreeSet::new();
    seal_keys.insert("host".to_string());
    let store = cook_luaotp::ProbeValueStore::new();
    store.insert("host", b"\"x86_64-linux\"".to_vec());

    let m = build_determinant_manifest(
        CACHE_VERSION,
        "cook/Cookfile::build",
        &[0xABu8; 32],
        0x1234,
        0x5678,
        0x9abc,
        &inputs,
        &["build/a.o".to_string()],
        &[],
        &consulted,
        &seal_keys,
        &store,
    );
    assert_eq!(m.recipe_namespace, "cook/Cookfile::build");
    assert_eq!(m.key, "ab".repeat(32));
    assert_eq!(m.inputs["src/a.c"], 0x1111);
    assert_eq!(m.inputs["src/b.c"], 0x2222);
    assert_eq!(m.output_paths, vec!["build/a.o".to_string()]);
    assert_eq!(m.consulted_env["CC"], "clang");
    assert_eq!(m.sealed_probes["host"], "\"x86_64-linux\"");
}

fn tmp_dir() -> (PathBuf, TempDir) {
    let d = TempDir::new().unwrap();
    (d.path().to_path_buf(), d)
}

fn default_env() -> BTreeMap<String, String> {
    BTreeMap::new()
}

fn work_node(payload: WorkPayload, recipe: &str, wd: PathBuf) -> WorkNode {
    WorkNode {
        process_env_vars: std::collections::BTreeMap::new(),
        payload: Some(payload),
        recipe_name: recipe.to_string(),
        cache_meta: None,
        working_dir: wd,
        env_vars: default_env(),
    }
}

fn presatisfied_node(recipe: &str, wd: PathBuf) -> WorkNode {
    WorkNode {
        process_env_vars: std::collections::BTreeMap::new(),
        payload: None,
        recipe_name: recipe.to_string(),
        cache_meta: None,
        working_dir: wd,
        env_vars: default_env(),
    }
}

/// Build a minimal CacheContext backed by a temp-dir LocalBackend.
/// Suitable for executor tests that don't exercise the cache path.
fn make_cache_ctx(tmp: &TempDir) -> Arc<CacheContext> {
    use cook_cache::{
        backend::LocalBackend, cache_ctx::CacheContext, cloud_config::CloudConfig,
    };
    use cook_fingerprint::EnvDenylist;
    Arc::new(CacheContext {
        denylist: Arc::new(EnvDenylist::baseline()),
        backend: Arc::new(LocalBackend::new(tmp.path().join("cloud"))),
        cloud_config: Arc::new(CloudConfig::default()),
        project_root: tmp.path().to_path_buf(),
        project_id: "test".to_string(),
        publish_enabled: true,
    })
}

// 1. Single node succeeds
#[test]
fn test_executor_runs_single_node() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);
    let mut dag = Dag::new();
    dag.add_node(work_node(shell("true"), "single", wd), &[]).unwrap();

    let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

// 2. Dependencies respected: A writes file, B reads it
#[test]
fn test_executor_respects_dependencies() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    let a = dag.add_node(
        work_node(shell("echo hello > output.txt"), "writer", wd.clone()),
            &[],
        ).unwrap();
        dag.add_node(
            work_node(shell("cat output.txt"), "reader", wd),
        &[a],
    ).unwrap();

    let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

// 3. Failure cancels downstream
#[test]
fn test_executor_failure_cancels_downstream() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    let a = dag.add_node(work_node(shell("false"), "fail_a", wd.clone()), &[]).unwrap();
    // B depends on A — should never run.
    dag.add_node(
        work_node(
            shell("echo should_not_run > /tmp/cook_test_should_not_exist"),
            "downstream_b",
            wd,
        ),
        &[a],
    ).unwrap();

    let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_err());
    match result.unwrap_err() {
        EngineError::TaskFailures { failures, .. } => {
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].1, "fail_a");
        }
        other => panic!("expected TaskFailures, got: {other:?}"),
    }
}

// 4. Parallel independent nodes (timing)
#[test]
fn test_executor_parallel_independent_nodes() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    for i in 0..4 {
        dag.add_node(
            work_node(shell("sleep 0.2"), &format!("sleep_{i}"), wd.clone()),
            &[],
        ).unwrap();
    }

    let start = std::time::Instant::now();
    let result = execute_dag(dag, 4, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    let elapsed = start.elapsed();

    assert!(result.is_ok(), "expected Ok, got: {result:?}");
    // With 4 workers, 4 x sleep 0.2 should take ~0.2s, not ~0.8s.
    assert!(
        elapsed.as_secs_f64() < 0.6,
        "took too long ({:.2}s), likely not parallel",
        elapsed.as_secs_f64()
    );
}

// 5. Empty DAG
#[test]
fn test_executor_empty_dag() {
    let (_wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);
    let dag: Dag<WorkNode> = Dag::new();
    let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok());
}

// 6. Presatisfied chain: presatisfied A -> presatisfied B -> work C
#[test]
fn test_executor_presatisfied_chain() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    let a = dag.add_node(presatisfied_node("cached_a", wd.clone()), &[]).unwrap();
    let b = dag.add_node(presatisfied_node("cached_b", wd.clone()), &[a]).unwrap();
    dag.add_node(work_node(shell("true"), "real_work", wd), &[b]).unwrap();

    let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

// 7. Failure does not cancel independent nodes
#[test]
fn test_executor_failure_does_not_cancel_independent() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    // A will fail
    dag.add_node(work_node(shell("false"), "fail_a", wd.clone()), &[]).unwrap();
    // B is independent, should succeed
    dag.add_node(work_node(shell("true"), "ok_b", wd), &[]).unwrap();

    let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_err());
    match result.unwrap_err() {
        EngineError::TaskFailures { failures, .. } => {
            // Only A should be in failures
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].1, "fail_a");
        }
        other => panic!("expected TaskFailures, got: {other:?}"),
    }
}

// 8. Interactive node runs after pool drains
#[test]
fn test_executor_interactive_node() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    let a = dag.add_node(work_node(shell("echo setup"), "setup", wd.clone()), &[]).unwrap();
    dag.add_node(
        work_node(
            WorkPayload::Interactive {
                cmd: "echo interactive".to_string(),
                line: 5,
                is_chore: false,
            },
            "run",
            wd,
        ),
        &[a],
    ).unwrap();

    let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

// 9. CS-0035: OutputLine events carry true fd-of-origin in the `stream`
//    field instead of attributing every captured byte to stdout.  Pre-fix,
//    both the success branch and the failure branch in execute_dag
//    hardcoded `is_stderr: false`, so any `Stream::Stderr` value rendered
//    in events.jsonl was unreachable end-to-end.
#[test]
fn test_executor_output_line_stream_reflects_fd_of_origin() {
    use std::sync::mpsc;

    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    // A shell command that emits one line to stdout and one to stderr.
    // The captured bytes' fds must round-trip through OutputLine events.
    let mut dag = Dag::new();
    dag.add_node(
        work_node(
            shell("echo to-stdout; echo to-stderr 1>&2"),
            "mixed",
            wd,
        ),
        &[],
    )
    .unwrap();

    let (tx, rx) = mpsc::channel::<EngineEvent>();
    let result = execute_dag(dag, 1, BTreeMap::new(), Some(tx), cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "expected Ok, got: {result:?}");

    let mut got_stdout = false;
    let mut got_stderr = false;
    while let Ok(event) = rx.try_recv() {
        if let EngineEvent::OutputLine { line, stream, .. } = event {
            match stream {
                Stream::Stdout => {
                    assert_eq!(line, "to-stdout", "stdout line content");
                    got_stdout = true;
                }
                Stream::Stderr => {
                    assert_eq!(line, "to-stderr", "stderr line content");
                    got_stderr = true;
                }
                _ => panic!("unexpected non-exhaustive Stream variant"),
            }
        }
    }
    assert!(got_stdout, "expected an OutputLine with stream=Stdout");
    assert!(got_stderr, "expected an OutputLine with stream=Stderr");
    }

    // ---------------------------------------------------------------------
    // CS-0050: engine MUST mkdir -p the parent of every declared cook-step
    // output before the step runs.
    // ---------------------------------------------------------------------

    fn cook_meta(output_paths: Vec<&str>) -> cook_contracts::CacheMeta {
        cook_contracts::CacheMeta {
            recipe_name: "r".into(),
        project_id: "test".into(),
        cookfile_path: "Cookfile".into(),
        cache_key: "k".into(),
        input_paths: vec![],
        output_paths: output_paths.into_iter().map(String::from).collect(),
        command_hash: 0,
        env_contribution: 0,
        consulted_env: BTreeMap::new(),
        discovered_inputs: None,
        seal_keys: Default::default(),
        sharing: Default::default(),
        record: false,
    }
}

fn cook_node(payload: WorkPayload, recipe: &str, wd: PathBuf, outputs: Vec<&str>) -> WorkNode {
    WorkNode {
        process_env_vars: std::collections::BTreeMap::new(),
        payload: Some(payload),
        recipe_name: recipe.to_string(),
        cache_meta: Some(cook_meta(outputs)),
        working_dir: wd,
        env_vars: default_env(),
    }
}

/// Build a cook node carrying a COOK-162 disposition (`local`/`pinned`) on
/// its CacheMeta. The CacheMeta's `recipe_name` is set to match the node's
/// recipe so `check_node_cache`'s cache-manager lookup resolves.
fn cook_node_disposition(
    payload: WorkPayload,
    recipe: &str,
    wd: PathBuf,
    outputs: Vec<&str>,
    sharing: cook_contracts::Sharing,
) -> WorkNode {
    let mut meta = cook_meta(outputs);
    meta.recipe_name = recipe.to_string();
    meta.cache_key = format!("k_{recipe}");
    meta.sharing = sharing;
    WorkNode {
        process_env_vars: std::collections::BTreeMap::new(),
        payload: Some(payload),
        recipe_name: recipe.to_string(),
        cache_meta: Some(meta),
        working_dir: wd,
        env_vars: default_env(),
    }
}

/// A `cache_managers` map carrying one fresh, empty manager for `recipe`,
/// backed by a temp cache dir. Required so `check_node_cache` does not
/// short-circuit to Miss on a missing manager.
fn empty_cache_managers(recipe: &str, dir: &std::path::Path) -> BTreeMap<String, Arc<ThreadSafeCacheManager>> {
    let mut m = BTreeMap::new();
    m.insert(
        recipe.to_string(),
        Arc::new(ThreadSafeCacheManager::new(dir.to_path_buf())),
    );
    m
}

// COOK-162 §3 sharing — `local` unit with no local StepEntry and an EMPTY
// shared backend must NOT consult the backend and must fall through to a
// normal rebuild (Miss). The node runs, produces its output, and succeeds.
#[test]
fn test_executor_cook162_local_cold_miss_rebuilds() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);
    let managers = empty_cache_managers("loc", _tmp.path());

    let mut dag = Dag::new();
    dag.add_node(
        cook_node_disposition(
            shell("echo hi > out.txt"),
            "loc",
            wd.clone(),
            vec!["out.txt"],
            cook_contracts::Sharing::Local,
        ),
        &[],
    )
    .unwrap();

    let result = execute_dag(dag, 1, managers, None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "local cold-miss should rebuild, got: {result:?}");
    assert!(wd.join("out.txt").exists(), "local unit should have run");
}

// COOK-162 §3 sharing — `pinned` (fetch-only) unit absent from BOTH the
// local index and the EMPTY shared backend is a HARD ERROR. The unit MUST
// NOT be dispatched/rebuilt; execute_dag returns TaskFailures and the
// declared output is never produced.
#[test]
fn test_executor_cook162_pinned_cold_miss_is_fatal() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);
    let managers = empty_cache_managers("pin", _tmp.path());

    let mut dag = Dag::new();
    dag.add_node(
        cook_node_disposition(
            // If this ran, it would create out.txt — it MUST NOT.
            shell("echo hi > out.txt"),
            "pin",
            wd.clone(),
            vec!["out.txt"],
            cook_contracts::Sharing::Pinned,
        ),
        &[],
    )
    .unwrap();

    let result = execute_dag(dag, 1, managers, None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    let err = result.expect_err("pinned cold-miss must be fatal");
    match err {
        EngineError::TaskFailures { failures, .. } => {
            assert_eq!(failures.len(), 1, "exactly one failure expected");
            assert_eq!(failures[0].1, "pin");
            assert!(
                failures[0].2.contains("pinned") && failures[0].2.contains("fetch-only"),
                "diagnostic should explain the fetch-only contract; got: {}",
                failures[0].2
            );
        }
        other => panic!("expected TaskFailures, got: {other:?}"),
    }
    assert!(
        !wd.join("out.txt").exists(),
        "pinned cold-miss MUST NOT dispatch the unit"
    );
}

// 10. CS-0050: a cook step's missing output parent dir is created
//     before the shell text runs, so authors can drop `mkdir -p`.
#[test]
fn test_executor_cs_0050_creates_missing_output_parent() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    // Output sits in `build/out/foo.txt` — neither `build` nor
    // `build/out` exists when the step starts. The shell text has NO
    // `mkdir -p` boilerplate.
    let mut dag = Dag::new();
    dag.add_node(
        cook_node(
            shell("echo hi > build/out/foo.txt"),
            "build",
            wd.clone(),
            vec!["build/out/foo.txt"],
        ),
        &[],
    )
    .unwrap();

    let result = execute_dag(dag, 1, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "expected Ok, got: {result:?}");

    let out = wd.join("build/out/foo.txt");
    assert!(out.exists(), "output {} not created", out.display());
    let body = std::fs::read_to_string(&out).unwrap();
    assert_eq!(body.trim_end(), "hi");
}

// 11. CS-0050: when the parent path resolves to a non-directory (a
//     regular file), the engine MUST surface a clear diagnostic
//     naming the output and the offending parent, NOT execute the
//     shell text, and NOT attempt to overwrite the file.
#[test]
fn test_executor_cs_0050_parent_is_file_diagnostic() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    // Make `build` a regular file; then declare an output whose
    // parent is `build/`.
    std::fs::write(wd.join("build"), b"not a dir").unwrap();

        let mut dag = Dag::new();
        dag.add_node(
            cook_node(
                shell("echo hi > build/foo.txt"),
            "build",
            wd.clone(),
            vec!["build/foo.txt"],
        ),
        &[],
    )
    .unwrap();

    let result = execute_dag(dag, 1, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    let err = result.expect_err("expected failure when parent is a regular file");
    match err {
        EngineError::TaskFailures { failures, .. } => {
            assert_eq!(failures.len(), 1);
            let msg = &failures[0].2;
            assert!(
                msg.contains("CS-0050"),
                "diagnostic should be tagged CS-0050; got: {msg}"
            );
            assert!(
                msg.contains("build/foo.txt"),
                "diagnostic should name the declared output; got: {msg}"
            );
            assert!(
                msg.contains("non-directory") || msg.contains("non-directory"),
                "diagnostic should explain why mkdir failed; got: {msg}"
            );
        }
        other => panic!("expected TaskFailures, got: {other:?}"),
    }

    // The `build` regular file MUST NOT have been overwritten.
    let body = std::fs::read_to_string(wd.join("build")).unwrap();
    assert_eq!(body, "not a dir");
        // And the declared output MUST NOT exist.
        assert!(!wd.join("build/foo.txt").exists());
}

// 12. CS-0050: the call is a no-op when cache_meta is absent (plate /
//     test units, presatisfied units) — those paths must not regress.
//     Exercised by the existing `test_executor_runs_single_node`
//     baseline; this test pins idempotence on a cook step whose
//     parent already exists as a directory.
#[test]
fn test_executor_cs_0050_idempotent_when_parent_exists() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    std::fs::create_dir_all(wd.join("build")).unwrap();

    let mut dag = Dag::new();
    dag.add_node(
        cook_node(
            shell("echo hi > build/foo.txt"),
            "build",
            wd.clone(),
            vec!["build/foo.txt"],
        ),
        &[],
    )
    .unwrap();

    let result = execute_dag(dag, 1, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
    assert!(wd.join("build/foo.txt").exists());
}

// 13. CS-0050 unit-level helper: an output with no parent component
//     (root-level path) is a no-op.
#[test]
fn test_ensure_output_parent_dirs_no_parent_is_noop() {
    let (wd, _tmp) = tmp_dir();
    let node = cook_node(shell("true"), "r", wd.clone(), vec!["out.txt"]);
    // wd.join("out.txt").parent() == Some(wd) which exists.
    ensure_output_parent_dirs(&node).expect("no parent component should be a no-op");
}

// -----------------------------------------------------------------
// CS-0051: chore-window grouping. A chore body MUST execute as a
// single drain — one InteractiveStart/InteractiveEnd pair covers all
// body steps, and the recipe completion event carries `kind: Chore`.
// -----------------------------------------------------------------

#[test]
fn chore_window_groups_consecutive_chore_steps_into_one_pair() {
    use std::sync::mpsc;
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    // Three chore steps (is_chore=true) for one recipe — they must group.
    let a = dag.add_node(
        work_node(
            WorkPayload::Interactive { cmd: "true".into(), line: 1, is_chore: true },
            "chore", wd.clone()),
        &[]).unwrap();
    let b = dag.add_node(
        work_node(
            WorkPayload::Interactive { cmd: "true".into(), line: 2, is_chore: true },
            "chore", wd.clone()),
        &[a]).unwrap();
    dag.add_node(
        work_node(
            WorkPayload::Interactive { cmd: "true".into(), line: 3, is_chore: true },
            "chore", wd.clone()),
        &[b]).unwrap();

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "got: {result:?}");

    let events: Vec<_> = rx.try_iter().collect();
    let starts = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveStart { .. })).count();
    let ends = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveEnd { .. })).count();
    assert_eq!(starts, 1, "exactly one InteractiveStart per chore window; got events:\n{events:#?}");
    assert_eq!(ends, 1, "exactly one InteractiveEnd per chore window; got events:\n{events:#?}");

    match events.iter().find(|e| matches!(e, EngineEvent::InteractiveStart { .. })).unwrap() {
        EngineEvent::InteractiveStart { chore_step_count, .. } => {
            assert_eq!(*chore_step_count, 3);
        }
        _ => unreachable!(),
    }

    // RecipeCompleted MUST carry kind: Chore for chore recipes.
    let recipe_completed = events
        .iter()
        .find(|e| matches!(e, EngineEvent::RecipeCompleted { .. }))
        .expect("expected RecipeCompleted event");
    match recipe_completed {
        EngineEvent::RecipeCompleted { kind, .. } => {
            assert_eq!(*kind, RecipeKind::Chore);
        }
        _ => unreachable!(),
    }
}

#[test]
fn chore_window_failure_mid_run_emits_one_node_failed_with_step_index() {
    use std::sync::mpsc;
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    let a = dag.add_node(
        work_node(
            WorkPayload::Interactive { cmd: "true".into(), line: 1, is_chore: true },
            "chore", wd.clone()),
        &[]).unwrap();
    let b = dag.add_node(
        work_node(
            WorkPayload::Interactive { cmd: "false".into(), line: 2, is_chore: true },
            "chore", wd.clone()),
        &[a]).unwrap();
    dag.add_node(
        work_node(
            WorkPayload::Interactive { cmd: "true".into(), line: 3, is_chore: true },
            "chore", wd),
        &[b]).unwrap();

    let (tx, rx) = mpsc::channel();
    let _result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));

    let events: Vec<_> = rx.try_iter().collect();
    let node_failed: Vec<_> = events.iter().filter(|e| matches!(e, EngineEvent::NodeFailed { .. })).collect();
    assert_eq!(node_failed.len(), 1, "exactly one NodeFailed per chore failure; got: {events:#?}");
    match node_failed[0] {
        EngineEvent::NodeFailed { error, .. } => {
            assert!(error.contains("step 2/3"), "expected 'step 2/3' in error, got: {error}");
        }
        _ => unreachable!(),
    }

    let end = events.iter().find(|e| matches!(e, EngineEvent::InteractiveEnd { .. })).unwrap();
    match end {
        EngineEvent::InteractiveEnd { failed_step, success, .. } => {
            assert_eq!(*failed_step, Some(2));
            assert!(!*success);
        }
        _ => unreachable!(),
    }
}

#[test]
fn non_chore_interactive_still_emits_per_node_pair() {
    use std::sync::mpsc;
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    dag.add_node(
        work_node(
            WorkPayload::Interactive {
                cmd: "echo legacy".into(),
                line: 1,
                is_chore: false,
            },
            "step", wd),
        &[]).unwrap();

    let (tx, rx) = mpsc::channel();
    let _result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));

    let events: Vec<_> = rx.try_iter().collect();
    let starts = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveStart { .. })).count();
    let ends = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveEnd { .. })).count();
    assert_eq!(starts, 1);
    assert_eq!(ends, 1);
    // chore_step_count must be 0 to flag the legacy path.
    match events.iter().find(|e| matches!(e, EngineEvent::InteractiveStart { .. })).unwrap() {
        EngineEvent::InteractiveStart { chore_step_count, .. } => assert_eq!(*chore_step_count, 0),
        _ => unreachable!(),
    }
}

// -----------------------------------------------------------------
// CS-0051 Lua-bundle integration: the chore-window drain admits Lua-
// bundle steps alongside shell steps. A mixed shell+Lua chore body
// produces a single InteractiveStart/End pair; a pure-Lua chore body
// does likewise; non-chore LuaChunks still route through the worker
// pool (regression guard).
// -----------------------------------------------------------------

#[test]
fn chore_window_groups_shell_and_lua_into_one_pair() {
    use std::sync::mpsc;
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    let a = dag.add_node(
        work_node(
            WorkPayload::Interactive { cmd: "true".into(), line: 1, is_chore: true },
            "shell1", wd.clone()),
        &[]).unwrap();
    let b = dag.add_node(
        work_node(
            WorkPayload::LuaChunk {
                code: "-- noop".into(),
                inputs: vec![],
                outputs: vec![],
                ingredient_groups: vec![],
                step_kind: cook_contracts::StepKind::Chore,
                is_chore: true,
                line: 0,
            },
            "shell1", wd.clone()),
        &[a]).unwrap();
    dag.add_node(
        work_node(
            WorkPayload::Interactive { cmd: "true".into(), line: 3, is_chore: true },
            "shell1", wd),
        &[b]).unwrap();

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "got: {result:?}");

    let events: Vec<_> = rx.try_iter().collect();
    let starts = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveStart { .. })).count();
    let ends = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveEnd { .. })).count();
    assert_eq!(
        starts, 1,
        "mixed shell+lua chore body must produce ONE InteractiveStart; got events:\n{events:#?}"
    );
    assert_eq!(
        ends, 1,
        "mixed shell+lua chore body must produce ONE InteractiveEnd; got events:\n{events:#?}"
    );
    match events.iter().find(|e| matches!(e, EngineEvent::InteractiveStart { .. })).unwrap() {
        EngineEvent::InteractiveStart { chore_step_count, .. } => {
            assert_eq!(*chore_step_count, 3, "chore_step_count covers all three body steps");
        }
        _ => unreachable!(),
    }
}

#[test]
fn pure_lua_chore_body_produces_one_drain_window() {
    use std::sync::mpsc;
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    let a = dag.add_node(
        work_node(
            WorkPayload::LuaChunk {
                code: "-- noop".into(),
                inputs: vec![],
                outputs: vec![],
                ingredient_groups: vec![],
                step_kind: cook_contracts::StepKind::Chore,
                is_chore: true,
                line: 0,
            },
            "lua_chore", wd.clone()),
        &[]).unwrap();
    dag.add_node(
        work_node(
            WorkPayload::LuaChunk {
                code: "-- noop".into(),
                inputs: vec![],
                outputs: vec![],
                ingredient_groups: vec![],
                step_kind: cook_contracts::StepKind::Chore,
                is_chore: true,
                line: 0,
            },
            "lua_chore", wd),
        &[a]).unwrap();

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "got: {result:?}");

    let events: Vec<_> = rx.try_iter().collect();
    let starts = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveStart { .. })).count();
    let ends = events.iter().filter(|e| matches!(e, EngineEvent::InteractiveEnd { .. })).count();
    assert_eq!(starts, 1, "pure-lua chore body must produce ONE InteractiveStart; got: {events:#?}");
    assert_eq!(ends, 1, "pure-lua chore body must produce ONE InteractiveEnd; got: {events:#?}");
    match events.iter().find(|e| matches!(e, EngineEvent::InteractiveStart { .. })).unwrap() {
        EngineEvent::InteractiveStart { chore_step_count, .. } => {
            assert_eq!(*chore_step_count, 2);
        }
        _ => unreachable!(),
    }
}

#[test]
fn non_chore_lua_chunk_still_dispatches_to_worker_pool() {
    // Regression: a LuaChunk with `is_chore = false` MUST continue to
    // route through the worker pool (its is_chore = false means it is
    // a regular cook/test/plate body, not a chore-window member).
    // We pin this by exercising a one-node DAG; the engine must
    // complete without queuing the unit on the interactive_queue.
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    dag.add_node(
        work_node(
            WorkPayload::LuaChunk {
                code: "-- noop".into(),
                inputs: vec![],
                outputs: vec![],
                ingredient_groups: vec![],
                step_kind: cook_contracts::StepKind::Cook,
                is_chore: false,
                line: 0,
            },
            "regular_lua", wd),
        &[]).unwrap();

    let result = execute_dag(dag, 2, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    assert!(result.is_ok(), "got: {result:?}");
}

// SHI-173: a failing cook step must produce Blocked TestResult rows for
// downstream test nodes, not short-circuit to EngineError::TaskFailures
// with no test results.
//
// In cook-mode callers (run()) this error still propagates unchanged.
// In test-mode (run_for_test_inner()) the Blocked rows are extracted and
// the error is swallowed. This test verifies the executor side: that
// TaskFailures.partial_test_results contains the Blocked row.
#[test]
fn cook_failure_produces_blocked_test_result() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    // Cook node that will always fail.
    let cook = dag.add_node(
        work_node(shell("false"), "blocked_by_build", wd.clone()),
        &[],
    ).unwrap();
    // Test node downstream of the failing cook node.
    dag.add_node(
        work_node(
            WorkPayload::Test {
                seal_keys: Default::default(),
                consumes: Vec::new(),
                cmd: "true".to_string(),
                line: 1,
                timeout: 30,
                should_fail: false,
                suite_name: "blocked_by_build".to_string(),
                test_name: "my_test".to_string(),
                iteration_item: None,
                lua_code: None,
                input_paths: vec![],
            },
            "blocked_by_build",
            wd.clone(),
        ),
        &[cook],
    ).unwrap();

    let result = execute_dag(
        dag, 2, BTreeMap::new(), None, cache_ctx, None, &[], &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
    );

    // The cook node failed → EngineError::TaskFailures
    let err = result.expect_err("expected TaskFailures due to failing cook node");
    match err {
        EngineError::TaskFailures { failures, partial_test_results, .. } => {
            // One cook failure.
            assert_eq!(failures.len(), 1, "expected 1 cook failure");
            assert_eq!(failures[0].1, "blocked_by_build");
            // Exactly one Blocked TestResult for the downstream test node.
            assert_eq!(
                partial_test_results.len(), 1,
                "expected 1 Blocked TestResult in partial_test_results"
            );
            let blocked = &partial_test_results[0];
            assert_eq!(blocked.outcome, crate::TestOutcome::Blocked);
            assert_eq!(blocked.name, "my_test");
            assert!(
                blocked.blocked_by.is_some(),
                "blocked_by should be populated"
            );
        }
        other => panic!("expected TaskFailures, got: {other:?}"),
    }
}

// SHI-line: WorkPayload::Test { line } must propagate into TestStarted and
// TestResult.line rather than remaining 0.
#[test]
fn test_line_number_propagates_from_payload_to_events() {
    use std::sync::mpsc;
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    dag.add_node(
        work_node(
            WorkPayload::Test {
                seal_keys: Default::default(),
                consumes: Vec::new(),
                cmd: "true".to_string(),
                line: 17,
                timeout: 30,
                should_fail: false,
                suite_name: "my_recipe".to_string(),
                test_name: "my_test".to_string(),
                iteration_item: None,
                lua_code: None,
                input_paths: vec![],
            },
            "my_recipe",
            wd,
        ),
        &[],
    ).unwrap();

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    let test_results = result.expect("test node should pass");

    // TestResult.line must carry 17.
    assert_eq!(test_results.len(), 1, "expected exactly one TestResult");
    assert_eq!(
        test_results[0].line, 17,
        "TestResult.line should be 17 (from WorkPayload::Test {{ line: 17 }})"
    );

    // The TestStarted event must also carry line 17.
    let events: Vec<_> = rx.try_iter().collect();
    let started = events.iter().find(|e| matches!(e, EngineEvent::TestStarted { .. }))
        .expect("expected a TestStarted event");
    match started {
        EngineEvent::TestStarted { line, .. } => {
            assert_eq!(*line, 17, "TestStarted.line should be 17");
        }
        _ => unreachable!(),
    }

    // The TestPassed event must also carry line 17.
    let passed = events.iter().find(|e| matches!(e, EngineEvent::TestPassed { .. }))
        .expect("expected a TestPassed event");
    match passed {
        EngineEvent::TestPassed { line, .. } => {
            assert_eq!(*line, 17, "TestPassed.line should be 17");
        }
        _ => unreachable!(),
    }
}

#[test]
fn test_iteration_item_propagates() {
    use std::sync::mpsc;
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    dag.add_node(
        work_node(
            WorkPayload::Test {
                seal_keys: Default::default(),
                consumes: Vec::new(),
                cmd: "true".to_string(),
                line: 17,
                timeout: 30,
                should_fail: false,
                suite_name: "my_recipe".to_string(),
                test_name: "my_test".to_string(),
                iteration_item: Some("a.cpp".into()),
                lua_code: None,
                input_paths: vec![],
            },
            "my_recipe",
            wd,
        ),
        &[],
    ).unwrap();

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(dag, 2, BTreeMap::new(), Some(tx), cache_ctx, None, &[], &BTreeMap::new(), std::sync::Arc::new(BTreeMap::new()));
    let test_results = result.expect("test node should pass");

    // TestResult.iteration_item must carry "a.cpp".
    assert_eq!(test_results.len(), 1, "expected exactly one TestResult");
    assert_eq!(
        test_results[0].iteration_item,
        Some("a.cpp".into()),
        "TestResult.iteration_item should be Some(\"a.cpp\")"
    );
    // TestResult.id must end with "[a.cpp]".
    assert!(
        test_results[0].id.0.ends_with("[a.cpp]"),
        "TestResult.id should end with [a.cpp], got: {}",
        test_results[0].id.0
    );

    // The TestStarted event must carry iteration_item = Some("a.cpp").
    let events: Vec<_> = rx.try_iter().collect();
    let started = events.iter().find(|e| matches!(e, EngineEvent::TestStarted { .. }))
        .expect("expected a TestStarted event");
    match started {
        EngineEvent::TestStarted { iteration_item, .. } => {
            assert_eq!(
                *iteration_item,
                Some("a.cpp".into()),
                "TestStarted.iteration_item should be Some(\"a.cpp\")"
            );
        }
        _ => unreachable!(),
    }
}

// -----------------------------------------------------------------
// CS-0074 G4/G5: probe cache hit/miss/invalidation.
//
// These tests exercise the G4 (cache lookup before dispatch) and G5
// (persist probe output to backend after worker returns) paths in
// execute_dag. They require that:
//   - A cache hit populates the ProbeValueStore and skips the
//     worker (NodeCacheHit event, no NodeStarted).
//   - A cache miss dispatches to the worker, which runs the produce
//     source, and the result is persisted to the backend with
//     kind=probe_value.
//   - Changing a declared env var forces a different fingerprint and
//     a cache miss even when a prior entry exists.
// -----------------------------------------------------------------

fn probe_unit(key: &str, produce: &str) -> cook_contracts::ProbeUnit {
    cook_contracts::ProbeUnit {
        key: key.to_string(),
        produce_source: produce.to_string(),
        produce_line: 1,
        inputs: cook_contracts::ProbeInputs::default(),
    }
}

fn probe_unit_with_env(key: &str, produce: &str, env_var: &str) -> cook_contracts::ProbeUnit {
    cook_contracts::ProbeUnit {
        key: key.to_string(),
        produce_source: produce.to_string(),
        produce_line: 1,
        inputs: cook_contracts::ProbeInputs {
            env: vec![env_var.to_string()],
            tools: vec![],
            files: vec![],
            requires: vec![],
        },
    }
}

fn probe_work_node(key: &str, produce: &str, wd: PathBuf) -> WorkNode {
    WorkNode {
        process_env_vars: std::collections::BTreeMap::new(),
        payload: Some(WorkPayload::Probe {
            key: key.to_string(),
            produce: produce.to_string(),
            line: 1,
        }),
        recipe_name: format!("probe:{}", key),
        cache_meta: None,
        working_dir: wd,
        env_vars: BTreeMap::new(),
    }
}

fn probe_work_node_with_env(
    key: &str,
    produce: &str,
    env_var: &str,
    env_val: &str,
    wd: PathBuf,
) -> WorkNode {
    let mut env_vars = BTreeMap::new();
    env_vars.insert(env_var.to_string(), env_val.to_string());
    WorkNode {
        process_env_vars: std::collections::BTreeMap::new(),
        payload: Some(WorkPayload::Probe {
            key: key.to_string(),
            produce: produce.to_string(),
            line: 1,
        }),
        recipe_name: format!("probe:{}", key),
        cache_meta: None,
        working_dir: wd,
        env_vars,
    }
}

/// Compute the fingerprint for a ProbeUnit with no env/tool/file/upstream
/// inputs, suitable for pre-seeding the backend in cache-hit tests.
fn fingerprint_for(pu: &cook_contracts::ProbeUnit, wd: &std::path::Path) -> [u8; 32] {
    let inputs = cook_fingerprint::resolve_probe_inputs(
        pu,
        wd,
        &|_| None,
        &BTreeMap::new(),
    )
    .expect("fingerprint resolution should succeed for simple probe");
    cook_fingerprint::compute_probe_fingerprint(&inputs)
}

/// Pre-populate the cache backend with known bytes under the given
/// fingerprint key. Used to set up the "cache hit" scenario for G4 tests.
fn seed_probe_cache(backend: &dyn cook_cache::backend::CacheBackend, fp: &[u8; 32], bytes: &[u8]) {
    let mut meta = cook_fingerprint::ArtifactMeta {
        recipe_namespace: "probe:test".into(),
        command_hash: 0,
        env_contribution: 0,
        schema_version: cook_fingerprint::CACHE_VERSION,
        size_bytes: bytes.len() as u64,
        tags: std::collections::BTreeSet::new(),
        consulted_env_keys: std::collections::BTreeSet::new(),
        output_index: 0,
        output_path: "probe:test".into(),
        content_hash: cook_fingerprint::ArtifactMeta::zero_content_hash(),
        kind: None,
        seal_contribution: 0,
        mode: cook_fingerprint::ArtifactMeta::default_mode(),
        target: None,
    }
    .as_probe_value();
    cook_cache::backend::put_bytes(backend, fp, bytes, &mut meta)
        .expect("seed_probe_cache: backend put failed");
}

// G4 test: pre-populate the cache with canned bytes; the probe's produce
// source calls `error()` so execution would fail — but on a hit we MUST
// skip dispatch and deliver the cached bytes without ever invoking produce.
#[test]
fn probe_cache_hit_skips_produce_execution() {
    use std::sync::mpsc;

    let (_wd, _tmp) = tmp_dir();
    let wd = _wd.clone();
    let cache_ctx = make_cache_ctx(&_tmp);

    // Build the ProbeUnit and compute its fingerprint.
    let pu = probe_unit("test:hit", "error('should not run')");
    let fp = fingerprint_for(&pu, &wd);

    // Seed the backend with the known bytes we expect to see in the store.
    let expected_bytes =
        cook_contracts::probe_value::encode_canonical_json(&serde_json::json!([true]));
    seed_probe_cache(cache_ctx.backend.as_ref(), &fp, &expected_bytes);

    // Build a DAG with the probe node.
    let mut dag = Dag::new();
    let node_id = dag
        .add_node(probe_work_node("test:hit", "error('should not run')", wd), &[])
        .unwrap();

    // Build probe_units_by_node: maps node 0 → our ProbeUnit.
    let mut probe_units_by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = BTreeMap::new();
    probe_units_by_node.insert(node_id, pu);

    // Listen for events to verify NodeCacheHit (not NodeStarted).
    let (tx, rx) = mpsc::channel();
    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        Some(tx),
        cache_ctx.clone(),
        None,
        &[],
        &probe_units_by_node,
        std::sync::Arc::new(BTreeMap::new()),
    );
    assert!(result.is_ok(), "expected Ok, got: {result:?}");

    // Verify NodeCacheHit was emitted (not NodeStarted).
    let events: Vec<_> = rx.try_iter().collect();
    let cache_hits: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::NodeCacheHit { .. }))
        .collect();
    let node_started: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::NodeStarted { .. }))
        .collect();
    assert_eq!(cache_hits.len(), 1, "expected exactly one NodeCacheHit; events: {events:#?}");
    assert_eq!(node_started.len(), 0, "expected no NodeStarted on cache hit; events: {events:#?}");

    // Also verify the cached bytes are still retrievable from the backend
    // (the put_bytes call in the test harness must not corrupt the entry).
    let post = cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp)
        .expect("post-hit get");
    let stored = post.expect("cache entry must still exist after a hit read");
    assert_eq!(stored, expected_bytes, "cached bytes must survive a hit read");

    // CS-0102: the hit must also materialise the canonical local copy at
    // .cook/probes/<key>.json with exactly the cached bytes.
    let probe_file = _tmp.path().join(".cook").join("probes").join("test:hit.json");
    assert!(
        probe_file.exists(),
        "cache hit must write {}",
        probe_file.display()
    );
    assert_eq!(
        std::fs::read(&probe_file).unwrap(),
        expected_bytes,
        ".cook/probes file must hold the exact cached bytes"
    );
}

// G5 test: on a cache miss the worker executes the produce source, the
// output is persisted to the backend with kind=probe_value, and the result
// is available in the probe-value store.
#[test]
fn probe_cache_miss_persists_output() {
    let (_wd, _tmp) = tmp_dir();
    let wd = _wd.clone();
    let cache_ctx = make_cache_ctx(&_tmp);

    // Produce source: return the integer 42.
    let produce = "return 42";
    let pu = probe_unit("test:miss", produce);
    let fp = fingerprint_for(&pu, &wd);

    // Backend starts empty — cache miss guaranteed.
    let pre = cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp)
        .expect("pre-check get");
    assert!(pre.is_none(), "backend must be empty before the run");

    let mut dag = Dag::new();
    let node_id = dag
        .add_node(probe_work_node("test:miss", produce, wd), &[])
        .unwrap();

    let mut probe_units_by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = BTreeMap::new();
    probe_units_by_node.insert(node_id, pu);

    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        None,
        cache_ctx.clone(),
        None,
        &[],
        &probe_units_by_node,
        std::sync::Arc::new(BTreeMap::new()),
    );
    assert!(result.is_ok(), "expected Ok, got: {result:?}");

    // G5: verify the artifact was persisted to the backend.
    let post = cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp)
        .expect("post-run get");
    assert!(
        post.is_some(),
        "probe artifact must be persisted to cache backend after execution (G5)"
    );

    // CS-0102: the persisted value must be the canonical JSON rendering of 42.
    let bytes = post.unwrap();
    let expected = cook_contracts::probe_value::encode_canonical_json(&serde_json::json!(42));
    assert_eq!(
        bytes, expected,
        "persisted probe bytes must be the canonical JSON rendering"
    );

    // G5: verify the persisted bytes are retrievable from the backend.
    // (G3 — probe-value store — is internal to execute_dag and not
    // accessible after the function returns, but the G5 backend entry
    // serves as equivalent evidence that the produce path ran to completion.)
    let post2 = cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp)
        .expect("second get");
    let persisted = post2.expect("artifact must still be in backend on second read");
    assert_eq!(persisted, bytes, "persisted bytes must round-trip through backend");

    // CS-0102: the miss path must also materialise .cook/probes/<key>.json
    // with the same bytes (file == store == CAS).
    let probe_file = _tmp.path().join(".cook").join("probes").join("test:miss.json");
    assert!(
        probe_file.exists(),
        "cache miss must write {}",
        probe_file.display()
    );
    assert_eq!(
        std::fs::read(&probe_file).unwrap(),
        expected,
        ".cook/probes file must hold the exact persisted bytes"
    );
}

// CS-0102 stale-artifact defence: a cache entry whose bytes are not
// probe-value JSON (e.g. a pre-CS-0102 artifact) MUST be treated as a
// miss — produce runs, no NodeCacheHit is emitted, and the entry is
// overwritten with canonical JSON.
#[test]
fn probe_cache_hit_with_non_json_bytes_falls_through_to_miss() {
    use std::sync::mpsc;

    let (_wd, _tmp) = tmp_dir();
    let wd = _wd.clone();
    let cache_ctx = make_cache_ctx(&_tmp);

    let produce = "return 7";
    let pu = probe_unit("test:stale", produce);
    let fp = fingerprint_for(&pu, &wd);

    // Seed the backend with bytes that are NOT JSON (0x91 0xc3 is the old
    // encoding of [true]).
    seed_probe_cache(cache_ctx.backend.as_ref(), &fp, &[0x91, 0xc3]);

    let mut dag = Dag::new();
    let node_id = dag
        .add_node(probe_work_node("test:stale", produce, wd), &[])
        .unwrap();

    let mut probe_units_by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = BTreeMap::new();
    probe_units_by_node.insert(node_id, pu);

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        Some(tx),
        cache_ctx.clone(),
        None,
        &[],
        &probe_units_by_node,
        std::sync::Arc::new(BTreeMap::new()),
    );
    assert!(result.is_ok(), "expected Ok, got: {result:?}");

    // The stale entry must NOT register as a cache hit.
    let events: Vec<_> = rx.try_iter().collect();
    let cache_hits: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::NodeCacheHit { .. }))
        .collect();
    assert_eq!(
        cache_hits.len(),
        0,
        "non-JSON cached bytes must fall through to miss; events: {events:#?}"
    );

    // The backend entry must now hold the canonical JSON rendering of 7.
    let post = cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp)
        .expect("post-run get")
        .expect("entry must exist after the re-run");
    assert_eq!(
        post,
        cook_contracts::probe_value::encode_canonical_json(&serde_json::json!(7)),
        "stale entry must be overwritten with canonical JSON"
    );
}

// G4/G5 invalidation test: changing a declared env var changes the probe's
// fingerprint, causing a cache miss even when a prior entry exists under
// the old fingerprint.
#[test]
fn probe_fingerprint_changes_invalidate_cache() {
    let (_wd, _tmp) = tmp_dir();
    let wd = _wd.clone();
    let cache_ctx = make_cache_ctx(&_tmp);
    let produce = "return 'result'";
    let env_var = "PROBE_TEST_VAR";

    // Build ProbeUnit for env_val="first".
    let pu_v1 = probe_unit_with_env("test:inv", produce, env_var);
    let fp_v1 = {
        let inputs = cook_fingerprint::resolve_probe_inputs(
            &pu_v1,
            &wd,
            &|name| if name == env_var { Some("first".into()) } else { None },
            &BTreeMap::new(),
        )
        .unwrap();
        cook_fingerprint::compute_probe_fingerprint(&inputs)
    };

    // Build ProbeUnit for env_val="second".
    let pu_v2 = probe_unit_with_env("test:inv", produce, env_var);
    let fp_v2 = {
        let inputs = cook_fingerprint::resolve_probe_inputs(
            &pu_v2,
            &wd,
            &|name| if name == env_var { Some("second".into()) } else { None },
            &BTreeMap::new(),
        )
        .unwrap();
        cook_fingerprint::compute_probe_fingerprint(&inputs)
    };
    assert_ne!(fp_v1, fp_v2, "fingerprints must differ when env var changes");

    // CS-0172: an `envs { }` probe determinant is read from the AMBIENT PROCESS
    // environment, so the simulated "machine change" is a real `set_var` rather
    // than a value planted in the node's variable map.
    std::env::set_var(env_var, "first");

    // --- Run 1: env_val="first" → cache miss → populate backend under fp_v1 ---
    {
        let mut dag = Dag::new();
        let node_id = dag
            .add_node(
                probe_work_node_with_env("test:inv", produce, env_var, "first", wd.clone()),
                &[],
            )
            .unwrap();
        let mut by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = BTreeMap::new();
        by_node.insert(node_id, pu_v1);

        let result = execute_dag(
            dag,
            2,
            BTreeMap::new(),
            None,
            cache_ctx.clone(),
            None,
            &[],
            &by_node,
            std::sync::Arc::new(BTreeMap::new()),
        );
        assert!(result.is_ok(), "run1 expected Ok, got: {result:?}");
    }

    // Verify fp_v1 is now in the backend.
    assert!(
        cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp_v1)
            .unwrap()
            .is_some(),
        "fp_v1 must be in backend after run1"
    );
    // fp_v2 must NOT be in the backend yet.
    assert!(
        cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp_v2)
            .unwrap()
            .is_none(),
        "fp_v2 must not be in backend before run2"
    );

    std::env::set_var(env_var, "second");

    // --- Run 2: env_val="second" → different fingerprint → cache miss ---
    {
        use std::sync::mpsc;
        let mut dag = Dag::new();
        let node_id = dag
            .add_node(
                probe_work_node_with_env("test:inv", produce, env_var, "second", wd.clone()),
                &[],
            )
            .unwrap();
        let mut by_node: BTreeMap<usize, cook_contracts::ProbeUnit> = BTreeMap::new();
        by_node.insert(node_id, pu_v2);

        let (tx, rx) = mpsc::channel();
        let result = execute_dag(
            dag,
            2,
            BTreeMap::new(),
            Some(tx),
            cache_ctx.clone(),
            None,
            &[],
            &by_node,
            std::sync::Arc::new(BTreeMap::new()),
        );
        assert!(result.is_ok(), "run2 expected Ok, got: {result:?}");

        // run2 must NOT have emitted a NodeCacheHit — the env change must
        // force a miss even though fp_v1 is in the backend.
        let events: Vec<_> = rx.try_iter().collect();
        let cache_hits: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, EngineEvent::NodeCacheHit { .. }))
            .collect();
        assert_eq!(
            cache_hits.len(),
            0,
            "run2 must not cache-hit because env var changed; events: {events:#?}"
        );
    }

    // After run2, fp_v2 must now exist in the backend as well (G5 persisted it).
    assert!(
        cook_cache::backend::get_bytes(cache_ctx.backend.as_ref(), &fp_v2)
            .unwrap()
            .is_some(),
        "fp_v2 must be in backend after run2"
    );

    std::env::remove_var(env_var);
}

// ---------------------------------------------------------------------------
// normalize_glob_pattern tests — CS-0085 trailing-** normalisation
// ---------------------------------------------------------------------------

#[test]
fn normalize_glob_pattern_appends_star_after_trailing_star_star() {
    assert_eq!(cook_fingerprint::normalize_glob_pattern("build/**").as_ref(), "build/**/*");
    assert_eq!(cook_fingerprint::normalize_glob_pattern(".next/**").as_ref(), ".next/**/*");
    assert_eq!(cook_fingerprint::normalize_glob_pattern("apps/web/.next/**").as_ref(), "apps/web/.next/**/*");
}

#[test]
fn normalize_glob_pattern_handles_bare_double_star() {
    assert_eq!(cook_fingerprint::normalize_glob_pattern("**").as_ref(), "**/*");
}

#[test]
fn normalize_glob_pattern_passes_through_non_trailing_double_star() {
    assert_eq!(cook_fingerprint::normalize_glob_pattern("**/lib/*.so").as_ref(), "**/lib/*.so");
    assert_eq!(cook_fingerprint::normalize_glob_pattern("src/**/*.c").as_ref(), "src/**/*.c");
}

#[test]
fn normalize_glob_pattern_passes_through_non_glob_patterns() {
    assert_eq!(cook_fingerprint::normalize_glob_pattern("*.c").as_ref(), "*.c");
    assert_eq!(cook_fingerprint::normalize_glob_pattern("file?.txt").as_ref(), "file?.txt");
    assert_eq!(cook_fingerprint::normalize_glob_pattern("build/main.o").as_ref(), "build/main.o");
}

#[test]
fn resolve_output_paths_handles_trailing_double_star() {
    let tmp = tempfile::tempdir().expect("tempdir");
        let wd = tmp.path();
        std::fs::create_dir_all(wd.join("build/sub")).unwrap();
    std::fs::write(wd.join("build/a.o"), b"a").unwrap();
    std::fs::write(wd.join("build/sub/b.o"), b"b").unwrap();

    let resolved = super::resolve_output_paths(
        &["build/**".to_string()],
        wd,
    );
    let mut paths = resolved.clone();
    paths.sort();
    assert_eq!(paths, vec!["build/a.o".to_string(), "build/sub/b.o".to_string()],
        "trailing-** normalization should match files at any depth");
}

#[test]
fn resolve_output_paths_reports_raw_empty_glob_after_shared_normalization() {
    let tmp = tempfile::tempdir().expect("tempdir");
        let resolved =
            super::resolve_output_paths_with_unmatched(&["build/**".to_string()], tmp.path());
    assert!(resolved.paths.is_empty());
    assert_eq!(resolved.unmatched_patterns, vec!["build/**"]);
}

#[test]
fn resolve_output_paths_does_not_report_literal_or_empty_directory_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("empty-dir")).unwrap();
    let resolved = super::resolve_output_paths_with_unmatched(
        &["literal.txt".to_string(), "empty-dir/".to_string()],
        tmp.path(),
    );
    assert_eq!(resolved.paths, vec!["literal.txt"]);
    assert!(resolved.unmatched_patterns.is_empty());
}

#[test]
fn resolve_output_paths_deduplicates_overlap() {
    let tmp = tempfile::tempdir().expect("tempdir");
        let wd = tmp.path();
        std::fs::create_dir_all(wd.join("build")).unwrap();
    std::fs::write(wd.join("build/a.o"), b"a").unwrap();
    std::fs::write(wd.join("build/b.o"), b"b").unwrap();

    let resolved = super::resolve_output_paths(
        &["build/**".to_string(), "build/a.o".to_string()],
        wd,
    );
    let mut paths = resolved.clone();
    paths.sort();
    assert_eq!(paths.len(), 2, "overlapping literal+glob should dedupe");
    assert_eq!(paths, vec!["build/a.o".to_string(), "build/b.o".to_string()]);
}

#[test]
fn resolve_output_paths_empty_glob_match_is_not_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
        let wd = tmp.path();
        let resolved = super::resolve_output_paths(
            &["build/**".to_string()],
        wd,
    );
    assert!(resolved.is_empty(),
        "glob matching nothing returns empty Vec; §17.6 item 3 says this MUST NOT be an error");
    }

    #[test]
    fn resolve_output_paths_deduplicates_duplicate_literals() {
        let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    std::fs::write(wd.join("main.o"), b"obj").unwrap();
    let resolved = super::resolve_output_paths(
        &["main.o".to_string(), "main.o".to_string()],
        wd,
    );
    assert_eq!(resolved, vec!["main.o".to_string()],
        "duplicate literal entries must dedupe to a single entry per §17.6 item 1");
}

#[test]
fn resolve_output_paths_expands_directory_output() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("pkg/sub")).unwrap();
    std::fs::write(root.join("pkg/a.js"), b"a").unwrap();
    std::fs::write(root.join("pkg/sub/b.wasm"), b"b").unwrap();

    let resolved = super::resolve_output_paths(&["pkg/".to_string()], root);
    let set: std::collections::BTreeSet<&str> = resolved.iter().map(|s| s.as_str()).collect();
    assert!(set.contains("pkg/a.js"));
    assert!(set.contains("pkg/sub/b.wasm"));
    assert_eq!(set.len(), 2); // files only (CS-0064), directory entries dropped
}
