# Cook Testing System Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `test` Cookfile keyword and `cook.test_case()` Lua API that produces unified JUnit XML + JSON test reports across all languages.

**Architecture:** The `test` keyword is a sibling to `plate` — it iterates over cook step outputs and runs each as a test binary. Unlike `plate`, test failures don't cancel sibling tests, stdout/stderr are captured, and results are aggregated into structured output. Language modules call `cook.test_case()` to report fine-grained results.

**Tech Stack:** Rust (parser, codegen, runtime, scheduler, CLI), Lua (module API), XML/JSON (output)

**Spec:** `docs/superpowers/specs/2026-03-16-testing-design.md`

---

## File Structure

### New files:
- `src/codegen/test_step.rs` — Lua codegen for `test` keyword (parallel to `plate_step.rs`)
- `src/runtime/test_api.rs` — `TestResultStore`, `cook.test_case()` registration, JUnit XML + JSON serializers
- `src/engine/test_output.rs` — Terminal summary printer, file writers for XML/JSON

### Modified files:
- `src/parser/lexer.rs` — Add `"test"` to reserved keywords
- `src/parser/ast.rs` — Add `TestStep` struct and `Step::Test` variant
- `src/parser/recipe.rs` — Parse `test` keyword with optional `timeout`/`should_fail`
- `src/codegen/mod.rs` — Re-export `test_step` module
- `src/codegen/recipe.rs` — Handle `Step::Test` in recipe codegen
- `src/codegen/template.rs` — Add `expand_test_cmd()` function
- `src/contracts/mod.rs` — Add `WorkPayload::Test`, `DepKind::TestSibling`, `TestResult`
- `src/runtime/engine.rs` — Register test API, pass `TestResultStore` through
- `src/runtime/capture.rs` — Add `cook.test_layer()` capture function
- `src/runtime/mod.rs` — Re-export `test_api` module
- `src/scheduler/dag.rs` — Change `dependents` to carry edge kind metadata
- `src/scheduler/builder.rs` — Emit `DepKind::TestSibling` edges for test units
- `src/scheduler/executor.rs` — Handle `WorkPayload::Test`, skip `TestSibling` in `cancel_subtree`
- `src/scheduler/pool.rs` — Add `WorkPayload::Test` arm to `execute_work_item` match
- `src/engine/error.rs` — Add `TestFailure` variant to `CookError`
- `src/cli/mod.rs` — Add `Command::Test` variant
- `src/engine/commands.rs` — Add `cmd_test()` function
- `src/engine/mod.rs` — Re-export test_output
- `tests/integration.rs` — Add test keyword integration tests

---

## Chunk 1: Parser & AST

### Task 1: Add TestStep to AST

**Files:**
- Modify: `src/parser/ast.rs:37-48`

- [ ] **Step 1: Write the failing test**

In `src/parser/tests.rs`, add:

```rust
#[test]
fn test_test_step_basic() {
    let source = r#"recipe "run-tests"
    ingredients "tests/*.c"
    cook "build/{stem}" using "cc {in} -o {out}"
    test "./{out}"
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 2);
    match &recipe.steps[1] {
        Step::Test { step, line } => {
            assert_eq!(*line, 4);
            assert_eq!(step.command, "./{out}");
            assert_eq!(step.timeout, None);
            assert!(!step.should_fail);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_test_step_basic -- --nocapture`
Expected: FAIL — `TestStep` struct doesn't exist, `Step::Test` variant doesn't exist

- [ ] **Step 3: Add TestStep struct and Step::Test variant**

In `src/parser/ast.rs`, after the `PlateStep` struct (line 39), add:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct TestStep {
    pub command: String,
    pub timeout: Option<u64>,
    pub should_fail: bool,
}
```

In the `Step` enum (line 42-48), add a new variant after `Plate`:

```rust
    Test { step: TestStep, line: usize },
```

- [ ] **Step 4: Run test — still fails (parser doesn't recognize `test` yet)**

Run: `cargo test test_test_step_basic -- --nocapture`
Expected: FAIL — parser doesn't produce `Step::Test`

### Task 2: Parse `test` keyword

**Files:**
- Modify: `src/parser/lexer.rs:54-56`
- Modify: `src/parser/recipe.rs:93-98`

- [ ] **Step 5: Add `"test"` to reserved keywords**

In `src/parser/lexer.rs`, find the `matches!()` call inside `try_parse_var_decl()` (line 54) and add `"test"`:

```rust
if matches!(name, "recipe" | "config" | "end" | "ingredients" | "cook" | "plate" | "using" | "use" | "test") {
```

Note: There is no `RESERVED` constant — keywords are checked inline via `matches!()`.

- [ ] **Step 6: Add test step parsing in recipe parser**

In `src/parser/recipe.rs`, after the plate parsing block (around line 93-98), add:

```rust
} else if let Some(rest) = strip_keyword(text, "test") {
    let (command, rest) = parse_test_command(rest, tok.line)?;
    let (timeout, rest) = parse_test_timeout(rest);
    let should_fail = rest.trim() == "should_fail";
    steps.push(Step::Test {
        step: TestStep {
            command,
            timeout,
            should_fail,
        },
        line: tok.line,
    });
}
```

Also add these helper functions in `src/parser/recipe.rs` (or `cook_line.rs` if that's where helpers live):

```rust
fn parse_test_command(text: &str, line: usize) -> Result<(String, &str), ParseError> {
    let text = text.trim();
    if !text.starts_with('"') {
        return Err(ParseError::Parse {
            line,
            message: format!("expected '\"', found: {}", text),
        });
    }
    let rest = &text[1..];
    let end = rest.find('"').ok_or(ParseError::Parse {
        line,
        message: "unterminated string".to_string(),
    })?;
    Ok((rest[..end].to_string(), rest[end + 1..].trim()))
}

