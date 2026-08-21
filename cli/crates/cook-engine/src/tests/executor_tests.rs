use super::*;
use cook_contracts::probe_key::LocalProbeKey;
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
    use cook_cache::FileRecord;
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
    seal_keys.insert(LocalProbeKey::new("host"));
    let store = cook_probe::store::ProbeValueStore::new();
    store.insert(&LocalProbeKey::new("host"), b"\"x86_64-linux\"".to_vec());

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

/// CS-0191: a test node is an ordinary node carrying a reporting name. These
/// tests used to build a `WorkPayload::Test` with `timeout: 30` — a value the
/// register surface could not produce, since CS-0135 removed the modifier and
/// every real construction hardcoded `u64::MAX`. Building test fixtures by hand
/// is exactly how a payload keeps fields nothing can set.
fn test_node(cmd: &str, name: &str, recipe: &str, wd: PathBuf) -> WorkNode {
    test_node_at(cmd, name, 1, None, recipe, wd)
}

/// As [`test_node`], with the source line and the fan-out member spelled out.
/// `member` is what the payload used to carry as `iteration_item`.
fn test_node_at(
    cmd: &str,
    name: &str,
    line: usize,
    member: Option<&str>,
    recipe: &str,
    wd: PathBuf,
) -> WorkNode {
    let mut node = work_node(
        WorkPayload::Shell {
            cmd: cmd.to_string(),
            line,
        },
        recipe,
        wd,
    );
    node.test_name = Some(name.to_string());
    node.member = member.map(|m| m.to_string());
    node
}

fn work_node(payload: WorkPayload, recipe: &str, wd: PathBuf) -> WorkNode {
    WorkNode {
        process_env_vars: std::collections::BTreeMap::new(),
        payload: Some(payload),
        recipe_name: recipe.to_string(),
        cache_meta: None,
        working_dir: wd,
        env_vars: default_env(),
        test_name: None,
        member: None,
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
        test_name: None,
        member: None,
    }
}

/// Build a minimal CacheContext backed by a temp-dir LocalBackend.
/// Suitable for executor tests that don't exercise the cache path.
fn make_cache_ctx(tmp: &TempDir) -> Arc<CacheContext> {
    use cook_cache::EnvDenylist;
    use cook_cache::{backend::LocalBackend, cache_ctx::CacheContext, cloud_config::CloudConfig};
    Arc::new(CacheContext {
        denylist: Arc::new(EnvDenylist::baseline()),
        backend: Arc::new(LocalBackend::new(tmp.path().join("cloud"))),
        cloud_config: Arc::new(CloudConfig::default()),
        project_root: tmp.path().to_path_buf(),
        project_id: "test".to_string(),
        publish_enabled: true,
        replay_logs: false,
    })
}

