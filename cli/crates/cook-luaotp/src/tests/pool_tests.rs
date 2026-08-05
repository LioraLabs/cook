use super::*;
use std::fs;
use tempfile::TempDir;

fn make_pool(n: usize) -> (WorkerPool, mpsc::Receiver<WorkResult>, TempDir) {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(n);
    (pool, rx, dir)
}

#[test]
fn dep_output_resolves_root_recipe() {
    let lua = unsafe { mlua::Lua::unsafe_new() };
    let cook = lua.create_table().unwrap();
    let recipe = Arc::new(Mutex::new("app".to_string()));
    let mut map = BTreeMap::new();
    map.insert("lib".to_string(), vec!["build/lib.txt".to_string()]);
    install_worker_dep_output_api(&lua, &cook, Arc::new(map), &recipe).unwrap();
    lua.globals().set("cook", cook).unwrap();
    let s: String = lua.load(r#"return cook.dep_output("lib")"#).eval().unwrap();
    assert_eq!(s, "build/lib.txt");
    let t: Vec<String> = lua.load(r#"return cook.dep_output_list("lib")"#).eval().unwrap();
    assert_eq!(t, vec!["build/lib.txt".to_string()]);
}

#[test]
fn dep_output_resolves_same_cookfile_prefix() {
    let lua = unsafe { mlua::Lua::unsafe_new() };
    let cook = lua.create_table().unwrap();
    let recipe = Arc::new(Mutex::new("sub.app".to_string()));
    let mut map = BTreeMap::new();
    map.insert("sub.lib".to_string(), vec!["sub/build/lib.a".to_string()]);
    install_worker_dep_output_api(&lua, &cook, Arc::new(map), &recipe).unwrap();
    lua.globals().set("cook", cook).unwrap();
    let s: String = lua.load(r#"return cook.dep_output("lib")"#).eval().unwrap();
    assert_eq!(s, "sub/build/lib.a");
}

#[test]
fn dep_output_nested_bare_ref_does_not_fall_back_to_root() {
    // A nested consumer's bare ref resolves against its OWN Cookfile only.
    // With no local `sub.lib` but a same-named root `lib`, this must be an
    // unknown referent (Lua error), NOT a silent fall-through to root —
    // matching the register-phase resolve_global_key semantics.
    let lua = unsafe { mlua::Lua::unsafe_new() };
    let cook = lua.create_table().unwrap();
    let recipe = Arc::new(Mutex::new("sub.app".to_string()));
    let mut map = BTreeMap::new();
    map.insert("lib".to_string(), vec!["build/root_lib.a".to_string()]);
    install_worker_dep_output_api(&lua, &cook, Arc::new(map), &recipe).unwrap();
    lua.globals().set("cook", cook).unwrap();
    let r = lua.load(r#"return cook.dep_output("lib")"#).eval::<String>();
    assert!(r.is_err(), "nested bare ref must not fall back to root recipe");
}

#[test]
fn dep_output_unknown_recipe_errors() {
    let lua = unsafe { mlua::Lua::unsafe_new() };
    let cook = lua.create_table().unwrap();
    let recipe = Arc::new(Mutex::new("app".to_string()));
    install_worker_dep_output_api(&lua, &cook, Arc::new(BTreeMap::new()), &recipe).unwrap();
    lua.globals().set("cook", cook).unwrap();
    let r = lua.load(r#"return cook.dep_output("nope")"#).eval::<String>();
    assert!(r.is_err(), "unknown recipe must raise a Lua error");
    }

    #[test]
    fn dep_output_empty_list_is_empty_string() {
        let lua = unsafe { mlua::Lua::unsafe_new() };
        let cook = lua.create_table().unwrap();
        let recipe = Arc::new(Mutex::new("app".to_string()));
    let mut map = BTreeMap::new();
    map.insert("lib".to_string(), Vec::<String>::new());
    install_worker_dep_output_api(&lua, &cook, Arc::new(map), &recipe).unwrap();
    lua.globals().set("cook", cook).unwrap();
    let s: String = lua.load(r#"return cook.dep_output("lib")"#).eval().unwrap();
    assert_eq!(s, "");
    let t: Vec<String> = lua.load(r#"return cook.dep_output_list("lib")"#).eval().unwrap();
    assert!(t.is_empty());
}

#[test]
fn test_pool_executes_shell_command() {
    let (pool, rx, dir) = make_pool(1);

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::Shell {
            cmd: "echo hello".to_string(),
            line: 1,
        },
        recipe_name: "test_recipe".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    assert!(result.success, "expected success, got error: {:?}", result.error);
    assert_eq!(result.id, 0);
    assert!(result.error.is_none());

    pool.shutdown();
}

#[test]
fn test_pool_multiple_workers() {
    let (pool, rx, dir) = make_pool(4);

    for i in 0..8 {
        pool.submit(WorkItem {
            process_env_vars: HashMap::new(),
            id: i,
            payload: WorkPayload::Shell {
                cmd: "true".to_string(),
                line: 1,
            },
            recipe_name: format!("recipe_{i}"),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
            project_root: dir.path().to_path_buf(),
        });
    }

    let mut results = Vec::new();
    for _ in 0..8 {
        results.push(rx.recv().unwrap());
    }

    assert_eq!(results.len(), 8);
    for r in &results {
        assert!(r.success, "work item {} failed: {:?}", r.id, r.error);
    }

    // Verify all IDs are present (order may vary)
    let mut ids: Vec<usize> = results.iter().map(|r| r.id).collect();
    ids.sort();
    assert_eq!(ids, (0..8).collect::<Vec<_>>());

    pool.shutdown();
}

#[test]
fn test_pool_reports_shell_failure() {
    let (pool, rx, dir) = make_pool(1);

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 42,
        payload: WorkPayload::Shell {
            cmd: "false".to_string(),
            line: 7,
        },
        recipe_name: "fail_recipe".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    assert!(!result.success);
    assert_eq!(result.id, 42);
    let err = result.error.as_ref().expect("expected error message");
    let failure =
        cook_contracts::CommandFailure::from_wire(err).expect("canonical command failure JSON");
    assert_eq!(failure.line(), 7);
    assert_eq!(failure.exit_code(), 1);
    assert_eq!(failure.command(), "false");

    pool.shutdown();
}

#[test]
fn worker_command_failure_uses_shared_json_contract() {
    let (pool, rx, dir) = make_pool(1);
    let command = "printf 'out:key\\n'; printf 'err \"quoted\"\\n' >&2\nexit 7";

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 43,
        payload: WorkPayload::Shell {
            cmd: command.to_string(),
            line: 23,
        },
        recipe_name: "json_failure".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    let wire = result.error.expect("expected command failure");
    let failure =
        cook_contracts::CommandFailure::from_wire(&wire).expect("canonical command failure JSON");
    assert_eq!(failure.line(), 23);
    assert_eq!(failure.exit_code(), 7);
    assert_eq!(failure.command(), command);
    assert_eq!(failure.stdout().as_str(), "out:key\n");
    assert_eq!(failure.stderr().as_str(), "err \"quoted\"\n");
    let expected = cook_contracts::CommandFailure::new(
        23,
        7,
        command,
        cook_contracts::CapturedStream::from_bytes(b"out:key\n"),
        cook_contracts::CapturedStream::from_bytes(b"err \"quoted\"\n"),
    )
    .to_wire();
    assert_eq!(wire, expected);

    pool.shutdown();
}

#[test]
fn test_pool_working_dir() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("file.txt"), "contents").unwrap();

    let (pool, rx) = WorkerPool::spawn(1);

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::Shell {
            cmd: "cat file.txt".to_string(),
            line: 1,
        },
        recipe_name: "dir_test".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    assert!(result.success, "expected success, got error: {:?}", result.error);

    pool.shutdown();
}

#[test]
fn test_pool_executes_lua_chunk_writing_multiple_outputs() {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);

    let code = r#"
            local f = io.open(outputs[1], "w")
            f:write("a")
            f:close()
            local g = io.open(outputs[2], "w")
            g:write("b")
            g:close()
        "#;

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::LuaChunk {
            code: code.to_string(),
            inputs: vec!["src.rs".to_string()],
            outputs: vec![
                dir.path().join("a.txt").to_string_lossy().into_owned(),
                dir.path().join("b.txt").to_string_lossy().into_owned(),
            ],
            ingredient_groups: vec![vec!["src.rs".to_string()]],
            step_kind: cook_contracts::StepKind::Cook,
            is_chore: false,
            line: 0,
        },
        recipe_name: "multi".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    assert!(
        result.success,
        "expected success, got error: {:?}",
        result.error
    );
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "a");
    assert_eq!(fs::read_to_string(dir.path().join("b.txt")).unwrap(), "b");

    pool.shutdown();
}

// COOK-191: execute-phase Lua errors must be sanitized at the source —
// no raw mlua traceback and no "lua error: " / "runtime error: "
// wrapper noise in `WorkResult.error`, unless the user opted in via
// `COOK_BACKTRACE=1` (mirrored by `-v` in cook-cli/src/main.rs).
//
// COOK_BACKTRACE is a process-global env var and cargo test runs run in
// parallel threads, so both the "clean by default" and "opt-in keeps
// traceback" assertions live in a single test function: the var is set
// and removed within this one test, and the default-off assertion runs
// first (before the var is ever touched) so it can never observe a
// sibling test's opt-in state.
#[test]
fn test_pool_lua_chunk_error_is_sanitized_by_default_and_keeps_traceback_with_cook_backtrace() {
    let result = run_lua_chunk_in_worker(r#"error("kaboom")"#);
    assert!(!result.success, "expected error() to fail the chunk");
    let err = result.error.as_deref().unwrap_or("");
    let expected = format!(
        "[rec] {}",
        cook_contracts::lua_error::sanitize(
            "lua error: runtime error: Cookfile:1: kaboom\nstack traceback:\n\t[C]: in ?",
            false
        )
    );
    assert!(err.contains("kaboom"), "error must retain the message; got: {err}");
    assert_eq!(err, expected, "worker and shared Lua sanitation must agree");
    assert!(
        !err.contains("stack traceback"),
        "error must not contain a raw Lua traceback by default; got: {err}"
    );
    assert!(
        !err.contains("lua error: runtime error:"),
        "error must not contain the raw mlua wrapper prefixes; got: {err}"
    );

    // SAFETY (test-only): COOK_BACKTRACE is read when constructing the error
    // at error-construction time inside the worker thread spawned by
    // `run_lua_chunk_in_worker`, which we join on before removing the
    // var below, so there is no cross-thread race within this test.
    std::env::set_var("COOK_BACKTRACE", "1");
    let result = run_lua_chunk_in_worker(r#"error("kaboom")"#);
    std::env::remove_var("COOK_BACKTRACE");

    assert!(!result.success, "expected error() to fail the chunk");
    let err = result.error.as_deref().unwrap_or("");
    assert!(err.contains("kaboom"), "error must retain the message; got: {err}");
    assert!(
        err.contains("stack traceback"),
        "COOK_BACKTRACE=1 must preserve the traceback; got: {err}"
    );
}

#[test]
fn test_pool_lua_chunk_sees_input_output_globals() {
    let dir = TempDir::new().unwrap();
    let out_path = dir.path().join("out.txt");
    let (pool, rx) = WorkerPool::spawn(1);

    // Use singular `input`/`output` convention (single input/output case).
    let code = r#"
            local f = io.open(output, "w")
            f:write(input)
            f:close()
        "#;

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::LuaChunk {
            code: code.to_string(),
            inputs: vec!["hello".to_string()],
            outputs: vec![out_path.to_string_lossy().into_owned()],
            ingredient_groups: vec![],
            step_kind: cook_contracts::StepKind::Cook,
            is_chore: false,
            line: 0,
        },
        recipe_name: "r".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
            project_root: dir.path().to_path_buf(),
        });

        let result = rx.recv().unwrap();
        assert!(
            result.success,
            "expected success, got error: {:?}",
        result.error
    );
    assert_eq!(fs::read_to_string(&out_path).unwrap(), "hello");

    pool.shutdown();
}

#[test]
fn test_pool_env_vars() {
    let dir = TempDir::new().unwrap();
    let mut env = HashMap::new();
    env.insert("MY_VAR".to_string(), "hello_from_pool".to_string());

    let (pool, rx) = WorkerPool::spawn(1);

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::Shell {
            cmd: "echo $MY_VAR".to_string(),
            line: 1,
        },
        recipe_name: "env_test".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: env,
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    assert!(result.success, "expected success, got error: {:?}", result.error);

    pool.shutdown();
}

/// CS-0017 multi-Cookfile imports route work items with different
/// `working_dir`s through the same worker. `fs.*` must resolve relative
/// paths against each item's cwd, not the cwd of the first item the
/// worker happened to see.
#[test]
fn test_pool_fs_api_uses_per_item_working_dir() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();
    fs::write(dir1.path().join("data.txt"), "from-dir1").unwrap();
    fs::write(dir2.path().join("data.txt"), "from-dir2").unwrap();

    let out1 = dir1.path().join("out.txt");
    let out2 = dir2.path().join("out.txt");

    // Single worker so both items hit the same VM and exercise refresh.
    let (pool, rx) = WorkerPool::spawn(1);

    let code = r#"
            local content = fs.read("data.txt")
            local f = io.open(output, "w")
            f:write(content)
            f:close()
        "#;

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::LuaChunk {
            code: code.to_string(),
            inputs: vec![],
            outputs: vec![out1.to_string_lossy().into_owned()],
            ingredient_groups: vec![],
            step_kind: cook_contracts::StepKind::Cook,
            is_chore: false,
            line: 0,
        },
        recipe_name: "r".to_string(),
            working_dir: dir1.path().to_path_buf(),
            env_vars: HashMap::new(),
            project_root: dir1.path().to_path_buf(),
        });
        let r1 = rx.recv().unwrap();
        assert!(r1.success, "first item failed: {:?}", r1.error);
    assert_eq!(fs::read_to_string(&out1).unwrap(), "from-dir1");

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 1,
        payload: WorkPayload::LuaChunk {
            code: code.to_string(),
            inputs: vec![],
            outputs: vec![out2.to_string_lossy().into_owned()],
            ingredient_groups: vec![],
            step_kind: cook_contracts::StepKind::Cook,
            is_chore: false,
            line: 0,
        },
        recipe_name: "r".to_string(),
            working_dir: dir2.path().to_path_buf(),
            env_vars: HashMap::new(),
            project_root: dir2.path().to_path_buf(),
        });
        let r2 = rx.recv().unwrap();
        assert!(r2.success, "second item failed: {:?}", r2.error);
    assert_eq!(
        fs::read_to_string(&out2).unwrap(),
        "from-dir2",
        "fs.read must resolve against item 2's working_dir, \
         not the first item's"
    );

    pool.shutdown();
}