fn parse_test_timeout(text: &str) -> (Option<u64>, &str) {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("timeout") {
        let rest = rest.trim();
        if let Some((num_str, remainder)) = rest.split_once(|c: char| c.is_whitespace()) {
            if let Ok(n) = num_str.parse::<u64>() {
                return (Some(n), remainder);
            }
        } else if let Ok(n) = rest.parse::<u64>() {
            return (Some(n), "");
        }
    }
    (None, text)
}
```

Make sure to add the necessary imports at the top of the file:
```rust
use super::ast::TestStep;
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test test_test_step_basic -- --nocapture`
Expected: PASS

- [ ] **Step 8: Write tests for timeout and should_fail options**

In `src/parser/tests.rs`, add:

```rust
#[test]
fn test_test_step_with_timeout() {
    let source = r#"recipe "run-tests"
    test "./{out}" timeout 30
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    match &recipe.steps[0] {
        Step::Test { step, .. } => {
            assert_eq!(step.command, "./{out}");
            assert_eq!(step.timeout, Some(30));
            assert!(!step.should_fail);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_test_step_with_should_fail() {
    let source = r#"recipe "run-tests"
    test "./{out}" should_fail
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    match &recipe.steps[0] {
        Step::Test { step, .. } => {
            assert_eq!(step.command, "./{out}");
            assert_eq!(step.timeout, None);
            assert!(step.should_fail);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_test_step_with_timeout_and_should_fail() {
    let source = r#"recipe "run-tests"
    test "./{out}" timeout 60 should_fail
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    match &recipe.steps[0] {
        Step::Test { step, .. } => {
            assert_eq!(step.command, "./{out}");
            assert_eq!(step.timeout, Some(60));
            assert!(step.should_fail);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}
```

- [ ] **Step 9: Run all parser tests**

Run: `cargo test --lib -- test_test_step`
Expected: All 4 tests PASS

- [ ] **Step 10: Commit**

```bash
git add src/parser/ast.rs src/parser/lexer.rs src/parser/recipe.rs src/parser/tests.rs
git commit -m "feat(parser): add test keyword with timeout and should_fail options"
```

---

## Chunk 2: Codegen

### Task 3: Generate Lua code for test steps

**Files:**
- Create: `src/codegen/test_step.rs`
- Modify: `src/codegen/mod.rs`
- Modify: `src/codegen/recipe.rs:68-75`
- Modify: `src/codegen/template.rs:31-36`

- [ ] **Step 1: Write the failing test**

In `src/codegen/tests.rs`, add:

```rust
#[test]
fn test_codegen_test_step() {
    let source = r#"recipe "run-tests"
    ingredients "tests/*.c"
    cook "build/{stem}" using "cc {in} -o {out}"
    test "./{out}"
end
"#;
    let ast = crate::parser::parse(source).unwrap();
    let lua = generate(&ast);
    assert!(lua.contains("cook.test_layer"), "expected cook.test_layer call in:\n{lua}");
    assert!(lua.contains("_test_out"), "expected _test_out variable in:\n{lua}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_codegen_test_step -- --nocapture`
Expected: FAIL — codegen doesn't handle `Step::Test` yet

- [ ] **Step 3: Add expand_test_cmd to template.rs**

In `src/codegen/template.rs`, after `expand_plate_cmd` (line 31-36), add:

```rust
pub(crate) fn expand_test_cmd(template: &str) -> String {
    let builtins = &[("{out}", "_test_out")];
    expand_template_with_env_fallback(template, builtins)
}
```

- [ ] **Step 4: Create test_step.rs**

Create `src/codegen/test_step.rs`:

```rust
use crate::contracts::hash_str;
use crate::parser::ast::*;

use super::template::expand_test_cmd;

pub(super) fn generate_test_step(
    out: &mut String,
    test_step: &TestStep,
    line: usize,
    last_cook_index: Option<usize>,
) {
    let source = if let Some(idx) = last_cook_index {
        format!("_cook_outputs_{}", idx)
    } else {
        "recipe.ingredients[1]".to_string()
    };

    let command_hash = hash_str(&test_step.command);
    let cmd_expr = expand_test_cmd(&test_step.command);
    let timeout = test_step.timeout.unwrap_or(300);
    let should_fail = if test_step.should_fail { "true" } else { "false" };

    out.push_str(&format!(
        "    for _, _test_out in ipairs({}) do\n",
        source
    ));
    out.push_str(&format!(
        "        cook.test_layer(_test_out, {}, {}, {}, function()\n",
        command_hash, timeout, should_fail
    ));
    out.push_str(&format!(
        "            cook.exec({}, {})\n",
        cmd_expr, line
    ));
    out.push_str("        end)\n");
    out.push_str("    end\n");
}
```

- [ ] **Step 5: Wire test_step into codegen**

In `src/codegen/mod.rs`, add:

```rust
mod test_step;
```

In `src/codegen/recipe.rs`, add a match arm after the `Step::Plate` arm (around line 68-75):

```rust
Step::Test {
    step: test_step,
    line,
} => {
    out.push_str("    cook.begin_step()\n");
    test_step::generate_test_step(&mut out, test_step, *line, prev_cook_index);
    out.push_str("    cook.end_step()\n");
}
```

Add the import at the top of `recipe.rs`:

```rust
use super::test_step;
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test test_codegen_test_step -- --nocapture`
Expected: PASS

- [ ] **Step 7: Write test for timeout and should_fail codegen**

In `src/codegen/tests.rs`, add:

```rust
#[test]
fn test_codegen_test_step_with_options() {
    let source = r#"recipe "run-tests"
    test "./{out}" timeout 30 should_fail
end
"#;
    let ast = crate::parser::parse(source).unwrap();
    let lua = generate(&ast);
    assert!(lua.contains("30"), "expected timeout 30 in:\n{lua}");
    assert!(lua.contains("true"), "expected should_fail true in:\n{lua}");
}
```

- [ ] **Step 8: Run test to verify it passes**

Run: `cargo test test_codegen_test_step_with_options -- --nocapture`
Expected: PASS

- [ ] **Step 9: Run all existing tests to ensure no regressions**

Run: `cargo test --lib`
Expected: All tests PASS

- [ ] **Step 10: Commit**

```bash
git add src/codegen/test_step.rs src/codegen/mod.rs src/codegen/recipe.rs src/codegen/template.rs src/codegen/tests.rs
git commit -m "feat(codegen): generate Lua for test keyword with cook.test_layer"
```

---

## Chunk 3: Contracts & TestResultStore

### Task 4: Add test-related types to contracts

**Files:**
- Modify: `src/contracts/mod.rs`

- [ ] **Step 1: Add TestResult and TestSuiteResults to contracts**

In `src/contracts/mod.rs`, add these types:

```rust
use std::collections::BTreeMap;

/// Result of a single test case execution.
#[derive(Debug, Clone)]
pub struct TestCaseResult {
    pub name: String,
    pub suite: String,
    pub status: TestStatus,
    pub time: f64,
    pub output: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TestStatus {
    Pass,
    Fail,
    Skip,
    Error,
}

/// Aggregated results for a single test suite.
#[derive(Debug, Clone)]
pub struct TestSuiteResult {
    pub name: String,
    pub cases: Vec<TestCaseResult>,
}

/// All test results for an entire build.
#[derive(Debug, Clone, Default)]
pub struct TestResults {
    pub suites: BTreeMap<String, TestSuiteResult>,
}

impl TestResults {
    pub fn new() -> Self {
        Self {
            suites: BTreeMap::new(),
        }
    }

    pub fn add_case(&mut self, suite_name: &str, case: TestCaseResult) {
        let suite = self.suites.entry(suite_name.to_string()).or_insert_with(|| {
            TestSuiteResult {
                name: suite_name.to_string(),
                cases: Vec::new(),
            }
        });
        suite.cases.push(case);
    }

    pub fn total_tests(&self) -> usize {
        self.suites.values().map(|s| s.cases.len()).sum()
    }

    pub fn total_passed(&self) -> usize {
        self.suites.values().flat_map(|s| &s.cases).filter(|c| c.status == TestStatus::Pass).count()
    }

    pub fn total_failed(&self) -> usize {
        self.suites.values().flat_map(|s| &s.cases).filter(|c| c.status == TestStatus::Fail).count()
    }

    pub fn total_errors(&self) -> usize {
        self.suites.values().flat_map(|s| &s.cases).filter(|c| c.status == TestStatus::Error).count()
    }

    pub fn total_skipped(&self) -> usize {
        self.suites.values().flat_map(|s| &s.cases).filter(|c| c.status == TestStatus::Skip).count()
    }

    pub fn total_time(&self) -> f64 {
        self.suites.values().flat_map(|s| &s.cases).map(|c| c.time).sum()
    }

    pub fn has_failures(&self) -> bool {
        self.total_failed() > 0 || self.total_errors() > 0
    }
}
```

- [ ] **Step 2: Add WorkPayload::Test variant**

In `src/contracts/mod.rs`, find the `WorkPayload` enum and add:

```rust
    Test {
        cmd: String,
        line: usize,
        timeout: u64,
        should_fail: bool,
        suite_name: String,
        test_name: String,
    },
```

- [ ] **Step 3: Add DepKind::TestSibling variant**

In `src/contracts/mod.rs`, find the `DepKind` enum and add:

```rust
    TestSibling(usize),  // like StepGroup but failures don't cancel siblings
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check`
Expected: Compiles (there may be "unused" warnings, which is fine)

- [ ] **Step 5: Commit**

```bash
git add src/contracts/mod.rs
git commit -m "feat(contracts): add TestResults, WorkPayload::Test, DepKind::TestSibling"
```

### Task 5: Create TestResultStore and cook.test_case API

**Files:**
- Create: `src/runtime/test_api.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/runtime/engine.rs`

- [ ] **Step 6: Create test_api.rs with TestResultStore and registration**

Create `src/runtime/test_api.rs`:

```rust
use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, Result as LuaResult};

use crate::contracts::{TestCaseResult, TestResults, TestStatus};

pub type SharedTestResults = Rc<RefCell<TestResults>>;

pub fn register_test_api(lua: &Lua, results: SharedTestResults) -> LuaResult<()> {
    let cook: mlua::Table = lua.globals().get("cook")?;

    let results_clone = results.clone();
    let test_case_fn = lua.create_function(move |_lua, args: (String, String, mlua::Table)| {
        let suite_name: String = args.0;
        let test_name: String = args.1;
        let opts: mlua::Table = args.2;

        let status_str: String = opts.get("status")?;
        let status = match status_str.as_str() {
            "pass" => TestStatus::Pass,
            "fail" => TestStatus::Fail,
            "skip" => TestStatus::Skip,
            "error" => TestStatus::Error,
            _ => TestStatus::Error,
        };

        let time: f64 = opts.get::<Option<f64>>("time")?.unwrap_or(0.0);
        let output: String = opts.get::<Option<String>>("output")?.unwrap_or_default();
        let message: String = opts.get::<Option<String>>("message")?.unwrap_or_default();

        let case = TestCaseResult {
            name: test_name,
            suite: suite_name.clone(),
            status,
            time,
            output,
            message,
        };

        results_clone.borrow_mut().add_case(&suite_name, case);
        Ok(())
    })?;

    cook.set("test_case", test_case_fn)?;
    Ok(())
}
```

- [ ] **Step 7: Register test API in engine.rs**

In `src/runtime/mod.rs`, add:

```rust
pub mod test_api;
```

In `src/runtime/engine.rs`, add the import:

```rust
use super::test_api::{register_test_api, SharedTestResults};
```

Add a `test_results: SharedTestResults` field to the `Runtime` struct. Initialize it in `Runtime::new()`:

```rust
test_results: Rc::new(RefCell::new(TestResults::new())),
```

In `register_recipe()`, after the `register_export_api` call, add:

```rust
register_test_api(&lua, self.test_results.clone())?;
```

Add a public method to access results:

```rust
pub fn test_results(&self) -> SharedTestResults {
    self.test_results.clone()
}
```

- [ ] **Step 8: Verify compilation**

Run: `cargo check`
Expected: Compiles cleanly

- [ ] **Step 9: Commit**

```bash
git add src/runtime/test_api.rs src/runtime/mod.rs src/runtime/engine.rs
git commit -m "feat(runtime): add TestResultStore and cook.test_case Lua API"
```

---

## Chunk 4: Test Execution in Scheduler

### Task 6: Add test_layer capture function

**Files:**
- Modify: `src/runtime/capture.rs`

- [ ] **Step 1: Add cook.test_layer() capture function**

In `src/runtime/capture.rs`, in the function that registers the layer API (look for where `cook.layer` is registered), add a new `cook.test_layer` registration below it. This function is similar to `cook.layer` but:

- Always uses `nil` output (like plate)
- Produces `WorkPayload::Test` instead of `WorkPayload::Shell`
- Uses `DepKind::TestSibling` instead of the current group's `DepKind::StepGroup`

The function signature from the generated Lua is:
`cook.test_layer(_test_out, command_hash, timeout, should_fail, function() cook.exec(cmd, line) end)`

```rust
// Use MultiValue parsing for safety (consistent with cook.layer's approach)
let test_layer_fn = lua.create_function(move |_lua, args: mlua::MultiValue| {
    let mut args_iter = args.into_iter();
    let input: String = match args_iter.next() {
        Some(mlua::Value::String(s)) => s.to_string_lossy().to_string(),
        _ => return Err(mlua::Error::runtime("test_layer: expected string input")),
    };
    let command_hash: u64 = match args_iter.next() {
        Some(mlua::Value::Integer(n)) => n as u64,
        Some(mlua::Value::Number(n)) => n as u64,
        _ => return Err(mlua::Error::runtime("test_layer: expected number command_hash")),
    };
    let timeout: u64 = match args_iter.next() {
        Some(mlua::Value::Integer(n)) => n as u64,
        Some(mlua::Value::Number(n)) => n as u64,
        _ => 300,
    };
    let should_fail: bool = match args_iter.next() {
        Some(mlua::Value::Boolean(b)) => b,
        _ => false,
    };
    let body: mlua::Function = match args_iter.next() {
        Some(mlua::Value::Function(f)) => f,
        _ => return Err(mlua::Error::runtime("test_layer: expected function body")),
    };

    // Check cache (same as plate — no output)
    let input_refs: Vec<&str> = vec![input.as_str()];
    let mut cstate = cache_state_clone.borrow_mut();
    let cache_key = format!("{}@{:x}", input, command_hash);
    let existing = cstate.cache.steps.get(&cache_key);
    let (result, updated_entry) = crate::cache::check::needs_rebuild_plate(
        existing, &input_refs, command_hash, &wd_clone,
    );

    if let crate::cache::check::RebuildResult::Skip = &result {
        // Presatisfied — register empty test unit
        let mut cap = capture_state_clone.borrow_mut();
        let dep_kind = if let Some(group_idx) = cap.current_group {
            DepKind::TestSibling(group_idx)
        } else {
            DepKind::Sequential
        };
        let unit_idx = cap.units.len();
        cap.units.push(CapturedUnit {
            payload: WorkPayload::Test {
                cmd: String::new(),
                line: 0,
                timeout,
                should_fail,
                suite_name: recipe_name_clone.clone(),
                test_name: input.clone(),
            },
            cache_meta: None,
            dep_kind: dep_kind.clone(),
        });
        if let DepKind::TestSibling(gi) = &dep_kind {
            cap.step_groups[*gi].push(unit_idx);
        }
        return Ok(());
    }

    // Need to rebuild — dry-run body to capture cook.exec
    {
        let mut cap = capture_state_clone.borrow_mut();
        cap.inside_layer = true;
        cap.layer_commands.clear();
    }

    body.call::<()>(())?;

    let mut cap = capture_state_clone.borrow_mut();
    cap.inside_layer = false;
    let (cmd, line) = cap.layer_commands.pop().unwrap_or_else(|| (String::new(), 0));
    cap.layer_commands.clear();

    let dep_kind = if let Some(group_idx) = cap.current_group {
        DepKind::TestSibling(group_idx)
    } else {
        DepKind::Sequential
    };
    let unit_idx = cap.units.len();
    cap.units.push(CapturedUnit {
        payload: WorkPayload::Test {
            cmd,
            line,
            timeout,
            should_fail,
            suite_name: recipe_name_clone.clone(),
            test_name: input.clone(),
        },
        cache_meta: Some(CacheMeta {
            inputs: input_refs.iter().map(|s| s.to_string()).collect(),
            output: cache_key.clone(),
            command_hash,
        }),
        dep_kind: dep_kind.clone(),
    });
    if let DepKind::TestSibling(gi) = &dep_kind {
        cap.step_groups[*gi].push(unit_idx);
    }

    if let Some(entry) = updated_entry {
        cstate.cache.steps.insert(cache_key, entry);
    }

    Ok(())
})?;

cook.set("test_layer", test_layer_fn)?;
```

Note: This is structurally very similar to the existing `cook.layer()` function. The key differences are:
1. Produces `WorkPayload::Test` instead of `WorkPayload::Shell`
2. Uses `DepKind::TestSibling` instead of `DepKind::StepGroup`
3. Accepts `timeout` and `should_fail` parameters
4. Passes `suite_name` (= recipe name) and `test_name` (= input binary) into the payload

You will need to clone the appropriate `Rc` references to use inside the closure, following the exact same pattern as the existing `cook.layer()` function. Look at what variables `cook.layer()` captures and clone the same ones.

- [ ] **Step 2: Verify compilation**

Run: `cargo check`
Expected: Compiles (may have warnings about unused WorkPayload::Test in match arms — that's OK, we'll handle it in executor next)

- [ ] **Step 3: Commit**

```bash
git add src/runtime/capture.rs
git commit -m "feat(runtime): add cook.test_layer capture function for test keyword"
```

### Task 7: Handle WorkPayload::Test and DepKind::TestSibling in scheduler

**Files:**
- Modify: `src/scheduler/dag.rs`
- Modify: `src/scheduler/builder.rs`
- Modify: `src/scheduler/executor.rs`

- [ ] **Step 4: Update DagNode to carry edge kind metadata**

In `src/scheduler/dag.rs`, change the `dependents` field:

```rust
pub struct DagNode {
    pub id: usize,
    pub payload: Option<WorkPayload>,
    pub recipe_name: String,
    pub cache_meta: Option<CacheMeta>,
    pub dependents: Vec<(usize, EdgeKind)>,  // was Vec<usize>
    pub remaining_deps: AtomicUsize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EdgeKind {
    Normal,
    TestSibling,
}
```

Update all code in `dag.rs` that pushes to `dependents` — it should push `(id, EdgeKind::Normal)` by default. Update the `complete()` method to iterate over `(dep_id, _edge_kind)` tuples.

In `add_node()` and `add_presatisfied()`, where it does:
```rust
self.nodes[dep_id].dependents.push(new_id);
```
Change to:
```rust
self.nodes[dep_id].dependents.push((new_id, EdgeKind::Normal));
```

In `complete()`, where it iterates:
```rust
for &dep_id in dependents {
```
Change to:
```rust
for &(dep_id, _) in dependents {
```

- [ ] **Step 5: Handle DepKind::TestSibling in DAG builder**

In `src/scheduler/builder.rs`, update the match on `unit.dep_kind` to handle `TestSibling`:

```rust
DepKind::TestSibling(gi) => {
    group_dag_ids.entry(gi).or_default().push(dag_id);
    // Same group tracking as StepGroup
    if let Some(&(_, pos)) = unit_group_info.get(&unit_idx) {
        let group_size = ru.step_groups[gi].len();
        if pos + 1 == group_size {
            barrier = group_dag_ids[&gi].clone();
        }
    }
}
```

**Edge kind mapping between DepKind and EdgeKind:**

The `DepKind` (in contracts) tells the builder what kind of unit this is. The `EdgeKind` (in dag.rs) annotates edges in the DAG for cancel_subtree behavior.

The mapping:
- Edges FROM barrier nodes TO test units = `EdgeKind::Normal` (tests still depend on build completing)
- When test units form the new barrier, edges FROM test units TO subsequent nodes = `EdgeKind::TestSibling` (so a failing test doesn't cancel other tests waiting on the same barrier)

Add a method to `ExecutionDag`:
```rust
pub fn add_node_with_edge_kind(
    &mut self,
    payload: WorkPayload,
    recipe_name: String,
    cache_meta: Option<CacheMeta>,
    dep_ids: &[usize],
    outgoing_edge_kind: EdgeKind,  // edge kind for dependents OF this node
) -> usize
```

The `outgoing_edge_kind` is stored on the node and used when this node's dependents are registered. When `add_node_with_edge_kind` pushes to `dep.dependents`, it uses the provided `EdgeKind` instead of `EdgeKind::Normal`.

In the builder, for `DepKind::TestSibling` units, call `add_node_with_edge_kind(..., EdgeKind::TestSibling)`. For all other units, use the existing `add_node` which defaults to `EdgeKind::Normal`.

Also update `is_presatisfied` in `builder.rs` to handle `WorkPayload::Test`:

```rust
fn is_presatisfied(unit: &CapturedUnit) -> bool {
    match &unit.payload {
        WorkPayload::Shell { cmd, .. } => cmd.is_empty() && unit.cache_meta.is_none(),
        WorkPayload::Test { cmd, .. } => cmd.is_empty() && unit.cache_meta.is_none(),
        _ => false,
    }
}
```

Without this, cached test units would be dispatched to the pool with empty commands instead of being skipped.

- [ ] **Step 6: Update cancel_subtree to skip TestSibling edges**

In `src/scheduler/executor.rs`, update `cancel_subtree`:

```rust
fn cancel_subtree(dag: &ExecutionDag, node_id: usize, cancelled: &mut Vec<bool>) {
    if cancelled[node_id] {
        return;
    }
    cancelled[node_id] = true;
    for &(dep_id, ref edge_kind) in &dag.node(node_id).dependents {
        if *edge_kind == EdgeKind::TestSibling {
            continue;  // Don't cancel sibling tests
        }
        cancel_subtree(dag, dep_id, cancelled);
    }
}
```

- [ ] **Step 7: Handle WorkPayload::Test in executor**

In `src/scheduler/executor.rs`, in the `process_ready` function (or wherever `WorkPayload::Shell` is dispatched to the thread pool), add handling for `WorkPayload::Test`:

```rust
WorkPayload::Test { cmd, line, timeout, should_fail, suite_name, test_name } => {
    // Execute like Shell but capture stdout/stderr separately
    // and apply timeout
    let result = run_test_in_worker(
        cmd, line, timeout, should_fail, &working_dir, &env_vars,
    );
    // Send result back through channel
}
```

Add a `run_test_in_worker` function that:
1. Spawns the command via `std::process::Command` with stdout/stderr captured (`Stdio::piped()`)
2. Waits with a timeout (`child.wait_timeout(Duration::from_secs(timeout))` — use the `wait_timeout` crate or a manual approach with a thread)
3. Returns a `WorkerResult` with success/failure + captured output

For the MVP, a simple approach to timeout: spawn the child, then use a thread to wait on it with a deadline. If the deadline passes, kill the child.

```rust
fn run_test_in_worker(
    cmd: &str,
    line: usize,
    timeout_secs: u64,
    should_fail: bool,
    working_dir: &Path,
    env_vars: &HashMap<String, String>,
) -> (bool, String, String, f64) {
    let start = std::time::Instant::now();

    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", cmd])
        .current_dir(working_dir)
        .envs(env_vars)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn test process");

    let timeout = std::time::Duration::from_secs(timeout_secs);

    // Wait with timeout
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };

    let elapsed = start.elapsed().as_secs_f64();

    let stdout = child.stdout.take().map(|mut s| {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut s, &mut buf).ok();
        buf
    }).unwrap_or_default();

    let stderr = child.stderr.take().map(|mut s| {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut s, &mut buf).ok();
        buf
    }).unwrap_or_default();

    let success = match status {
        Some(s) => {
            let code_ok = s.success();
            if should_fail { !code_ok } else { code_ok }
        }
        None => false, // timeout = failure
    };

    (success, stdout, stderr, elapsed)
}
```

**Threading test results back from the scheduler:**

The `TestResultStore` is `Rc<RefCell<>>` (not thread-safe), but the executor runs work on a thread pool. The solution: extend `WorkResult` (returned through the mpsc channel from workers) to optionally carry test output:

In `src/scheduler/pool.rs`, extend `WorkResult`:
```rust
pub struct WorkResult {
    pub id: usize,
    pub success: bool,
    pub error: Option<String>,
    pub test_output: Option<TestOutput>,  // NEW
}

pub struct TestOutput {
    pub suite_name: String,
    pub test_name: String,
    pub stdout: String,
    pub stderr: String,
    pub duration: f64,
    pub timed_out: bool,
    pub should_fail: bool,
    pub exit_success: bool,
}
```

The executor's main loop (in `executor.rs`) already receives `WorkResult` via the channel. After receiving a test result, it converts the `TestOutput` into a `TestCaseResult` and stores it. Since the executor's main loop runs on a single thread, it can hold an `&mut Vec<TestCaseResult>` without any thread-safety concerns.

Change `execute_dag`'s signature to return collected test results:
```rust
pub fn execute_dag(...) -> Result<Vec<TestCaseResult>, SchedulerError>
```

All existing callers (`cmd_run`, `cmd_serve`) can ignore the returned vec with `let _ =` or `?` since they won't have test units. Only `cmd_test` will use the results.

**Handling WorkPayload::Test in pool.rs:**

In `src/scheduler/pool.rs`, in `execute_work_item` (line 258-291), add a match arm:

```rust
WorkPayload::Test { cmd, line, timeout, should_fail, suite_name, test_name } => {
    execute_test(work.id, cmd, *line, *timeout, *should_fail, suite_name, test_name, working_dir, env_vars)
}
```

Add the `execute_test` function in pool.rs:
```rust
fn execute_test(
    id: usize,
    cmd: &str,
    _line: usize,
    timeout_secs: u64,
    should_fail: bool,
    suite_name: &str,
    test_name: &str,
    working_dir: &PathBuf,
    env_vars: &HashMap<String, String>,
) -> WorkResult {
    use std::io::Read;
    let start = std::time::Instant::now();

    let child = std::process::Command::new("/bin/sh")
        .args(["-c", cmd])
        .current_dir(working_dir)
        .envs(env_vars)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => return WorkResult {
            id,
            success: false,
            error: Some(format!("failed to spawn test: {e}")),
            test_output: Some(TestOutput {
                suite_name: suite_name.to_string(),
                test_name: test_name.to_string(),
                stdout: String::new(),
                stderr: format!("failed to spawn: {e}"),
                duration: 0.0,
                timed_out: false,
                should_fail,
                exit_success: false,
            }),
        },
    };

    // Read stdout/stderr in separate threads to avoid deadlock
    // (child may block on write if pipe buffer fills)
    let stdout_handle = child.stdout.take().map(|s| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let mut reader = std::io::BufReader::new(s);
            reader.read_to_string(&mut buf).ok();
            buf
        })
    });
    let stderr_handle = child.stderr.take().map(|s| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let mut reader = std::io::BufReader::new(s);
            reader.read_to_string(&mut buf).ok();
            buf
        })
    });

    // Wait with timeout
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let timed_out;
    let exit_success;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                timed_out = false;
                exit_success = status.success();
                break;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    exit_success = false;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => {
                timed_out = false;
                exit_success = false;
                break;
            }
        }
    }

    let stdout = stdout_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = stderr_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let duration = start.elapsed().as_secs_f64();

    let success = if should_fail { !exit_success } else { exit_success };

    WorkResult {
        id,
        success,
        error: if success { None } else { Some(format!("test failed: {test_name}")) },
        test_output: Some(TestOutput {
            suite_name: suite_name.to_string(),
            test_name: test_name.to_string(),
            stdout,
            stderr,
            duration,
            timed_out,
            should_fail,
            exit_success,
        }),
    }
}
```

Note: stdout/stderr are drained in separate threads BEFORE waiting for the child to prevent pipe-buffer deadlocks.

**Add CookError::TestFailure:**

In `src/engine/error.rs`, add to the `CookError` enum:
```rust
    TestFailure(usize),  // number of failed tests
```

In `exit_code()`:
```rust
    CookError::TestFailure(_) => 1,
```

In `Display`:
```rust
    CookError::TestFailure(n) => write!(f, "{n} test(s) failed"),
```

- [ ] **Step 8: Verify compilation**

Run: `cargo check`
Expected: Compiles

- [ ] **Step 9: Run existing tests**

Run: `cargo test`
Expected: All existing tests still pass (no regressions from DepKind/EdgeKind changes)

- [ ] **Step 10: Commit**

```bash
git add src/scheduler/dag.rs src/scheduler/builder.rs src/scheduler/executor.rs src/contracts/mod.rs
git commit -m "feat(scheduler): handle WorkPayload::Test with continue-on-failure semantics"
```

---

## Chunk 5: Test Output (JUnit XML + JSON + Terminal)

### Task 8: Write JUnit XML and JSON serializers

**Files:**
- Create: `src/engine/test_output.rs`
- Modify: `src/engine/mod.rs`

- [ ] **Step 1: Write unit tests for XML output**

In `src/engine/test_output.rs` (create the file), add tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{TestCaseResult, TestResults, TestStatus};

    fn sample_results() -> TestResults {
        let mut results = TestResults::new();
        results.add_case("suite-a", TestCaseResult {
            name: "test_pass".into(),
            suite: "suite-a".into(),
            status: TestStatus::Pass,
            time: 0.1,
            output: String::new(),
            message: String::new(),
        });
        results.add_case("suite-a", TestCaseResult {
            name: "test_fail".into(),
            suite: "suite-a".into(),
            status: TestStatus::Fail,
            time: 0.5,
            output: "expected 4 got 5".into(),
            message: "assertion failed".into(),
        });
        results.add_case("suite-b", TestCaseResult {
            name: "test_skip".into(),
            suite: "suite-b".into(),
            status: TestStatus::Skip,
            time: 0.0,
            output: String::new(),
            message: "not implemented".into(),
        });
        results
    }

    #[test]
    fn test_junit_xml_output() {
        let results = sample_results();
        let xml = to_junit_xml(&results);
        assert!(xml.contains("<?xml version="));
        assert!(xml.contains("<testsuites"));
        assert!(xml.contains(r#"tests="3""#));
        assert!(xml.contains(r#"failures="1""#));
        assert!(xml.contains(r#"errors="0""#));
        assert!(xml.contains(r#"<testsuite name="suite-a""#));
        assert!(xml.contains(r#"<testsuite name="suite-b""#));
        assert!(xml.contains(r#"<testcase name="test_pass""#));
        assert!(xml.contains("<failure"));
        assert!(xml.contains("<skipped"));
    }

    #[test]
    fn test_json_output() {
        let results = sample_results();
        let json = to_json(&results);
        assert!(json.contains(r#""suites""#));
        assert!(json.contains(r#""suite-a""#));
        assert!(json.contains(r#""test_pass""#));
        assert!(json.contains(r#""status":"pass""#));
        assert!(json.contains(r#""status":"fail""#));
        assert!(json.contains(r#""summary""#));
    }

    #[test]
    fn test_terminal_summary() {
        let results = sample_results();
        let summary = format_terminal_summary(&results);
        assert!(summary.contains("suite-a"));
        assert!(summary.contains("1 passed"));
        assert!(summary.contains("FAILED"));
        assert!(summary.contains("3 tests:"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_junit_xml_output -- --nocapture`
Expected: FAIL — functions don't exist

- [ ] **Step 3: Implement the serializers**

In `src/engine/test_output.rs`:

```rust
use crate::contracts::{TestResults, TestStatus};
use std::path::Path;

/// Escape XML special characters.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Serialize test results to JUnit XML format.
pub fn to_junit_xml(results: &TestResults) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!(
        "<testsuites tests=\"{}\" failures=\"{}\" errors=\"{}\" time=\"{:.3}\">\n",
        results.total_tests(),
        results.total_failed(),
        results.total_errors(),
        results.total_time(),
    ));

    for suite in results.suites.values() {
        let tests = suite.cases.len();
        let failures = suite.cases.iter().filter(|c| c.status == TestStatus::Fail).count();
        let errors = suite.cases.iter().filter(|c| c.status == TestStatus::Error).count();
        let time: f64 = suite.cases.iter().map(|c| c.time).sum();

        xml.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"{}\" time=\"{:.3}\">\n",
            xml_escape(&suite.name), tests, failures, errors, time,
        ));

        for case in &suite.cases {
            xml.push_str(&format!(
                "    <testcase name=\"{}\" classname=\"{}\" time=\"{:.3}\"",
                xml_escape(&case.name),
                xml_escape(&case.suite),
                case.time,
            ));

            match case.status {
                TestStatus::Pass => {
                    xml.push_str("/>\n");
                }
                TestStatus::Fail => {
                    xml.push_str(">\n");
                    xml.push_str(&format!(
                        "      <failure message=\"{}\" type=\"TestFailure\">{}</failure>\n",
                        xml_escape(&case.message),
                        xml_escape(&case.output),
                    ));
                    xml.push_str("    </testcase>\n");
                }
                TestStatus::Error => {
                    xml.push_str(">\n");
                    xml.push_str(&format!(
                        "      <error message=\"{}\" type=\"TestError\">{}</error>\n",
                        xml_escape(&case.message),
                        xml_escape(&case.output),
                    ));
                    xml.push_str("    </testcase>\n");
                }
                TestStatus::Skip => {
                    xml.push_str(">\n");
                    xml.push_str(&format!(
                        "      <skipped message=\"{}\"/>\n",
                        xml_escape(&case.message),
                    ));
                    xml.push_str("    </testcase>\n");
                }
            }
        }

        xml.push_str("  </testsuite>\n");
    }

    xml.push_str("</testsuites>\n");
    xml
}

/// Serialize test results to JSON format for Cook Cloud.
pub fn to_json(results: &TestResults) -> String {
    let mut json = String::new();
    json.push_str("{\n");

    // Timestamp
    let now = chrono_now();
    json.push_str(&format!("  \"timestamp\":\"{}\",\n", now));

    // Suites
    json.push_str("  \"suites\":[\n");
    let suite_count = results.suites.len();
    for (i, suite) in results.suites.values().enumerate() {
        json.push_str("    {\n");
        json.push_str(&format!("      \"name\":\"{}\",\n", json_escape(&suite.name)));
        json.push_str(&format!("      \"tests\":{},\n", suite.cases.len()));
        let failures = suite.cases.iter().filter(|c| c.status == TestStatus::Fail).count();
        let errors = suite.cases.iter().filter(|c| c.status == TestStatus::Error).count();
        let time: f64 = suite.cases.iter().map(|c| c.time).sum();
        json.push_str(&format!("      \"failures\":{},\n", failures));
        json.push_str(&format!("      \"errors\":{},\n", errors));
        json.push_str(&format!("      \"time\":{:.3},\n", time));
        json.push_str("      \"cases\":[\n");

        for (j, case) in suite.cases.iter().enumerate() {
            let status_str = match case.status {
                TestStatus::Pass => "pass",
                TestStatus::Fail => "fail",
                TestStatus::Skip => "skip",
                TestStatus::Error => "error",
            };
            json.push_str(&format!(
                "        {{\"name\":\"{}\",\"status\":\"{}\",\"time\":{:.3}}}",
                json_escape(&case.name), status_str, case.time,
            ));
            if j + 1 < suite.cases.len() {
                json.push(',');
            }
            json.push('\n');
        }

        json.push_str("      ]\n");
        json.push_str("    }");
        if i + 1 < suite_count {
            json.push(',');
        }
        json.push('\n');
    }
    json.push_str("  ],\n");

    // Summary
    json.push_str(&format!(
        "  \"summary\":{{\"tests\":{},\"passed\":{},\"failed\":{},\"errors\":{},\"skipped\":{},\"time\":{:.3}}}\n",
        results.total_tests(),
        results.total_passed(),
        results.total_failed(),
        results.total_errors(),
        results.total_skipped(),
        results.total_time(),
    ));

    json.push_str("}\n");
    json
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn chrono_now() -> String {
    // Use std::time for a simple ISO 8601 timestamp
    // For proper formatting, consider adding the `chrono` or `time` crate
    // For now, use a Unix timestamp as ISO 8601
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // Simple ISO 8601 — good enough for MVP
    format!("{}",duration.as_secs())
}

/// Format a human-readable terminal summary.
pub fn format_terminal_summary(results: &TestResults) -> String {
    let mut out = String::new();

    for suite in results.suites.values() {
        let passed = suite.cases.iter().filter(|c| c.status == TestStatus::Pass).count();
        let failed = suite.cases.iter().filter(|c| c.status == TestStatus::Fail || c.status == TestStatus::Error).count();
        let skipped = suite.cases.iter().filter(|c| c.status == TestStatus::Skip).count();

        out.push_str(&format!("test {} ", suite.name));
        out.push_str("... ");

        let mut parts = Vec::new();
        if passed > 0 {
            parts.push(format!("{} passed", passed));
        }
        if failed > 0 {
            parts.push(format!("{} FAILED", failed));
        }
        if skipped > 0 {
            parts.push(format!("{} skipped", skipped));
        }
        out.push_str(&parts.join(", "));
        out.push('\n');
    }

    out.push_str(&format!(
        "\n{} tests: {} passed, {} failed, {} skipped ({:.1}s)\n",
        results.total_tests(),
        results.total_passed(),
        results.total_failed() + results.total_errors(),
        results.total_skipped(),
        results.total_time(),
    ));

    out
}

/// Write test results to files in the .cook directory.
pub fn write_test_results(results: &TestResults, cook_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(cook_dir)?;

    let xml = to_junit_xml(results);
    std::fs::write(cook_dir.join("test-results.xml"), xml)?;

    let json = to_json(results);
    std::fs::write(cook_dir.join("test-results.json"), json)?;

    Ok(())
}
```

- [ ] **Step 4: Wire up test_output in engine/mod.rs**

In `src/engine/mod.rs`, add:

```rust
pub mod test_output;
```

- [ ] **Step 5: Run tests**

Run: `cargo test test_junit_xml_output test_json_output test_terminal_summary -- --nocapture`
Expected: All 3 PASS

- [ ] **Step 6: Commit**

```bash
git add src/engine/test_output.rs src/engine/mod.rs
git commit -m "feat(engine): add JUnit XML, JSON, and terminal summary test output writers"
```

---

## Chunk 6: CLI & Engine Integration

### Task 9: Add cook test CLI subcommand

**Files:**
- Modify: `src/cli/mod.rs`
- Modify: `src/engine/commands.rs` (or `src/engine/pipeline.rs`)

- [ ] **Step 1: Add Command::Test to CLI**

In `src/cli/mod.rs`, add a `Test` variant to the `Command` enum:

```rust
    /// Run all test recipes
    Test {
        /// Filter tests by name substring
        #[arg(long)]
        filter: Option<String>,

        /// Show all test output (don't capture)
        #[arg(long)]
        verbose: bool,

        /// Multiply all timeouts by this factor
        #[arg(long, default_value = "1")]
        timeout_multiplier: u64,

        /// Run every test through this wrapper command
        #[arg(long)]
        wrapper: Option<String>,

        /// List tests without running them
        #[arg(long)]
        list: bool,
    },
```

- [ ] **Step 2: Wire Command::Test to engine**

In the main dispatch (wherever `Command::Build` etc are matched), add:

```rust
Command::Test { filter, verbose, timeout_multiplier, wrapper, list } => {
    engine::cmd_test(cli, filter, verbose, timeout_multiplier, wrapper, list)?;
}
```

- [ ] **Step 3: Implement cmd_test in engine**

In the engine commands file, add `cmd_test()`. This follows the same structure as `cmd_run()` but:

1. Parses and generates Lua as normal
2. Resolves execution order for ALL recipes (or a specific test recipe)
3. Executes each recipe, collecting test results
4. After all recipes finish, writes test output and prints summary

```rust
pub fn cmd_test(
    cli: &Cli,
    filter: Option<String>,
    verbose: bool,
    timeout_multiplier: u64,
    wrapper: Option<String>,
    list: bool,
) -> Result<(), CookError> {
    let (cookfile, lua_source) = read_and_parse(cli)?;

    let cookfile_dir = cli.file.parent().unwrap_or(std::path::Path::new("."));
    let dotenv_vars = load_env(cookfile_dir);
    let env_vars = resolve_env(&cookfile, None, dotenv_vars, &cli.set)?;

    // Find all recipes that contain test steps
    let test_recipes: Vec<String> = cookfile
        .recipes
        .iter()
        .filter(|r| r.steps.iter().any(|s| matches!(s, Step::Test { .. })))
        .map(|r| r.name.clone())
        .collect();

    if test_recipes.is_empty() {
        eprintln!("cook: no test recipes found");
        return Ok(());
    }

    // Resolve execution order including dependencies
    // (test recipes may depend on build recipes)
    let mut rt = Runtime::new(cookfile_dir.to_path_buf(), env_vars.clone());
    rt.set_quiet(cli.quiet);

    let num_jobs = cli.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    });

    let cache_dir = cookfile_dir.join(".cook").join("cache");
    let cache_manager = std::sync::Arc::new(crate::cache::ThreadSafeCacheManager::new(cache_dir));

    for recipe_name in &test_recipes {
        let order = analyzer::resolve_execution_order(&cookfile, recipe_name)?;
        for name in &order {
            eprintln!("cook: registering recipe '{name}'");
            let units = rt.register_recipe(&lua_source, name)?;
            let dag = crate::scheduler::builder::build_dag(vec![units]);
            if dag.is_empty() { continue; }
            crate::scheduler::execute_dag(
                dag, num_jobs, cookfile_dir.to_path_buf(),
                env_vars.clone(), cli.quiet, Some(cache_manager.clone()),
            )?;
        }
    }

    // Write results
    let results = rt.test_results();
    let results = results.borrow();

    if results.total_tests() > 0 {
        let cook_dir = cookfile_dir.join(".cook");
        crate::engine::test_output::write_test_results(&results, &cook_dir)
            .map_err(|e| CookError::Other(format!("failed to write test results: {e}")))?;

        eprint!("{}", crate::engine::test_output::format_terminal_summary(&results));

        if results.has_failures() {
            return Err(CookError::TestFailure(results.total_failed() + results.total_errors()));
        }
    } else {
        eprintln!("cook: no tests were executed");
    }

    Ok(())
}
```

Add `Step` and `TestCaseResult` to imports so the `matches!` macro and result collection work.

**Important:** `execute_dag` now returns `Result<Vec<TestCaseResult>, SchedulerError>`. After each recipe's DAG execution, collect the returned `TestCaseResult` values and write them to the `TestResultStore`:

```rust
let test_case_results = crate::scheduler::execute_dag(
    dag, num_jobs, cookfile_dir.to_path_buf(),
    env_vars.clone(), cli.quiet, Some(cache_manager.clone()),
)?;

// Write test results from scheduler into the runtime's store
let store = rt.test_results();
let mut store = store.borrow_mut();
for case in test_case_results {
    store.add_case(&case.suite, case);
}
```

**Unimplemented CLI flags — emit warnings for MVP:**

At the top of `cmd_test`, add:
```rust
if filter.is_some() {
    eprintln!("cook: warning: --filter is not yet implemented, running all tests");
}
if verbose {
    eprintln!("cook: warning: --verbose is not yet implemented");
}
if timeout_multiplier != 1 {
    eprintln!("cook: warning: --timeout-multiplier is not yet implemented");
}
if wrapper.is_some() {
    eprintln!("cook: warning: --wrapper is not yet implemented");
}
if list {
    eprintln!("cook: warning: --list is not yet implemented");
}
```

This avoids silent no-ops that confuse users.

- [ ] **Step 4: Verify compilation**

Run: `cargo check`
Expected: Compiles

- [ ] **Step 5: Commit**

```bash
git add src/cli/mod.rs src/engine/commands.rs src/engine/pipeline.rs
git commit -m "feat(cli): add cook test subcommand with JUnit XML + JSON output"
```

---

## Chunk 7: Integration Test

### Task 10: End-to-end integration test

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: Write integration test for test keyword**

In `tests/integration.rs`, add:

```rust
/// Verify that `test` keyword:
/// - Parses correctly
/// - Generates Lua with cook.test_layer
/// - Produces TestStep AST nodes
#[test]
fn test_test_keyword_parse_and_codegen() {
    let source = r#"recipe "build"
    ingredients "tests/*.c"
    cook "build/{stem}" using "cc {in} -o {out}"
    test "./{out}"
end
"#;
    // Parse
    let ast = cook::parser::parse(source).unwrap();
    let recipe = &ast.recipes[0];
    assert_eq!(recipe.steps.len(), 2);
    assert!(matches!(&recipe.steps[1], cook::parser::ast::Step::Test { .. }));

    // Codegen
    let lua = cook::codegen::generate(&ast);
    assert!(lua.contains("cook.test_layer"), "generated Lua should contain cook.test_layer:\n{lua}");
}

#[test]
fn test_test_keyword_with_options() {
    let source = r#"recipe "build"
    test "./{out}" timeout 30 should_fail
end
"#;
    let ast = cook::parser::parse(source).unwrap();
    let recipe = &ast.recipes[0];
    match &recipe.steps[0] {
        cook::parser::ast::Step::Test { step, .. } => {
            assert_eq!(step.timeout, Some(30));
            assert!(step.should_fail);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test integration test_test_keyword`
Expected: PASS

- [ ] **Step 3: Write integration test for test output serialization**

```rust
#[test]
fn test_junit_xml_and_json_output() {
    use cook::contracts::{TestCaseResult, TestResults, TestStatus};
    use cook::engine::test_output::{to_junit_xml, to_json, format_terminal_summary};

    let mut results = TestResults::new();
    results.add_case("my-suite", TestCaseResult {
        name: "test_one".into(),
        suite: "my-suite".into(),
        status: TestStatus::Pass,
        time: 0.1,
        output: String::new(),
        message: String::new(),
    });
    results.add_case("my-suite", TestCaseResult {
        name: "test_two".into(),
        suite: "my-suite".into(),
        status: TestStatus::Fail,
        time: 0.5,
        output: "bad value".into(),
        message: "assert failed".into(),
    });

    let xml = to_junit_xml(&results);
    assert!(xml.contains(r#"<testsuites tests="2" failures="1" errors="0""#));

    let json = to_json(&results);
    assert!(json.contains(r#""passed":1"#));
    assert!(json.contains(r#""failed":1"#));

    let summary = format_terminal_summary(&results);
    assert!(summary.contains("1 passed"));
    assert!(summary.contains("1 FAILED"));
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All tests PASS, no regressions

- [ ] **Step 5: Commit**

```bash
git add tests/integration.rs
git commit -m "test: add integration tests for test keyword and test output"
```

---

## Chunk 8: cpp.test() Module Function

### Task 11: Add cpp.test() to the cpp module

**Files:**
- Modify: `examples/cpp-project/cook_modules/cpp.lua`

- [ ] **Step 1: Add cpp.test() function**

In `cook_modules/cpp.lua`, add a `cpp.test()` function after the existing `cpp.executable()` or `cpp.compile_commands()`:

```lua
--- Build and run test executables, reporting results via cook.test_case().
--- @param name string — suite name for test results
--- @param opts table — { dir, sources, links, standard, includes, defines, timeout, extra_cflags }
function cpp.test(name, opts)
    opts = opts or {}
    local timeout = opts.timeout or 300

    -- Discover test sources (same logic as static_library/executable)
    local sources = opts.sources
    if not sources and opts.dir then
        sources = fs.glob(opts.dir .. "*.cpp")
        local c_files = fs.glob(opts.dir .. "*.c")
        for _, f in ipairs(c_files) do
            sources[#sources + 1] = f
        end
        local cc_files = fs.glob(opts.dir .. "*.cc")
        for _, f in ipairs(cc_files) do
            sources[#sources + 1] = f
        end
    end

    if not sources or #sources == 0 then
        error("cpp.test('" .. name .. "'): no sources found")
    end

    -- Compile each test source into a binary
    local test_binaries = {}
    for _, src in ipairs(sources) do
        local stem = path.stem(src)
        local bin_path = "build/test/" .. name .. "/" .. stem

        -- Build compile flags (reuse existing flag-building logic)
        local flags = {}
        local compiler = detect_cxx(src)

        -- Resolve transitive includes/defines from linked targets
        local all_includes = opts.includes or {}
        local all_defines = opts.defines or {}
        local all_libs = {}
        local all_lib_flags = {}

        if opts.links then
            local link_data = resolve_links(opts.links)
            for _, inc in ipairs(link_data.includes) do
                all_includes[#all_includes + 1] = inc
            end
            for _, def in ipairs(link_data.defines) do
                all_defines[#all_defines + 1] = def
            end
            for _, lp in ipairs(link_data.lib_paths) do
                all_libs[#all_libs + 1] = lp
            end
        end

        for _, inc in ipairs(all_includes) do
            flags[#flags + 1] = "-I" .. inc
        end
        for _, def in ipairs(all_defines) do
            flags[#flags + 1] = "-D" .. def
        end
        if opts.standard then
            flags[#flags + 1] = "-std=" .. opts.standard
        end

        -- Compile + link in one step (test binaries are typically small)
        local link_flags = {}
        for _, lp in ipairs(all_libs) do
            link_flags[#link_flags + 1] = lp
        end
        if opts.system_libs then
            for _, lib in ipairs(opts.system_libs) do
                link_flags[#link_flags + 1] = "-l" .. lib
            end
        end

        local cmd = compiler .. " " .. src .. " "
            .. table.concat(flags, " ") .. " "
            .. table.concat(link_flags, " ") .. " "
            .. "-o " .. bin_path

        -- Register compile step
        cook.add_unit(
            { src },
            bin_path,
            cmd,
            { step_group = true }
        )

        test_binaries[#test_binaries + 1] = { bin = bin_path, name = stem }
    end

    -- Run each test binary via cook.test_layer for parallel execution
    -- and continue-on-failure semantics (runs through the scheduler, not immediately)
    cook.begin_step()
    for _, tb in ipairs(test_binaries) do
        local cmd_hash = cook.hash(tb.bin)
        cook.test_layer(tb.bin, cmd_hash, timeout, false, function()
            cook.exec(tb.bin, 0)
        end)
    end
    cook.end_step()
end
```

Note: The compile step uses `cook.add_unit()` for parallel compilation. Test execution uses `cook.test_layer()` which registers test units in the scheduler's DAG — they run in parallel with continue-on-failure semantics and produce results that feed into the JUnit XML / JSON output. The `cook.test_layer` function automatically calls `cook.test_case` after each test completes (handled by the executor).

- [ ] **Step 2: Update the example Cookfile to demonstrate cpp.test()**

Add a test recipe to `examples/cpp-project/Cookfile`:

```
recipe "test": "mathlib"
    >{
        cpp.test("mathlib-tests", {
            dir = "tests/",
            links = { "mathlib" },
            standard = "c++17",
        })
    }
end
```

- [ ] **Step 3: Commit**

```bash
git add examples/cpp-project/cook_modules/cpp.lua examples/cpp-project/Cookfile
git commit -m "feat(cpp): add cpp.test() module function for test compilation and execution"
```

---

## Summary

| Chunk | Tasks | What it delivers |
|-------|-------|-----------------|
| 1: Parser & AST | 1-2 | `test` keyword recognized, parsed with timeout/should_fail |
| 2: Codegen | 3 | Lua generation with `cook.test_layer()` calls |
| 3: Contracts & Store | 4-5 | TestResults types, cook.test_case() Lua API |
| 4: Scheduler | 6-7 | WorkPayload::Test execution, continue-on-failure |
| 5: Output | 8 | JUnit XML + JSON + terminal summary |
| 6: CLI | 9 | `cook test` subcommand |
| 7: Integration | 10 | End-to-end tests |
| 8: cpp module | 11 | `cpp.test()` convenience function |

After each chunk, all existing tests should still pass. Each chunk produces a working, committable state.
