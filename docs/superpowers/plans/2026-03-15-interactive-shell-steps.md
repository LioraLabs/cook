# Interactive Shell Steps (`@` prefix) Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `@` prefix support for bare shell steps that gives child processes full terminal access (inherited stdin/stdout/stderr) instead of captured output.

**Architecture:** The parser detects `@` on bare shell lines and sets `interactive: true` on `Step::Shell`. Codegen emits `cook.interactive()` instead of `cook.exec()`. The runtime registers `cook.interactive` in both normal and capture modes. The scheduler treats interactive nodes as barriers — draining all in-flight work before running them on the main thread with inherited stdio.

**Tech Stack:** Rust, mlua, std::process::Command

---

## File Structure

| File | Change | Responsibility |
|------|--------|---------------|
| `src/parser/ast.rs` | Modify | Add `interactive` field to `Step::Shell` |
| `src/parser/mod.rs` | Modify | Detect `@` prefix, strip it, set flag |
| `src/codegen/mod.rs` | Modify | Emit `cook.interactive()` for interactive steps |
| `src/runtime/api.rs` | Modify | Register `cook.interactive` in normal + capture modes |
| `src/scheduler/dag.rs` | Modify | Add `Interactive` variant to `WorkPayload` |
| `src/scheduler/builder.rs` | Modify | Handle interactive units as barriers |
| `src/scheduler/mod.rs` | Modify | Drain-and-run-on-main-thread logic for interactive nodes |
| `src/scheduler/pool.rs` | Modify | Defensive `Interactive` arm in `execute_work_item` |
| `tests/integration.rs` | Modify | Add integration test for `@` steps |

---

### Task 1: Add `interactive` field to `Step::Shell` in AST

**Files:**
- Modify: `src/parser/ast.rs:35-37`

- [ ] **Step 1: Write the failing test**

Add a test in `src/parser/ast.rs` that constructs a `Step::Shell` with `interactive: true`:

```rust
#[test]
fn test_interactive_shell_step() {
    let step = Step::Shell {
        command: "./bin/app".to_string(),
        line: 2,
        interactive: true,
    };
    match step {
        Step::Shell { interactive, .. } => assert!(interactive),
        _ => panic!("expected Shell step"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_interactive_shell_step -- --nocapture`
Expected: FAIL — `Step::Shell` does not have `interactive` field.

- [ ] **Step 3: Add `interactive` field to `Step::Shell`**

In `src/parser/ast.rs`, change the `Shell` variant from:
```rust
Shell { command: String, line: usize },
```
to:
```rust
Shell { command: String, line: usize, interactive: bool },
```

- [ ] **Step 4: Fix all compilation errors from the new field**

Every existing `Step::Shell { command, line }` pattern and construction needs `interactive: false` added. Locations:

- `src/parser/ast.rs` tests: `test_recipe_no_metadata` (line 79)
- `src/parser/mod.rs`: construction at line 299, line 342
- `src/codegen/mod.rs`: match arm at line 29, test helper constructions
- `tests/integration.rs`: any direct `Step::Shell` constructions (none expected — integration tests use Cookfile strings)

Run: `cargo build` to find all locations. Add `interactive: false` to each.

- [ ] **Step 5: Run all tests to verify nothing broke**

Run: `cargo test`
Expected: All existing tests PASS, new test PASS.

- [ ] **Step 6: Commit**

```bash
git add src/parser/ast.rs src/parser/mod.rs src/codegen/mod.rs
git commit -m "feat(ast): add interactive field to Step::Shell"
```

---

### Task 2: Parse `@` prefix on bare shell lines

**Files:**
- Modify: `src/parser/mod.rs:296-303`

- [ ] **Step 1: Write failing test for `@` parsing**