/// A Rust panic inside work-item processing must not hang the engine.
/// The worker should surface a failure `WorkResult` and keep processing
/// subsequent items. The magic recipe name `"__cook_test_panic__"` is
/// recognized by `execute_work_item` under `#[cfg(test)]` and panics
/// before the work payload runs — this exercises the panic boundary
/// directly (mlua catches panics raised from inside Lua callbacks, so
/// a Lua-side trigger wouldn't reach `catch_unwind`).
#[test]
fn test_pool_recovers_from_worker_panic() {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);

    // First item: triggers a panic in execute_work_item.
    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 7,
        payload: WorkPayload::Shell {
            cmd: "true".to_string(),
            line: 1,
        },
        recipe_name: "__cook_test_panic__".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });
    let r1 = rx.recv().unwrap();
    assert_eq!(r1.id, 7);
    assert!(!r1.success, "expected failure result, got success");
    let err = r1.error.as_ref().expect("expected error message");
    assert!(
        err.to_lowercase().contains("panic"),
        "error should mention the panic: {err}"
    );

    // Second item: same worker pool should still process this.
    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 8,
        payload: WorkPayload::Shell {
            cmd: "echo recovered".to_string(),
            line: 1,
        },
        recipe_name: "recovery".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });
    let r2 = rx.recv().unwrap();
    assert_eq!(r2.id, 8);
    assert!(r2.success, "post-panic item failed: {:?}", r2.error);

    pool.shutdown();
}