// 1. Single node succeeds
#[test]
fn test_executor_runs_single_node() {
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);
    let mut dag = Dag::new();
    dag.add_node(work_node(shell("true"), "single", wd), &[]).unwrap();

    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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
        )
        .unwrap();
    dag.add_node(work_node(shell("cat output.txt"), "reader", wd), &[a])
        .unwrap();

    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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

    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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
    let result = execute_dag(
        dag,
        4,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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
    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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

    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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

    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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

#[test]
fn command_failure_event_summarizes_without_transport_payload() {
    use std::sync::mpsc;

    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);
    let command = "printf raw-output; exit 7";
    let mut dag = Dag::new();
    dag.add_node(
        work_node(
            WorkPayload::Shell {
                cmd: command.into(),
                line: 23,
            },
            "failure",
            wd,
        ),
        &[],
    )
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let _ = execute_dag(
        dag,
        1,
        BTreeMap::new(),
        Some(tx),
        cache_ctx,
        &[],
        &BTreeMap::new(),
        Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
    let error = rx
        .try_iter()
        .find_map(|event| match event {
            EngineEvent::NodeFailed { error, .. } => Some(error),
            _ => None,
        })
        .expect("command failure should emit NodeFailed");

    assert!(!error.contains("COOK_CMD_FAILED:"), "{error}");
    assert!(!error.contains("\"exit_code\":"), "{error}");
    assert!(error.contains("line 23"), "{error}");
    assert!(error.contains("exit 7"), "{error}");
    assert!(error.contains(command), "{error}");
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

    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[test]
fn interactive_command_failure_uses_shared_json_contract() {
    let (wd, _tmp) = tmp_dir();
    let command = "printf 'key:value\\n\"quoted\"'\nexit 7";
    let wire = run_interactive_on_main(
        command,
        23,
        &wd,
        &BTreeMap::new(),
        &cook_probe::store::ProbeValueStore::new(),
        "",
    )
    .expect_err("interactive command should fail");
    let failure =
        cook_contracts::CommandFailure::from_wire(&wire).expect("canonical command failure JSON");

    assert_eq!(failure.line(), 23);
    assert_eq!(failure.exit_code(), 7);
    assert_eq!(failure.command(), command);
    assert_eq!(failure.stdout().as_str(), "");
    assert_eq!(failure.stderr().as_str(), "");
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
        work_node(shell("echo to-stdout; echo to-stderr 1>&2"), "mixed", wd),
        &[],
    )
    .unwrap();

    let (tx, rx) = mpsc::channel::<EngineEvent>();
    let result = execute_dag(
        dag,
        1,
        BTreeMap::new(),
        Some(tx),
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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
        inputs: vec![],
        consumes: Vec::new(),
        member_keyed: false,
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
        test_name: None,
        member: None,
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
        test_name: None,
        member: None,
    }
}

/// A `cache_managers` map carrying one fresh, empty manager for `recipe`,
/// backed by a temp cache dir. Required so `check_node_cache` does not
/// short-circuit to Miss on a missing manager.
fn empty_cache_managers(
    recipe: &str,
    dir: &std::path::Path,
) -> BTreeMap<String, Arc<ThreadSafeCacheManager>> {
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

    let result = execute_dag(
        dag,
        1,
        managers,
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
    assert!(
        result.is_ok(),
        "local cold-miss should rebuild, got: {result:?}"
    );
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

    let result = execute_dag(
        dag,
        1,
        managers,
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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

    let result = execute_dag(
        dag,
        1,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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

    let result = execute_dag(
        dag,
        1,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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

    let result = execute_dag(
        dag,
        1,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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
    let a = dag
        .add_node(
            work_node(
                WorkPayload::Interactive {
                    cmd: "true".into(),
                    line: 1,
                    is_chore: true,
                },
                "chore",
                wd.clone(),
            ),
            &[],
        )
        .unwrap();
    let b = dag
        .add_node(
            work_node(
                WorkPayload::Interactive {
                    cmd: "true".into(),
                    line: 2,
                    is_chore: true,
                },
                "chore",
                wd.clone(),
            ),
            &[a],
        )
        .unwrap();
    dag.add_node(
        work_node(
            WorkPayload::Interactive {
                cmd: "true".into(),
                line: 3,
                is_chore: true,
            },
            "chore",
            wd.clone(),
        ),
        &[b],
    )
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        Some(tx),
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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
    let a = dag
        .add_node(
            work_node(
                WorkPayload::Interactive {
                    cmd: "true".into(),
                    line: 1,
                    is_chore: true,
                },
                "chore",
                wd.clone(),
            ),
            &[],
        )
        .unwrap();
    let b = dag
        .add_node(
            work_node(
                WorkPayload::Interactive {
                    cmd: "false".into(),
                    line: 2,
                    is_chore: true,
                },
                "chore",
                wd.clone(),
            ),
            &[a],
        )
        .unwrap();
    dag.add_node(
        work_node(
            WorkPayload::Interactive {
                cmd: "true".into(),
                line: 3,
                is_chore: true,
            },
            "chore",
            wd,
        ),
        &[b],
    )
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let _result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        Some(tx),
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );

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
            "step",
            wd,
        ),
        &[],
    )
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let _result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        Some(tx),
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );

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
    let a = dag
        .add_node(
            work_node(
                WorkPayload::Interactive {
                    cmd: "true".into(),
                    line: 1,
                    is_chore: true,
                },
                "shell1",
                wd.clone(),
            ),
            &[],
        )
        .unwrap();
    let b = dag
        .add_node(
            work_node(
                WorkPayload::LuaChunk {
                    code: "-- noop".into(),
                    inputs: vec![],
                    outputs: vec![],
                    gather_groups: vec![],
                    step_kind: cook_contracts::StepKind::Chore,
                    is_chore: true,
                    line: 0,
                },
                "shell1",
                wd.clone(),
            ),
            &[a],
        )
        .unwrap();
    dag.add_node(
        work_node(
            WorkPayload::Interactive {
                cmd: "true".into(),
                line: 3,
                is_chore: true,
            },
            "shell1",
            wd,
        ),
        &[b],
    )
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        Some(tx),
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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
    let a = dag
        .add_node(
            work_node(
                WorkPayload::LuaChunk {
                    code: "-- noop".into(),
                    inputs: vec![],
                    outputs: vec![],
                    gather_groups: vec![],
                    step_kind: cook_contracts::StepKind::Chore,
                    is_chore: true,
                    line: 0,
                },
                "lua_chore",
                wd.clone(),
            ),
            &[],
        )
        .unwrap();
    dag.add_node(
        work_node(
            WorkPayload::LuaChunk {
                code: "-- noop".into(),
                inputs: vec![],
                outputs: vec![],
                gather_groups: vec![],
                step_kind: cook_contracts::StepKind::Chore,
                is_chore: true,
                line: 0,
            },
            "lua_chore",
            wd,
        ),
        &[a],
    )
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        Some(tx),
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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
                gather_groups: vec![],
                step_kind: cook_contracts::StepKind::Cook,
                is_chore: false,
                line: 0,
            },
            "regular_lua",
            wd,
        ),
        &[],
    )
    .unwrap();

    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        None,
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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
        test_node("true", "my_test", "blocked_by_build", wd.clone()),
        &[cook],
    ).unwrap();

    let result = execute_dag(
        dag, 2, BTreeMap::new(), None, cache_ctx, &[], &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
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

// SHI-line, amended CS-0191: the unit's source line must propagate into
// TestStarted and TestResult.line rather than remaining 0. The line now comes
// from the payload every unit has rather than from a test-only variant.
#[test]
fn test_line_number_propagates_from_payload_to_events() {
    use std::sync::mpsc;
    let (wd, _tmp) = tmp_dir();
    let cache_ctx = make_cache_ctx(&_tmp);

    let mut dag = Dag::new();
    dag.add_node(
        test_node_at("true", "my_test", 17, None, "my_recipe", wd),
        &[],
    ).unwrap();

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        Some(tx),
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
    let test_results = result.expect("test node should pass");

    // TestResult.line must carry 17.
    assert_eq!(test_results.len(), 1, "expected exactly one TestResult");
    assert_eq!(
        test_results[0].line, 17,
        "TestResult.line should be 17 (from the payload's line)"
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
        // CS-0191: the fan-out member, which the payload used to carry a
        // second copy of as `iteration_item`.
        test_node_at("true", "my_test", 17, Some("a.cpp"), "my_recipe", wd),
        &[],
    ).unwrap();

    let (tx, rx) = mpsc::channel();
    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        Some(tx),
        cache_ctx,
        &[],
        &BTreeMap::new(),
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
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
// CS-0074/CS-0243 G4/G5: a reached probe always observes.
//
// Before CS-0243 this block exercised the G4 (cache lookup before dispatch)
// and G5 (persist probe output to backend after worker returns) paths in
// execute_dag. Both are gone: there is no probe-value cache, so a probe
// dispatch either resolves without a VM (a synthesised producer kind, or
// COOK-526's cross-phase single-flight — see cook-probe's own tests) or runs
// `produce` on the worker, full stop.
// -----------------------------------------------------------------

fn probe_unit(key: &str, produce: &str) -> cook_contracts::ProbeUnit {
    cook_contracts::ProbeUnit {
        key: LocalProbeKey::new(key),
        produce_source: produce.to_string(),
        produce_line: 1,
        inputs: cook_contracts::ProbeInputs::default(),
    }
}

fn probe_work_node(key: &str, produce: &str, wd: PathBuf) -> WorkNode {
    WorkNode {
        process_env_vars: std::collections::BTreeMap::new(),
        payload: Some(WorkPayload::Probe {
            key: LocalProbeKey::new(key),
            produce: produce.to_string(),
            line: 1,
        }),
        recipe_name: format!("probe:{}", key),
        cache_meta: None,
        working_dir: wd,
        env_vars: BTreeMap::new(),
        test_name: None,
        member: None,
    }
}

// ---------------------------------------------------------------------------
// normalize_glob_pattern tests — CS-0085 trailing-** normalisation
// ---------------------------------------------------------------------------

#[test]
fn normalize_glob_pattern_appends_star_after_trailing_star_star() {
    assert_eq!(cook_cache::normalize_glob_pattern("build/**").as_ref(), "build/**/*");
    assert_eq!(cook_cache::normalize_glob_pattern(".next/**").as_ref(), ".next/**/*");
    assert_eq!(cook_cache::normalize_glob_pattern("apps/web/.next/**").as_ref(), "apps/web/.next/**/*");
}

#[test]
fn normalize_glob_pattern_handles_bare_double_star() {
    assert_eq!(cook_cache::normalize_glob_pattern("**").as_ref(), "**/*");
}

#[test]
fn normalize_glob_pattern_passes_through_non_trailing_double_star() {
    assert_eq!(cook_cache::normalize_glob_pattern("**/lib/*.so").as_ref(), "**/lib/*.so");
    assert_eq!(cook_cache::normalize_glob_pattern("src/**/*.c").as_ref(), "src/**/*.c");
}

#[test]
fn normalize_glob_pattern_passes_through_non_glob_patterns() {
    assert_eq!(cook_cache::normalize_glob_pattern("*.c").as_ref(), "*.c");
    assert_eq!(cook_cache::normalize_glob_pattern("file?.txt").as_ref(), "file?.txt");
    assert_eq!(cook_cache::normalize_glob_pattern("build/main.o").as_ref(), "build/main.o");
}

#[test]
fn resolve_output_paths_handles_trailing_double_star() {
    let tmp = tempfile::tempdir().expect("tempdir");
        let wd = tmp.path();
        std::fs::create_dir_all(wd.join("build/sub")).unwrap();
    std::fs::write(wd.join("build/a.o"), b"a").unwrap();
    std::fs::write(wd.join("build/sub/b.o"), b"b").unwrap();

    let resolved = super::resolve_output_paths(&["build/**".to_string()], wd);
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

    let resolved =
        super::resolve_output_paths(&["build/**".to_string(), "build/a.o".to_string()], wd);
    let mut paths = resolved.clone();
    paths.sort();
    assert_eq!(paths.len(), 2, "overlapping literal+glob should dedupe");
    assert_eq!(paths, vec!["build/a.o".to_string(), "build/b.o".to_string()]);
}

#[test]
fn resolve_output_paths_empty_glob_match_is_not_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    let resolved = super::resolve_output_paths(&["build/**".to_string()], wd);
    assert!(
        resolved.is_empty(),
        "glob matching nothing returns empty Vec; §17.6 item 3 says this MUST NOT be an error"
    );
}

    #[test]
    fn resolve_output_paths_deduplicates_duplicate_literals() {
        let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    std::fs::write(wd.join("main.o"), b"obj").unwrap();
    let resolved = super::resolve_output_paths(&["main.o".to_string(), "main.o".to_string()], wd);
    assert_eq!(
        resolved,
        vec!["main.o".to_string()],
        "duplicate literal entries must dedupe to a single entry per §17.6 item 1"
    );
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

// ---------------------------------------------------------------------------
// CS-0243 — a reached probe always observes; there is no probe-value cache
// ---------------------------------------------------------------------------

/// CS-0243/CS-0244: a probe dispatch consults no store. The `produce` body
/// raises, so any route that could have answered this node without running it
/// would make the run succeed; it must fail.
///
/// Before CS-0244 this test additionally planted a value in the cache backend
/// at the exact fingerprint the probe would have addressed, and asserted it
/// came back untouched. There is no fingerprint left to address anything by,
/// so the seeding half is gone with the key it needed.
#[test]
fn a_probe_dispatch_always_runs_produce() {
    use std::sync::mpsc;

    let (_wd, _tmp) = tmp_dir();
    let wd = _wd.clone();
    let cache_ctx = make_cache_ctx(&_tmp);

    let pu = probe_unit("test:always", "error('produce ran')");

    let mut dag = Dag::new();
    let node_id = dag
        .add_node(
            probe_work_node("test:always", "error('produce ran')", wd),
            &[],
        )
        .unwrap();
    let mut probe_units_by_node: BTreeMap<usize, (cook_contracts::ProbeUnit, std::path::PathBuf, bool)> =
        BTreeMap::new();
    probe_units_by_node.insert(node_id, (pu, _wd.clone(), false));

    let (tx, _rx) = mpsc::channel();
    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        Some(tx),
        cache_ctx.clone(),
        &[],
        &probe_units_by_node,
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );

    assert!(
        result.is_err(),
        "a reached probe MUST always run produce; something answered the node \
         without running it, so the raising produce body never ran"
    );
}

/// COOK-527 review (finding G): deleting `probe_cache_miss_persists_output`
/// alongside the cache it pinned also dropped its non-cache assertion — that
/// the EXECUTOR's produce path, not just `cook_probe::eval` in isolation,
/// writes `.cook/probes/<key>.json` with the exact canonical bytes.
/// `record`'s own doc calls that write load-bearing and unconditional, and
/// COOK-526's cross-phase single-flight depends on it, so it needs coverage
/// here rather than only at eval level and indirectly through surface
/// fixtures.
#[test]
fn executor_produce_path_materialises_the_canonical_probes_file() {
    let (_wd, _tmp) = tmp_dir();
    let wd = _wd.clone();
    let cache_ctx = make_cache_ctx(&_tmp);

    let produce = "return 42";
    let pu = probe_unit("test:materialise", produce);

    let mut dag = Dag::new();
    let node_id = dag
        .add_node(probe_work_node("test:materialise", produce, wd), &[])
        .unwrap();
    let mut probe_units_by_node: BTreeMap<usize, (cook_contracts::ProbeUnit, std::path::PathBuf, bool)> =
        BTreeMap::new();
    probe_units_by_node.insert(node_id, (pu, _wd.clone(), false));

    let result = execute_dag(
        dag,
        2,
        BTreeMap::new(),
        None,
        cache_ctx.clone(),
        &[],
        &probe_units_by_node,
        std::sync::Arc::new(BTreeMap::new()),
        &std::sync::atomic::AtomicU64::new(0),
    );
    assert!(result.is_ok(), "expected Ok, got: {result:?}");

    let probe_file = _tmp.path().join(".cook").join("probes").join("test:materialise.json");
    assert!(
        probe_file.exists(),
        "the executor's produce path must write {}",
        probe_file.display()
    );
    let expected = cook_contracts::probe_value::encode_canonical_json(&serde_json::json!(42));
    assert_eq!(
        std::fs::read(&probe_file).unwrap(),
        expected,
        ".cook/probes file must hold the exact canonical bytes the executor's produce path wrote"
    );
}

// ---------------------------------------------------------------------------
// CS-0189: every cacheable unit publishes an observation
// ---------------------------------------------------------------------------

fn publish_ctx(wd: &std::path::Path) -> cook_cache::cache_ctx::CacheContext {
    use cook_cache::{backend::LocalBackend, cloud_config::CloudConfig};
    cook_cache::cache_ctx::CacheContext {
        denylist: std::sync::Arc::new(cook_cache::EnvDenylist::baseline()),
        backend: std::sync::Arc::new(LocalBackend::new(wd.join("cloud"))),
        cloud_config: std::sync::Arc::new(CloudConfig::default()),
        project_root: wd.to_path_buf(),
        project_id: "p".to_string(),
        publish_enabled: true,
        replay_logs: false,
    }
}

fn meta_for(cache_key: &str, inputs: &[&str], outputs: &[&str]) -> cook_contracts::CacheMeta {
    cook_contracts::CacheMeta {
        recipe_name: "r".into(),
        project_id: "p".into(),
        cookfile_path: "Cookfile".into(),
        cache_key: cache_key.into(),
        inputs: inputs.iter().map(|s| (*s).into()).collect(),
        consumes: Vec::new(),
        member_keyed: false,
        output_paths: outputs.iter().map(|s| s.to_string()).collect(),
        command_hash: 0xbeef,
        env_contribution: 0,
        consulted_env: Default::default(),
        discovered_inputs: None,
        seal_keys: Default::default(),
        sharing: Default::default(),
        record: false,
    }
}

/// An observing unit has no file outputs, but its verdict, duration, cause,
/// and output stream are still a shareable observation.
#[test]
fn an_observing_unit_publishes_its_observation_to_the_shared_store() {
    let dir = TempDir::new().unwrap();
    let wd = dir.path();
    std::fs::write(wd.join("in.txt"), "x").unwrap();

    let cm = std::sync::Arc::new(cook_cache::ThreadSafeCacheManager::new(wd.join("idx")));
    let ctx = publish_ctx(wd);
    let published = std::sync::atomic::AtomicU64::new(0);
    let meta = meta_for(":0123456789abcdef", &["in.txt"], &[]);

    publish_completion(
        &cm,
        &meta,
        wd,
        Duration::from_millis(12),
        &[],
        &cook_probe::store::ProbeValueStore::new(),
        &ctx,
        &published,
        &[],
    );

    assert_eq!(
        published.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "an observing unit's observation counts as a publish"
    );
    assert!(
        std::fs::read_dir(wd.join("cloud")).unwrap().next().is_some(),
        "the observation must reach the shared store"
    );
    // It DOES record locally — that is the whole point of the fold.
    let idx = cm.get_or_load("r");
    assert!(
        idx.steps.contains_key(":0123456789abcdef"),
        "the local record is what serves the verdict"
    );
}

/// The control: producing units continue to publish their file artifacts too.
#[test]
fn a_producing_unit_still_publishes() {
    let dir = TempDir::new().unwrap();
    let wd = dir.path();
    std::fs::write(wd.join("in.txt"), "x").unwrap();
    std::fs::write(wd.join("out.txt"), "y").unwrap();

    let cm = std::sync::Arc::new(cook_cache::ThreadSafeCacheManager::new(wd.join("idx")));
    let ctx = publish_ctx(wd);
    let published = std::sync::atomic::AtomicU64::new(0);
    let meta = meta_for("out.txt", &["in.txt"], &["out.txt"]);

    publish_completion(
        &cm,
        &meta,
        wd,
        Duration::from_millis(12),
        &[],
        &cook_probe::store::ProbeValueStore::new(),
        &ctx,
        &published,
        &[],
    );

    assert_eq!(published.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(
        std::fs::read_dir(wd.join("cloud")).unwrap().next().is_some(),
        "a producing unit's artifact and manifest reach the store"
    );
}