Add in `src/parser/mod.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn test_interactive_shell_step() {
    let source = "recipe \"run\"\n    @./bin/app\nend\n";
    let result = parse(source).unwrap();
    let step = &result.recipes[0].steps[0];
    match step {
        Step::Shell { command, interactive, .. } => {
            assert!(interactive, "expected interactive=true");
            assert_eq!(command, "./bin/app", "@ should be stripped from command");
        }
        other => panic!("expected Shell step, got {:?}", other),
    }
}

#[test]
fn test_non_interactive_shell_step() {
    let source = "recipe \"build\"\n    echo hello\nend\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Shell { interactive, .. } => {
            assert!(!interactive, "expected interactive=false for normal shell step");
        }
        other => panic!("expected Shell step, got {:?}", other),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_interactive_shell_step test_non_interactive_shell_step -- --nocapture`
Expected: `test_interactive_shell_step` FAILS (@ not stripped, interactive not set).

- [ ] **Step 3: Implement `@` detection in parser**

In `src/parser/mod.rs`, in `parse_recipe()`, find the `else` branch at line 298 that creates `Step::Shell` from `Token::Content(text)`. Change it from:

```rust
} else {
    steps.push(Step::Shell {
        command: text.clone(),
        line: tok.line,
    });
}
```

to:

```rust
} else if let Some(cmd) = text.strip_prefix('@') {
    let cmd = cmd.to_string();
    if cmd.is_empty() {
        return Err(ParseError::Parse {
            line: tok.line,
            message: "interactive '@' prefix requires a command".to_string(),
        });
    }
    steps.push(Step::Shell {
        command: cmd,
        line: tok.line,
        interactive: true,
    });
} else {
    steps.push(Step::Shell {
        command: text.clone(),
        line: tok.line,
        interactive: false,
    });
}
```

- [ ] **Step 4: Add test for empty `@` parse error**

```rust
#[test]
fn test_empty_interactive_step_errors() {
    let source = "recipe \"run\"\n    @\nend\n";
    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("requires a command"), "got: {err}");
}
```

- [ ] **Step 5: Run all tests**

Run: `cargo test`
Expected: All PASS.

- [ ] **Step 6: Commit**

```bash
git add src/parser/mod.rs
git commit -m "feat(parser): detect @ prefix on bare shell steps"
```

---

### Task 3: Emit `cook.interactive()` in codegen

**Files:**
- Modify: `src/codegen/mod.rs:29-31`

- [ ] **Step 1: Write failing test**

Add in `src/codegen/mod.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn test_interactive_shell_step() {
    let cookfile = make_cookfile(vec![make_recipe(
        "run",
        vec![],
        vec![],
        vec![Step::Shell {
            command: "./bin/app".to_string(),
            line: 5,
            interactive: true,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.interactive([[./bin/app]], 5)"),
        "expected cook.interactive call, got: {output}"
    );
    assert!(
        !output.contains("cook.exec"),
        "interactive step should not emit cook.exec"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_interactive_shell_step -p cook -- --nocapture`
Expected: FAIL — codegen emits `cook.exec` not `cook.interactive`.

- [ ] **Step 3: Implement codegen for interactive steps**

In `src/codegen/mod.rs`, change the `Step::Shell` match arm from:

```rust
Step::Shell { command, line } => {
    let wrapped = wrap_lua_string(command);
    out.push_str(&format!("    cook.exec({}, {})\n", wrapped, line));
}
```

to:

```rust
Step::Shell { command, line, interactive } => {
    let wrapped = wrap_lua_string(command);
    if *interactive {
        out.push_str(&format!("    cook.interactive({}, {})\n", wrapped, line));
    } else {
        out.push_str(&format!("    cook.exec({}, {})\n", wrapped, line));
    }
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All PASS.

- [ ] **Step 5: Commit**

```bash
git add src/codegen/mod.rs
git commit -m "feat(codegen): emit cook.interactive() for @ steps"
```

---

### Task 4: Add `Interactive` variant to `WorkPayload`

**Files:**
- Modify: `src/scheduler/dag.rs:4-12`

- [ ] **Step 1: Add the variant**

In `src/scheduler/dag.rs`, add to the `WorkPayload` enum:

```rust
#[derive(Debug, Clone)]
pub enum WorkPayload {
    Shell { cmd: String, line: usize },
    LuaChunk {
        code: String,
        input: String,
        output: String,
        ingredient_groups: Vec<Vec<String>>,
    },
    Interactive { cmd: String, line: usize },
}
```

- [ ] **Step 2: Fix compilation — update `is_presatisfied` in `builder.rs`**

In `src/scheduler/builder.rs`, the `is_presatisfied` function matches on `WorkPayload::Shell`. The `Interactive` variant is never presatisfied, so add it:

```rust
fn is_presatisfied(unit: &CapturedUnit) -> bool {
    match &unit.payload {
        WorkPayload::Shell { cmd, .. } => cmd.is_empty() && unit.cache_meta.is_none(),
        _ => false,
    }
}
```

This already handles `Interactive` via the `_ => false` arm. No change needed.

- [ ] **Step 3: Fix compilation — update `execute_work_item` in `pool.rs`**

In `src/scheduler/pool.rs`, `execute_work_item` matches on `WorkPayload`. Add an arm for `Interactive` that panics (interactive steps should never be dispatched to the pool):

```rust
WorkPayload::Interactive { .. } => {
    WorkResult {
        id: work.id,
        success: false,
        error: Some("BUG: interactive step dispatched to worker pool".to_string()),
    }
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All PASS.

- [ ] **Step 5: Commit**

```bash
git add src/scheduler/dag.rs src/scheduler/pool.rs
git commit -m "feat(dag): add Interactive variant to WorkPayload"
```

---

### Task 5: Register `cook.interactive` in runtime API

**Files:**
- Modify: `src/runtime/api.rs`

- [ ] **Step 1: Write failing test for normal-mode `cook.interactive`**

Add in `src/runtime/mod.rs` tests (or `src/runtime/api.rs` tests if they exist separately — check which file has the runtime tests):

```rust
#[test]
fn test_runtime_interactive_shell() {
    let dir = tempfile::tempdir().unwrap();
    let cookfile_str = r#"recipe "run"
    @echo interactive_output
end"#;
    let cookfile = crate::parser::parse(cookfile_str).unwrap();
    let lua_source = crate::codegen::generate(&cookfile);
    assert!(lua_source.contains("cook.interactive"));
}
```

This verifies the full pipeline (parse → codegen) produces `cook.interactive`.

- [ ] **Step 2: Register `cook.interactive` in normal mode**

In `src/runtime/api.rs`, inside `register_cook_api()`, after the `cook.exec` registration (around line 71), add:

```rust
// cook.interactive(cmd, line) — run with inherited stdio (real terminal)
let wd_i = working_dir.clone();
let env_i = env_vars.clone();
let interactive_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
    run_interactive_command(&cmd, &wd_i, &env_i, line)
})?;
cook.set("interactive", interactive_fn)?;
```

- [ ] **Step 3: Implement `run_interactive_command`**

Add this function in `src/runtime/api.rs` near `run_shell_command`:

```rust
fn run_interactive_command(
    cmd: &str,
    wd: &std::path::Path,
    env: &HashMap<String, String>,
    line: usize,
) -> mlua::Result<String> {
    let mut child_env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in env {
        child_env.insert(k.clone(), v.clone());
    }

    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(wd)
        .envs(&child_env)
        .status()
        .map_err(|e| mlua::Error::runtime(format!("failed to execute: {e}")))?;

    if !status.success() {
        let code = status.code().unwrap_or(1);
        return Err(mlua::Error::runtime(format!(
            "COOK_CMD_FAILED:{}:{}:{}",
            line, code, cmd
        )));
    }

    Ok(String::new())
}
```

- [ ] **Step 4: Register `cook.interactive` in capture mode**

In `src/runtime/api.rs`, inside `register_cook_api_capture()`, after the `cook.exec` capture registration (around line 484), add:

```rust
// cook.interactive(cmd, line) — capture mode
let cs_i = capture_state.clone();
let interactive_capture_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
    let mut state = cs_i.borrow_mut();
    let unit = CapturedUnit {
        payload: WorkPayload::Interactive {
            cmd: cmd.clone(),
            line,
        },
        cache_meta: None,
        dep_kind: DepKind::Sequential,
    };
    state.units.push(unit);
    Ok("".to_string())
})?;
cook.set("interactive", interactive_capture_fn)?;
```

- [ ] **Step 5: Run all tests**

Run: `cargo test`
Expected: All PASS.

- [ ] **Step 6: Commit**

```bash
git add src/runtime/api.rs
git commit -m "feat(runtime): register cook.interactive in normal and capture modes"
```

---

### Task 6: Handle interactive nodes in scheduler

**Files:**
- Modify: `src/scheduler/mod.rs:50-211`

- [ ] **Step 1: Write failing test**

Add in `src/scheduler/mod.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn test_scheduler_interactive_node() {
    let (wd, _tmp) = tmp_dir();

    let mut dag = ExecutionDag::new();
    // Normal shell node, then interactive node
    let a = dag.add_node(shell("echo setup"), "setup".to_string(), None, &[]);
    dag.add_node(
        WorkPayload::Interactive {
            cmd: "echo interactive".to_string(),
            line: 5,
        },
        "run".to_string(),
        None,
        &[a],
    );

    let result = execute_dag(dag, 2, wd, HashMap::new(), false, None);
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_scheduler_interactive_node -- --nocapture`
Expected: FAIL — `execute_dag` dispatches interactive node to pool, which returns the BUG error.

- [ ] **Step 3: Implement interactive node handling in `execute_dag`**

The key change is in `process_ready` and the main loop. When a ready node has an `Interactive` payload, instead of dispatching to the pool, we need to:
1. Wait for all pending pool work to complete
2. Run the interactive command on the main thread
3. Report the result directly

Modify `execute_dag` in `src/scheduler/mod.rs`. Change `process_ready` to return a signal when it encounters an interactive node instead of submitting it, then handle it in the main loop.

Replace the `process_ready` function and adjust the main loop:

```rust
fn process_ready(
    dag: &ExecutionDag,
    id: usize,
    pool: &WorkerPool,
    cancelled: &mut Vec<bool>,
    finished: &mut usize,
    interactive_queue: &mut Vec<usize>,
) -> usize {
    if cancelled[id] {
        *finished += 1;
        return 0;
    }

    let node = dag.node(id);
    match &node.payload {
        None => {
            // Pre-satisfied: complete immediately and cascade.
            *finished += 1;
            let newly_ready = dag.complete(id);
            let mut submitted = 0;
            for nid in newly_ready {
                submitted += process_ready(dag, nid, pool, cancelled, finished, interactive_queue);
            }
            submitted
        }
        Some(WorkPayload::Interactive { .. }) => {
            // Queue for main-thread execution after pool drains.
            interactive_queue.push(id);
            0
        }
        Some(payload) => {
            pool.submit(WorkItem {
                id,
                payload: payload.clone(),
                recipe_name: node.recipe_name.clone(),
            });
            1
        }
    }
}
```

Then update the main loop in `execute_dag` to handle the interactive queue. After seeding and after each `rx.recv()` result processing, check if there are queued interactive nodes. If there are and `pending == 0` (pool is drained), run them on the main thread:

```rust
// After the existing main loop, add interactive handling.
// The full revised loop structure:

let mut interactive_queue: Vec<usize> = Vec::new();

// Seed
let initial = dag.initial_ready();
for id in initial {
    pending += process_ready(&dag, id, &pool, &mut cancelled, &mut finished, &mut interactive_queue);
}

loop {
    // If pool is drained and we have interactive nodes queued, run them.
    while pending == 0 && !interactive_queue.is_empty() {
        let id = interactive_queue.remove(0);
        if cancelled[id] {
            finished += 1;
            continue;
        }

        let node = dag.node(id);
        if let Some(WorkPayload::Interactive { cmd, line }) = &node.payload {
            let result = run_interactive_on_main(cmd, *line, &working_dir, &env_vars);
            finished += 1;

            if result.is_ok() {
                let newly_ready = dag.complete(id);
                for nid in newly_ready {
                    pending += process_ready(&dag, nid, &pool, &mut cancelled, &mut finished, &mut interactive_queue);
                }
            } else {
                let err_msg = result.unwrap_err();
                failures.push((id, node.recipe_name.clone(), err_msg));
                for &dep_id in &dag.node(id).dependents {
                    cancel_subtree(&dag, dep_id, &mut cancelled);
                }
            }
        }
    }

    // If nothing left, break.
    if pending == 0 && interactive_queue.is_empty() {
        break;
    }

    // Wait for pool results.
    let result = rx.recv().expect("worker channel closed unexpectedly");
    pending -= 1;
    finished += 1;

    if result.success {
        // Update cache entry if this node has cache metadata
        if let Some(ref cm) = cache_manager {
            if let Some(ref meta) = dag.node(result.id).cache_meta {
                let new_inputs: Vec<crate::cache::store::FileRecord> = meta
                    .input_paths
                    .iter()
                    .map(|rel| {
                        let abs = working_dir.join(rel);
                        crate::cache::store::FileRecord {
                            path: rel.clone(),
                            mtime: crate::cache::check::stat_mtime(&abs).unwrap_or(0),
                            hash: crate::cache::check::hash_file(&abs).unwrap_or(0),
                        }
                    })
                    .collect();
                let new_output = meta.output_path.as_ref().map(|rel| {
                    let abs = working_dir.join(rel);
                    crate::cache::store::FileRecord {
                        path: rel.clone(),
                        mtime: crate::cache::check::stat_mtime(&abs).unwrap_or(0),
                        hash: crate::cache::check::hash_file(&abs).unwrap_or(0),
                    }
                });
                cm.update_step(
                    &meta.recipe_name,
                    &meta.cache_key,
                    crate::cache::store::StepEntry {
                        inputs: new_inputs,
                        output: new_output,
                        command_hash: meta.command_hash,
                    },
                );
            }
        }

        let newly_ready = dag.complete(result.id);
        for id in newly_ready {
            pending += process_ready(&dag, id, &pool, &mut cancelled, &mut finished, &mut interactive_queue);
        }
    } else {
        let node = dag.node(result.id);
        let err_msg = result
            .error
            .unwrap_or_else(|| "unknown error".to_string());
        failures.push((result.id, node.recipe_name.clone(), err_msg));

        for &dep_id in &dag.node(result.id).dependents {
            cancel_subtree(&dag, dep_id, &mut cancelled);
        }
    }
}
```

Add the `run_interactive_on_main` helper:

```rust
fn run_interactive_on_main(
    cmd: &str,
    line: usize,
    working_dir: &std::path::Path,
    env_vars: &HashMap<String, String>,
) -> Result<(), String> {
    let mut child_env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in env_vars {
        child_env.insert(k.clone(), v.clone());
    }

    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(working_dir)
        .envs(&child_env)
        .status()
        .map_err(|e| format!("failed to execute: {e}"))?;

    if !status.success() {
        let code = status.code().unwrap_or(1);
        return Err(format!("COOK_CMD_FAILED:{}:{}:{}", line, code, cmd));
    }

    Ok(())
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All PASS.

- [ ] **Step 5: Commit**

```bash
git add src/scheduler/mod.rs
git commit -m "feat(scheduler): drain pool and run interactive nodes on main thread"
```

---

### Task 7: Integration test

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: Write integration test**

Add to `tests/integration.rs`:

```rust
#[test]
fn test_interactive_shell_step() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "run"
    echo "setup done"
    @echo "interactive step ran"
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("run")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "interactive step failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_interactive_step_with_dependency() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "run": "build"
    @echo "running app"
end

recipe "build"
    echo "building" > build.log
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("run")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "interactive with dep failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Verify build ran first
    assert!(dir.path().join("build.log").exists(), "build.log should exist");
}