/// Dropping `WorkerPool` without an explicit `shutdown()` must signal
/// the workers and join them. Otherwise the queue `Arc` outlives the
/// pool — the workers leak, blocked on the condvar forever.
#[test]
fn test_pool_drop_cleans_up_workers() {
    let weak;
    {
        let (pool, _rx) = WorkerPool::spawn(2);
        weak = Arc::downgrade(&pool.queue);
    } // pool dropped here without shutdown()

    // After Drop, all worker threads should have exited and released
    // their `Arc<SharedQueue>` clones, so the only remaining strong
    // ref (the pool's) is also gone.
    assert!(
        weak.upgrade().is_none(),
        "queue Arc still alive after pool drop — workers were not joined"
    );
}

/// CS-0191: a run's exit code is a property of the run, so it rides on
/// `WorkResult` like its duration and its output. This replaces
/// `test_output_carries_exit_code`, which asserted the same thing about a
/// `TestOutput` that existed only for the two test-shaped executors — and did
/// it by hand-building the struct rather than running anything.
#[test]
fn a_failing_command_reports_its_exit_code_on_the_result() {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);
    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::Shell { cmd: "exit 7".to_string(), line: 1 },
        recipe_name: "r".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });
    let result = rx.recv().expect("result");
    drop(pool);
    assert!(!result.success);
    assert_eq!(result.exit_code, Some(7));
}

// -----------------------------------------------------------------
// Standard §6.3.2 regression tests: register-only Cook Lua API
// helpers must raise a §6.3.2-shaped diagnostic when called from
// execute-phase Lua (lua_line / lua_block / cook-body >{ … } payload).
// -----------------------------------------------------------------

/// Submit a single LuaChunk work item that runs `code` on a worker VM,
/// then return the resulting `WorkResult` for inspection.
fn run_lua_chunk_in_worker(code: &str) -> WorkResult {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);
    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::LuaChunk {
            code: code.to_string(),
            inputs: vec![],
            outputs: vec![],
            ingredient_groups: vec![],
            step_kind: cook_contracts::StepKind::Cook,
            is_chore: false,
            line: 0,
        },
        recipe_name: "rec".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });
    let result = rx.recv().unwrap();
    pool.shutdown();
    result
}

fn assert_register_only_diagnostic(result: &WorkResult, fn_name: &str) {
    assert!(
        !result.success,
        "expected register-only-API call to fail; got success"
    );
    let err = result.error.as_deref().unwrap_or("");
    let needle_fn = format!("cook.{fn_name}");
    assert!(
        err.contains(&needle_fn),
        "diagnostic must name the function `{needle_fn}`; got: {err}"
    );
    assert!(
        err.contains("execute-phase Lua"),
        "diagnostic must identify the calling step kind as execute-phase Lua; got: {err}"
    );
}