#[test]
fn test_interactive_step_failure_propagates() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "run"
    @false
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("run")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "interactive step with 'false' should fail"
    );
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test integration`
Expected: All PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: add integration tests for interactive @ shell steps"
```

---

### Task 8: Emit Lua test and parse-error test for `@` in cook/plate

**Files:**
- Modify: `tests/integration.rs`
- Modify: `src/codegen/mod.rs` (test only)

- [ ] **Step 1: Write codegen emit-lua integration test**

Add to `tests/integration.rs`:

```rust
#[test]
fn test_interactive_step_emits_cook_interactive_in_lua() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "run"
    @./bin/app
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("--emit-lua")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("cook.interactive"),
        "emit-lua should contain cook.interactive, got: {stdout}"
    );
}
```

- [ ] **Step 2: Run test**

Run: `cargo test test_interactive_step_emits_cook_interactive_in_lua --test integration`
Expected: PASS.

- [ ] **Step 3: Write parser-level test that `@` in `cook using` clause is rejected**

The `@` prefix should only work on bare shell steps. The `cook using` clause parses its command via `parse_single_quoted_string`, so `@` in the string is just a character — it won't trigger the interactive parser path. However, we should verify this doesn't accidentally produce an interactive step.

Add a parser test in `src/parser/mod.rs`:

```rust
#[test]
fn test_at_in_cook_using_is_not_interactive() {
    let source = r#"recipe "build"
    ingredients "src/*.c"
    cook "build/{stem}.o" using "@gcc -c {in} -o {out}"
end
"#;
    let result = parse(source).unwrap();
    // The cook step's using clause should contain the @ literally
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            match &step.using_clause {
                Some(UsingClause::Shell(cmd)) => {
                    assert!(cmd.starts_with('@'), "@ should be preserved in using clause");
                }
                other => panic!("expected Shell using clause, got {:?}", other),
            }
        }
        other => panic!("expected Cook step, got {:?}", other),
    }
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/integration.rs src/parser/mod.rs
git commit -m "test: verify emit-lua output and @ in cook/plate for interactive steps"
```

---

### Task 9: Guard `cook serve` against interactive steps

**Files:**
- Modify: `src/cli/mod.rs`

The spec requires that `@` steps under `cook serve` produce a runtime error. Since `cook serve` calls `cmd_run` which goes through the full registration → DAG → execution pipeline, interactive steps will hit the scheduler's `run_interactive_on_main`. The simplest guard is to check during DAG building or execution whether we're in serve mode and an interactive node is encountered.

- [ ] **Step 1: Add a `has_interactive_steps` check**

In `src/cli/mod.rs`, in `cmd_serve()`, after `read_and_parse` and `resolve_execution_order`, check whether any recipe in the execution order contains an `@` step. If so, return an error immediately.

```rust
// In cmd_serve(), after reading and parsing:
for recipe in &cookfile.recipes {
    for step in &recipe.steps {
        if let crate::parser::ast::Step::Shell { interactive: true, line, .. } = step {
            return Err(CookError::Other(format!(
                "line {}: interactive '@' steps are not supported under 'cook serve'",
                line
            )));
        }
    }
}
```

- [ ] **Step 2: Write integration test**

Add to `tests/integration.rs`:

```rust
#[test]
fn test_cook_serve_rejects_interactive_steps() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    @./bin/app
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("serve")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "cook serve should reject interactive steps"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not supported under"),
        "expected serve rejection message, got: {stderr}"
    );
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All PASS.

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs tests/integration.rs
git commit -m "feat(cli): reject interactive @ steps under cook serve"
```