#[test]
fn cook_exec_from_execute_phase_raises_section_6_3_2_diagnostic() {
    let result = run_lua_chunk_in_worker(r#"cook.exec("echo hi", 0)"#);
    assert_register_only_diagnostic(&result, "exec");
}

#[test]
fn cook_interactive_from_execute_phase_raises_section_6_3_2_diagnostic() {
    let result = run_lua_chunk_in_worker(r#"cook.interactive("echo hi", 0)"#);
    assert_register_only_diagnostic(&result, "interactive");
}

#[test]
fn cook_add_unit_from_execute_phase_raises_section_6_3_2_diagnostic() {
    let result =
        run_lua_chunk_in_worker(r#"cook.add_unit({command = "echo hi"})"#);
    assert_register_only_diagnostic(&result, "add_unit");
}

#[test]
fn cook_step_group_from_execute_phase_raises_section_6_3_2_diagnostic() {
    let result = run_lua_chunk_in_worker(r#"cook.step_group("g")"#);
    assert_register_only_diagnostic(&result, "step_group");
}

#[test]
fn cook_recipe_from_execute_phase_raises_section_6_3_2_diagnostic() {
    let result =
        run_lua_chunk_in_worker(r#"cook.recipe("inner", {}, function() end)"#);
        assert_register_only_diagnostic(&result, "recipe");
}

/// §22.5.2: cook.probe MUST raise a register-only-API diagnostic on the
/// execute-phase VM (CS-0074).
#[test]
fn cook_probe_from_execute_phase_raises_register_only_diagnostic() {
    let result =
        run_lua_chunk_in_worker(r#"cook.probe("cc:x", { inputs = {}, produce = "return 1" })"#);
    assert!(
        !result.success,
        "cook.probe on execute VM must fail; got success"
    );
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains("cook.probe: register-only API"),
        "diagnostic must start with 'cook.probe: register-only API'; got: {err}"
    );
    assert!(err.contains("execute-phase Lua"), "got: {err}");
}

/// SHI-216 / CS-0072: every register-only guard message MUST include
/// a `>>` migration hint so users know how to move the call to register
/// phase.  We spot-check `cook.add_unit` (representative of all five).
#[test]
fn register_only_guard_includes_double_arrow_migration_hint() {
    let result =
        run_lua_chunk_in_worker(r#"cook.add_unit({command = "echo hi"})"#);
    assert!(
        !result.success,
        "expected register-only-API call to fail; got success"
    );
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains(">>"),
        "diagnostic must include `>>` migration hint; got: {err}"
    );
    assert!(
        err.contains("register"),
            "diagnostic must mention `register` block; got: {err}"
    );
}

/// `cook.sh` is the both-phase shell-out helper (§6.3.1) and MUST
/// continue to work on the worker VM. Guard against accidentally
/// classifying it as register-only.
#[test]
fn cook_sh_from_execute_phase_still_works() {
    let result = run_lua_chunk_in_worker(r#"cook.sh("true")"#);
    assert!(
        result.success,
        "cook.sh must remain callable in execute phase; got error: {:?}",
        result.error
    );
}

// -----------------------------------------------------------------
// CS-0069 regressions: execute-phase `cook.load_module` MUST honour
// §7's four-path resolution order, not just the top-level two paths.
// -----------------------------------------------------------------

/// Submit a single LuaChunk work item that runs `code` on a worker VM
/// rooted at `cwd`, then return the resulting `WorkResult` for
/// inspection. Unlike `run_lua_chunk_in_worker`, this lets the caller
/// pre-populate `cwd` with module-resolution fixtures.
fn run_lua_chunk_in_worker_at(cwd: &std::path::Path, code: &str) -> WorkResult {
    let (pool, rx) = WorkerPool::spawn(1);
    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::LuaChunk {
            code: code.to_string(),
            inputs: vec![],
            outputs: vec![],
            ingredient_groups: vec![],
            step_kind: cook_contracts::StepKind::Cook,
            is_chore: false,
            line: 0,
        },
        recipe_name: "rec".to_string(),
        working_dir: cwd.to_path_buf(),
        env_vars: HashMap::new(),
        project_root: cwd.to_path_buf(),
    });
    let result = rx.recv().unwrap();
    pool.shutdown();
    result
}

/// CS-0069: a module installed under `cook_modules/share/lua/5.4/<name>/init.lua`
/// (the canonical LuaRocks share-tree layout for multi-file rocks) MUST
/// be resolvable by execute-phase `cook.load_module`, not just by the
/// register phase. Pre-CS-0069 this raised "module not found" because
/// pool.rs only searched the two top-level paths.
#[test]
fn cook_load_module_resolves_share_lua_5_4_init() {
    let dir = TempDir::new().unwrap();
    let share_pkg = dir.path()
        .join("cook_modules/share/lua/5.4/share_only_pkg");
    fs::create_dir_all(&share_pkg).expect("mkdir share path");
    fs::write(
        share_pkg.join("init.lua"),
        "return { from_share = true }",
    ).expect("write init.lua");

    let code = r#"
            local m = cook.load_module("share_only_pkg")
            assert(m.from_share == true, "expected from_share=true, got "..tostring(m.from_share))
        "#;
    let result = run_lua_chunk_in_worker_at(dir.path(), code);
    assert!(
        result.success,
        "cook.load_module must resolve share/lua/5.4/<name>/init.lua; got error: {:?}",
        result.error
    );
}

/// CS-0069: a flat module file at `cook_modules/share/lua/5.4/<name>.lua`
/// (single-file rocks) MUST also be resolvable from the execute phase.
#[test]
fn cook_load_module_resolves_share_lua_5_4_flat() {
    let dir = TempDir::new().unwrap();
    let share_dir = dir.path().join("cook_modules/share/lua/5.4");
    fs::create_dir_all(&share_dir).expect("mkdir share path");
    fs::write(
        share_dir.join("share_flat_pkg.lua"),
        "return { kind = 'flat' }",
    ).expect("write flat module");

    let code = r#"
            local m = cook.load_module("share_flat_pkg")
            assert(m.kind == "flat", "expected kind=flat, got "..tostring(m.kind))
        "#;
    let result = run_lua_chunk_in_worker_at(dir.path(), code);
    assert!(
        result.success,
        "cook.load_module must resolve share/lua/5.4/<name>.lua; got error: {:?}",
        result.error
    );
}

/// CS-0069: when a module exists at both the top-level and share-tree
/// paths, the top-level (hand-vendored) copy MUST win. Mirrors the
/// priority test in cook-register/src/module_loader.rs.
#[test]
fn cook_load_module_top_level_wins_over_share_lua() {
    let dir = TempDir::new().unwrap();
    let modules_dir = dir.path().join("cook_modules");
    let share_dir = modules_dir.join("share/lua/5.4");
    fs::create_dir_all(&share_dir).expect("mkdir share path");
    // Top-level (should win): flat <name>.lua under cook_modules/.
    fs::write(
        modules_dir.join("dup_pkg.lua"),
        "return { from = 'top-level' }",
    ).expect("write top-level module");
    // Share-tree (should lose): init.lua under share/lua/5.4/<name>/.
    let share_pkg = share_dir.join("dup_pkg");
    fs::create_dir_all(&share_pkg).expect("mkdir share pkg");
    fs::write(
        share_pkg.join("init.lua"),
        "return { from = 'share' }",
    ).expect("write share module");

    let code = r#"
            local m = cook.load_module("dup_pkg")
            assert(m.from == "top-level", "expected from=top-level, got "..tostring(m.from))
        "#;
    let result = run_lua_chunk_in_worker_at(dir.path(), code);
    assert!(
        result.success,
        "cook.load_module must prefer top-level over share-tree; got error: {:?}",
        result.error
    );
}

// -----------------------------------------------------------------
// CS-0070 / CS-0074 / CS-0152 regressions: execute-phase VM
// cook.probes surface.
//
// cook.probes.get reads from the SharedProbeValueStore (populated by
// upstream probe units); a never-materialised key is a hard error
// (CS-0152), while a stored `null` value still decodes to Lua nil
// with no error. cook.probes.set is deprecated and raises on the
// execute-phase VM (CS-0074).
// -----------------------------------------------------------------

/// CS-0152: `cook.probes.get(key)` on a key that was never
/// materialised in the probe-value store MUST raise a runtime error
/// naming the key and pointing at how to demand it (§22.5.8), rather
/// than silently returning nil.
#[test]
fn cook_probes_get_errors_for_missing_key() {
    let code = r#"
            local v = cook.probes.get("never_set")
        "#;
    let result = run_lua_chunk_in_worker(code);
    assert!(
        !result.success,
        "cook.probes.get for a never-materialised key must raise, not return nil"
    );
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains("never_set"),
        "error must name the missing key; got: {err}"
    );
    assert!(
        err.contains("not materialised"),
        "error must say the value was not materialised; got: {err}"
    );
}

/// CS-0152: `cook.probes.scope(label).get(key)` on a miss MUST raise,
/// naming the FULL scoped key (`"<label>:<key>"`), matching the
/// unscoped diagnostic.
#[test]
fn cook_probes_scope_get_errors_for_missing_key() {
    let code = r#"
            local scoped = cook.probes.scope("cc")
            local v = scoped.get("missing")
        "#;
    let result = run_lua_chunk_in_worker(code);
    assert!(
        !result.success,
        "scoped cook.probes.get for a never-materialised key must raise, not return nil"
    );
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains("cc:missing"),
        "error must name the full scoped key 'cc:missing'; got: {err}"
    );
    assert!(
        err.contains("not materialised"),
        "error must say the value was not materialised; got: {err}"
    );
}

/// §24.4.3 / CS-0203: a scope label containing ':' MUST raise on the
/// execute-phase VM too — the same shared diagnostic the register VM
/// raises. Until COOK-412 no phase enforced the rule.
#[test]
fn cook_probes_scope_refuses_a_colon_label() {
    let code = r#"cook.probes.scope("a:b")"#;
    let result = run_lua_chunk_in_worker(code);
    assert!(!result.success, "colon label must raise");
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains("must not contain ':'"),
        "error must name the label rule; got: {err}"
    );
}

/// CS-0152: a key that IS present in the probe-value store whose
/// canonical JSON payload is `null` MUST still decode to Lua `nil`
/// with NO error — value-was-null is not the same thing as
/// never-materialised. cook_cc finder produce bodies rely on this
/// (`cook.probes.get(KEY) or { ... }`).
#[test]
fn cook_probes_get_returns_nil_for_stored_null_without_error() {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);

    {
        let bytes = cook_contracts::probe_value::encode_canonical_json(&serde_json::Value::Null);
        pool.probe_value_store().insert("cc:absent_tool", bytes);
    }

    let code = r#"
            local v = cook.probes.get("cc:absent_tool")
            assert(v == nil, "expected nil for stored null, got "..tostring(v))
        "#;

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::LuaChunk {
            code: code.to_string(),
            inputs: vec![],
            outputs: vec![],
            ingredient_groups: vec![],
            step_kind: cook_contracts::StepKind::Cook,
            is_chore: false,
            line: 0,
        },
        recipe_name: "rec".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    pool.shutdown();
    assert!(
        result.success,
        "cook.probes.get on a stored-null key must return nil without error; got error: {:?}",
        result.error
    );
}

/// CS-0074/CS-0102: `cook.probes.get(key)` MUST return the JSON-decoded value
/// that was written into the probe-value store by the scheduler (§22.5.7).
#[test]
fn cook_probes_get_reads_from_probe_value_store() {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);

    // Pre-populate the store as the scheduler would after a probe completes.
    {
        let bytes = cook_contracts::probe_value::encode_canonical_json(
            &serde_json::json!({"found": true, "version": "1.2.3"}),
        );
        pool.probe_value_store().insert("cc:zlib", bytes);
    }

    let code = r#"
            local r = cook.probes.get("cc:zlib")
            assert(r ~= nil, "expected non-nil result from probe store")
            assert(r.found == true, "expected found=true, got "..tostring(r.found))
            assert(r.version == "1.2.3", "expected version=1.2.3, got "..tostring(r.version))
        "#;

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::LuaChunk {
            code: code.to_string(),
            inputs: vec![],
            outputs: vec![],
            ingredient_groups: vec![],
            step_kind: cook_contracts::StepKind::Cook,
            is_chore: false,
            line: 0,
        },
        recipe_name: "rec".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    pool.shutdown();
    assert!(
        result.success,
        "cook.probes.get must read from probe-value store; got error: {:?}",
        result.error
    );
}

/// CS-0074: `cook.probes.set` is deprecated on the execute-phase VM and
/// MUST raise a runtime error directing the author to use `cook.probe`.
#[test]
fn cook_probes_set_on_execute_vm_raises_deprecation_error() {
    let result = run_lua_chunk_in_worker(r#"cook.probes.set("x", 1)"#);
    assert!(!result.success, "expected cook.probes.set to fail on execute VM");
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains("cook.probes.set"),
        "error must name cook.probes.set; got: {err}"
    );
    assert!(
        err.contains("deprecated"),
        "error must say 'deprecated'; got: {err}"
    );
}

/// COOK-208 / CS-0136: `cook.cache.*` is a hard error on the execute-phase
/// VM with a did-you-mean pointing at `cook.probes`.
#[test]
fn cook_cache_is_hard_error_with_did_you_mean() {
    let result = run_lua_chunk_in_worker(r#"return cook.cache.get("x")"#);
    assert!(!result.success, "expected cook.cache.get to be a hard error");
        let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains("cook.cache' was renamed to 'cook.probes'")
            && err.contains("cook.probes.get"),
        "rename diagnostic must name the new spelling; got: {err}"
    );
}

/// CS-0074/CS-0102: `cook.probes.scope(label).get(key)` MUST read from the
/// probe-value store using the scoped key `"<label>:<key>"`.
#[test]
fn cook_probes_scope_get_reads_from_probe_value_store() {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);

    // Pre-populate with scoped key format `"<label>:<key>"`.
    {
        let bytes = cook_contracts::probe_value::encode_canonical_json(
            &serde_json::json!("gcc-14"),
        );
        pool.probe_value_store().insert("cc:compiler", bytes);
        }

        let code = r#"
        local scoped = cook.probes.scope("cc")
        local v = scoped.get("compiler")
            assert(v == "gcc-14", "expected gcc-14, got "..tostring(v))
    "#;

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::LuaChunk {
            code: code.to_string(),
            inputs: vec![],
            outputs: vec![],
            ingredient_groups: vec![],
            step_kind: cook_contracts::StepKind::Cook,
            is_chore: false,
            line: 0,
        },
        recipe_name: "rec".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    pool.shutdown();
    assert!(
        result.success,
        "scoped cook.probes.get must read from probe-value store; got error: {:?}",
        result.error
    );
}

/// CS-0074: `cook.probes.scope(label).set` MUST raise on execute-phase VM.
#[test]
fn cook_probes_scope_set_on_execute_vm_raises_deprecation_error() {
    let result = run_lua_chunk_in_worker(r#"
            local s = cook.probes.scope("foo")
            s.set("x", 1)
        "#);
    assert!(!result.success, "expected scoped cook.probes.set to fail on execute VM");
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains("deprecated"),
        "scoped set error must say 'deprecated'; got: {err}"
    );
}

// -----------------------------------------------------------------
// CS-0200: the execute-phase VM REFUSES `cook.export` / `cook.import`.
//
// This block previously held three CS-0071 regressions asserting that the
// worker exposed the same name-keyed surface as the register VM, backed by
// an in-memory per-worker store, and that "cross-worker visibility is not
// required". The Standard licensed that (§24.5 permitted a per-worker
// scratch store) while §12.3.4 simultaneously required a register-time
// export to be observable here. The permission and the requirement were
// mutually exclusive; the implementation met the permission, so an
// execute-phase import returned nil for every register-time export.
// CS-0200 withdrew the surface. The surviving register-phase round trip is
// pinned in standard/conformance/positive/export-import-register-phase-register-ok.
// -----------------------------------------------------------------

/// CS-0200: `cook.export` and `cook.import` are register-phase only; the
/// worker VM refuses both by name.
///
/// This replaces three tests that pinned the withdrawn behaviour, one of which
/// deserves recording. `cook_export_store_isolated_per_worker` asserted that a
/// second worker MUST NOT see the first worker's export, and read as a
/// safety property ("no cross-worker leakage"). It was in fact pinning the
/// defect: units of one recipe are dispatched from a shared queue to whichever
/// worker is free, so that isolation is exactly what made an execute-phase
/// export visible or invisible by scheduling. A test can hold a bug in place
/// by describing it as a guarantee.
#[test]
fn cook_export_is_refused_on_the_worker_vm() {
    let out = run_lua_chunk_in_worker(r#"cook.export("scratch", { value = 1 })"#);
    assert!(!out.success, "execute-phase cook.export must raise");
    let err = out.error.unwrap_or_default();
    assert!(
        err.contains("cook.export: register-phase only") && err.contains("CS-0200"),
        "diagnostic must name the function, the phase and the change: {err}"
    );
}

#[test]
fn cook_import_is_refused_on_the_worker_vm() {
    let out = run_lua_chunk_in_worker(r#"local _ = cook.import("scratch")"#);
    assert!(!out.success, "execute-phase cook.import must raise");
    let err = out.error.unwrap_or_default();
    assert!(
        err.contains("cook.import: register-phase only") && err.contains("CS-0200"),
        "diagnostic must name the function, the phase and the change: {err}"
    );
}

/// The refusal must not be a silent nil, which is what an unknown name used to
/// return and what made the whole surface look like it worked.
#[test]
fn an_unknown_name_also_raises_rather_than_returning_nil() {
    let out = run_lua_chunk_in_worker(
        r#"
            local v = cook.import("never-exported-anywhere")
            assert(false, "unreachable: import returned "..tostring(v))
        "#,
    );
    assert!(!out.success);
    let err = out.error.unwrap_or_default();
    assert!(
        err.contains("register-phase only"),
        "must fail at the import, not at the assert: {err}"
    );
}

/// CS-0069: the diagnostic when no candidate path matches MUST list
/// all four attempted paths.
#[test]
fn cook_load_module_miss_diagnostic_lists_all_four_paths() {
    let dir = TempDir::new().unwrap();
    let code = r#"cook.load_module("nonexistent_pkg")"#;
    let result = run_lua_chunk_in_worker_at(dir.path(), code);
    assert!(!result.success, "expected miss to fail");
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains("nonexistent_pkg.lua"),
        "diagnostic must mention top-level flat path; got: {err}"
    );
    assert!(
        err.contains("nonexistent_pkg/init.lua"),
        "diagnostic must mention top-level init path; got: {err}"
    );
    assert!(
        err.contains("share/lua/5.4/nonexistent_pkg.lua"),
        "diagnostic must mention share-tree flat path; got: {err}"
    );
    assert!(
        err.contains("share/lua/5.4/nonexistent_pkg/init.lua"),
        "diagnostic must mention share-tree init path; got: {err}"
    );
}

// -----------------------------------------------------------------
// G1: WorkPayload::Probe dispatch tests (CS-0074)
// -----------------------------------------------------------------

/// G1 (CS-0102): dispatching a `WorkPayload::Probe` MUST produce a
/// successful `WorkResult` with `probe_output: Some(ProbeOutput { key, bytes })`
/// where `bytes` is the canonical-JSON rendering of the value returned by
/// `produce` (§22.5.5, §22.5.6).
#[test]
fn probe_unit_produces_canonical_json_bytes() {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::Probe {
            key: "test:simple".into(),
            produce: r#"return { found = true, paths = {"a", "b"} }"#.into(),
            line: 1,
        },
        recipe_name: "probe_recipe".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    pool.shutdown();

    assert!(result.success, "probe dispatch must succeed; got error: {:?}", result.error);
    assert!(result.probe_output.is_some(), "probe_output must be Some");

    let probe_output = result.probe_output.unwrap();
    assert_eq!(probe_output.key, "test:simple");
    assert!(!probe_output.bytes.is_empty(), "probe bytes must be non-empty");

    let decoded = cook_contracts::probe_value::decode_json(&probe_output.bytes)
        .expect("must decode");
    assert_eq!(
        decoded,
        serde_json::json!({"found": true, "paths": ["a", "b"]}),
    );
    // The worker MUST emit the ONE canonical rendering verbatim (CS-0102):
    // re-encoding the decoded value must reproduce the exact bytes.
    assert_eq!(
        probe_output.bytes,
        cook_contracts::probe_value::encode_canonical_json(&decoded),
        "ProbeOutput.bytes must be the canonical JSON rendering"
    );
}

/// G1 (CS-0123): the worker VM MUST expose cook.json_decode /
/// cook.yaml_decode (§24.8) so a demand-driven probe produce body can
/// decode structured output — parity with the register pre-pass VM.
#[test]
fn probe_produce_can_call_codecs_on_worker_vm() {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::Probe {
            key: "test:codecs".into(),
            produce: r#"
                    local j = cook.json_decode('{"name":"foo","items":[1,2]}')
                    local y = cook.yaml_decode("word: hello\n")
                    return { name = j.name, second = j.items[2], word = y.word }
                "#.into(),
            line: 1,
        },
        recipe_name: "probe_recipe".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    pool.shutdown();

    assert!(result.success, "codec probe must succeed; got: {:?}", result.error);
    let decoded = cook_contracts::probe_value::decode_json(&result.probe_output.unwrap().bytes)
        .expect("must decode");
    assert_eq!(decoded, serde_json::json!({"name": "foo", "second": 2, "word": "hello"}));
}

/// G1: a probe whose `produce` source raises a Lua error MUST fail the
/// WorkResult with a diagnostic naming the probe key (§22.5.6).
#[test]
fn probe_unit_lua_error_fails_with_key_in_diagnostic() {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::Probe {
            key: "test:error".into(),
                produce: r#"error("intentional probe failure")"#.into(),
            line: 1,
        },
        recipe_name: "probe_recipe".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    pool.shutdown();

    assert!(!result.success, "probe with Lua error must fail");
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains("test:error"),
            "error must name the probe key; got: {err}"
    );
}

/// G1: a probe that returns a non-serialisable value (function) MUST fail
/// with a diagnostic naming the probe key (§22.5.4).
#[test]
fn probe_unit_non_serialisable_value_fails() {
    let dir = TempDir::new().unwrap();
    let (pool, rx) = WorkerPool::spawn(1);

    pool.submit(WorkItem {
        process_env_vars: HashMap::new(),
        id: 0,
        payload: WorkPayload::Probe {
            key: "test:bad_type".into(),
            produce: r#"return function() end"#.into(),
            line: 1,
        },
        recipe_name: "probe_recipe".to_string(),
        working_dir: dir.path().to_path_buf(),
        env_vars: HashMap::new(),
        project_root: dir.path().to_path_buf(),
    });

    let result = rx.recv().unwrap();
    pool.shutdown();

    assert!(!result.success, "probe returning non-serialisable value must fail");
    let err = result.error.as_deref().unwrap_or("");
    assert!(
        err.contains("test:bad_type"),
        "error must name the probe key; got: {err}"
    );
}

// ── COOK-361: agreement guard against re-forking the substitution paths ──
//
// Every probe-substitution position shares `cook_contracts::sigil::subst`
// (worker commands CS-0192, chore drain spawns CS-0193, register output
// patterns CS-0195), but each entry point wraps its own store lookup and
// read-view construction. This pins the worker wrapper to the law directly:
// the same ident over the same canonical value must render identically
// through `resolve_probe_sigils` and through `substitute`. If a second
// renderer is ever deliberately introduced, COOK-361's standing rule
// requires a new agreement test beside this one.

#[test]
fn worker_substitution_agrees_with_the_law() {
    use cook_contracts::probe_value::encode_canonical_json;
    use cook_contracts::sigil::{probe_ref, subst::substitute};

    let value = serde_json::json!({
        "name": "zlib",
        "cflags": ["-O2", "-Wall"],
        "version": 3.0,
    });
    let store = crate::store::ProbeValueStore::new();
    store.insert("cc:zlib", encode_canonical_json(&value));

    for ident in ["cc:zlib.name", "cc:zlib.cflags[2]", "cc:zlib.version"] {
        let via_worker =
            crate::pool::resolve_probe_sigils(&store, &format!("echo $<{ident}>"))
                .expect("worker path renders");
        let r = probe_ref(ident).expect("probe-shaped");
        let via_law = substitute(&value, r.path(), ident).expect("law renders");
        assert_eq!(via_worker, format!("echo {via_law}"), "ident {ident}");
    }

    // The read view too: a tool-path annotation merged by the store side
    // must render exactly what the law renders over the merged value.
    let tools = serde_json::json!({"gcc": {"hash": "ab12"}});
    store.insert("cc:tc", encode_canonical_json(&tools));
    store.set_tool_paths(
        "cc:tc",
        std::collections::BTreeMap::from([("gcc".to_string(), "/usr/bin/gcc".to_string())]),
    );
    let via_worker = crate::pool::resolve_probe_sigils(&store, "$<cc:tc.gcc.path>")
        .expect("read view renders");
    let mut merged = tools.clone();
    cook_contracts::probe_value::merge_tool_paths(
        &mut merged,
        &std::collections::BTreeMap::from([("gcc".to_string(), "/usr/bin/gcc".to_string())]),
    );
    let r = probe_ref("cc:tc.gcc.path").expect("probe-shaped");
    let via_law = substitute(&merged, r.path(), "cc:tc.gcc.path").expect("law renders");
    assert_eq!(via_worker, via_law);
}
