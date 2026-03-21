# Cook Testing System — Design Spec

**Date:** 2026-03-16
**Status:** Approved
**Goal:** Unified, multi-language test orchestration with structured output for Cook Cloud.

## Overview

Cook's testing system has two layers:

1. **Cook core** — a Lua API for recording test results + a `test` Cookfile keyword for simple cases
2. **Language modules** — module-level `*.test()` functions that know how to run and parse tests for their language

A single `cook build test-all` recipe can compile test results from C++, Rust, Python (or any language with a module) into one JUnit XML + JSON report.

## Architecture

```
Cookfile recipe
  ├── test "./{out}"              ← Cookfile keyword (simple, plate-like)
  └── >{ cpp.test(...) }         ← module function (language-aware)
        rust.test(...)
        python.test(...)
              │
              ▼
        cook.test_case()          ← core Lua API (modules call this)
              │
              ▼
        Cook core aggregator
              │
              ├── .cook/test-results.xml   (JUnit XML)
              ├── .cook/test-results.json  (machine-readable)
              └── Terminal summary
```

## Layer 1: Cook Core

### Lua API

Modules report test results through a single function:

```lua
-- Record a test result. Suite is lazily created on first call with a given suite_name.
cook.test_case(suite_name, test_name, {
    status = "pass" | "fail" | "skip",
    time = 0.123,           -- seconds (optional)
    output = "...",          -- captured stdout+stderr (optional)
    message = "...",         -- failure/skip message (optional)
})
```

**Shared state:** Results are collected in a `TestResultStore` (`Rc<RefCell<BTreeMap<...>>>`) shared across recipe VMs, following the same pattern as `ExportStore` in `runtime/export_api.rs`. Registered on the `cook` table in `engine.rs` alongside `cook.export`/`cook.import`. All collections use `BTreeMap`/`BTreeSet` for deterministic output per CLAUDE.md conventions.

Cook core collects all reported results during recipe execution and writes aggregated output at the end of the build.

### `test` Cookfile Keyword

A sibling to `plate`. Iterates over previous `cook` step outputs, runs each as a test binary:

```
recipe "cpp-tests": "build"
    ingredients "tests/test_*.c"
    cook "build/{stem}" using "{CC} {in} -o {out}"
    test "./{out}"
end
```

Behavior:
- Iterates over outputs from the preceding `cook` step (like `plate`)
- Runs each binary in parallel via Cook's worker pool
- Captures stdout/stderr — only displays output for failures
- **Continues on failure** (runs ALL tests, unlike `plate`)
- Each binary becomes a `cook.test_case()` call under the hood
- Suite name = recipe name

### `test` Keyword Options

```
test "./{out}"                         -- basic
test "./{out}" timeout 30              -- per-test timeout (default: 300s)
test "./{out}" should_fail             -- expect non-zero exit
test "./{out}" timeout 30 should_fail  -- both (order matters: timeout before should_fail)
```

**Parser grammar:** Options are parsed as ordered optional trailing keywords after the command string, modeled on how `cook` handles `using`. The grammar is:

```
test_step := "test" <quoted_string> ["timeout" <integer>] ["should_fail"]
```

Options must appear in this fixed order (timeout before should_fail). This avoids combinatorial parsing complexity while covering all practical cases.

### CLI

**Breaking change:** `cook test` becomes a built-in subcommand. Currently, unknown subcommands are dispatched as recipe names via `External(Vec<String>)` in `cli/mod.rs`. Adding a `Test` variant to the `Command` enum means users with a recipe named "test" must use `cook build test` instead. This is an acceptable trade-off — `cook test` as a first-class command is more valuable than preserving the shorthand for a recipe name.

```
cook test                              -- run all recipes containing test steps
cook test --filter "math*"             -- substring match on test name
cook test --verbose                    -- stream output live
cook test --timeout-multiplier 3       -- scale all timeouts
cook test --wrapper valgrind           -- run every test through a tool
cook test --list                       -- print test names without running
```

### Output

Always produced (no flag needed):

**`.cook/test-results.xml`** — JUnit XML:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<testsuites tests="47" failures="2" errors="0" time="8.3">
  <testsuite name="cpp-tests/test_vec" tests="3" failures="0" errors="0" time="0.9">
    <testcase name="test_add" classname="cpp-tests/test_vec" time="0.3"/>
    <testcase name="test_dot" classname="cpp-tests/test_vec" time="0.3"/>
    <testcase name="test_cross" classname="cpp-tests/test_vec" time="0.3"/>
  </testsuite>
  <testsuite name="rust-tests/mylib" tests="30" failures="1" errors="0" time="5.2">
    <testcase name="math::test_overflow" classname="rust-tests/mylib" time="0.1">
      <failure message="assertion failed" type="TestFailure">stack trace...</failure>
    </testcase>
    ...
  </testsuite>
  <testsuite name="py-tests" tests="14" failures="1" errors="0" time="3.1">
    <testcase name="test_disabled_feature" classname="py-tests" time="0.0">
      <skipped message="not implemented yet"/>
    </testcase>
    ...
  </testsuite>
</testsuites>
```

Note: `errors` = infrastructure failures (test binary crashed/timed out), `failures` = assertion failures (test ran but failed). `classname` uses Cook's `suite_name` convention for grouping in CI dashboards.

**`.cook/test-results.json`** — machine-readable (Cook Cloud ingest format):
```json
{
  "suites": [
    {
      "name": "cpp-tests/test_vec",
      "tests": 3,
      "failures": 0,
      "time": 0.9,
      "cases": [
        { "name": "test_add", "status": "pass", "time": 0.3 },
        ...
      ]
    }
  ],
  "timestamp": "2026-03-16T14:30:00Z",
  "summary": { "tests": 47, "passed": 45, "failed": 2, "errors": 0, "skipped": 0, "time": 8.3 }
}
```

**Terminal summary:**
```
test cpp-tests/test_vec       ... 3 passed
test cpp-tests/test_util      ... 2 passed
test rust-tests/mylib         ... 29 passed, 1 FAILED
test py-tests                 ... 13 passed, 1 FAILED

47 tests: 45 passed, 2 failed, 0 skipped (8.3s)
```

### Capture/Execution Model Integration

The `test` keyword must work within Cook's two-phase architecture (capture → DAG execution). This is the key architectural difference from `plate`.

**Capture phase:** The `test` keyword's generated Lua code produces `CapturedUnit`s with a new `WorkPayload::Test` variant (added to `contracts/`). This variant carries:
- The command template (with `{out}` expansion, like plate)
- Timeout value (default 300s)
- `should_fail` flag
- Suite name (defaults to recipe name)

**Execution phase:** The scheduler runs `WorkPayload::Test` units through the worker pool, but with different semantics:
- **stdout/stderr are captured** separately (not mixed into `SharedWriter`). The worker captures both streams into buffers and returns them as part of a `TestResult` struct through the result channel.
- **Continue on failure:** Test units use a new `DepKind::TestSibling` edge between test units within the same recipe. The scheduler's `cancel_subtree` logic skips `TestSibling` edges — failure of one test does not cancel other tests in the same recipe. Cross-recipe dependencies (`DepKind::Normal`) still cancel as before.
- **Result collection:** After each `WorkPayload::Test` unit completes, the executor calls `cook.test_case()` on the `TestResultStore` with the captured exit code, stdout, stderr, and duration.

**Timeout enforcement:** The worker wraps test subprocess execution with a timeout. On timeout, the process is killed and a `fail` result is recorded with message "timed out after Ns".

### `cook serve` Interaction

`test` steps are supported under `cook serve` — re-running tests on file changes is a natural workflow. They are not interactive steps and do not need TTY access.

### Exit Codes

- 0 = all tests passed
- 1 = one or more tests failed
- 2 = configuration/build error (tests couldn't run)

## Layer 2: Language Modules

Modules provide language-specific `*.test()` functions that compile/run tests and report results via `cook.test_case()`.

### cpp.test()

```lua
cpp.test("test-suite-name", {
    dir = "tests/",                   -- auto-discover test sources
    -- or --
    sources = { "tests/test_vec.cpp" },
    links = { "mathlib" },
    standard = "c++17",
    timeout = 60,
})
```

What it does:
1. Compiles test sources into individual binaries (reuses `cpp.executable()` internals)
2. Runs each binary, captures exit code + output
3. Reports each binary via `cook.test_case()` (suite is auto-created on first call)
4. Per-binary granularity by default (Tier 1)

### rust.test() (future module)

```lua
rust.test("rust-tests", {
    package = "mylib",          -- cargo package name
    -- or --
    workspace = true,           -- test entire workspace
})
```

What it does:
1. Runs `cargo test --message-format json` (or `cargo nextest` if available)
2. Parses JSON output for individual test results
3. Reports each test via `cook.test_case()`

### python.test() (future module)

```lua
python.test("py-tests", {
    dir = "tests/",
    runner = "pytest",          -- or "unittest"
})
```

What it does:
1. Runs `pytest --junitxml=<tmpfile>` (or unittest equivalent)
2. Parses the JUnit XML for individual test results
3. Re-reports via `cook.test_case()`

### The "test-all" Pattern

```
use "cpp"
use "rust"
use "python"

recipe "test-all": "build"
    >{
        cpp.test("cpp-tests", { dir = "tests/cpp/", links = {"mathlib"} })
        rust.test("rust-tests", { package = "mylib" })
        python.test("py-tests", { dir = "tests/python/", runner = "pytest" })
    }
end
```

One recipe, all languages, one report. This is the monorepo testing story.

## Changes Required

### Contracts (`contracts/`)
- New `WorkPayload::Test` variant carrying: command template, timeout, should_fail flag, suite name
- New `TestResult` struct: exit_code, stdout, stderr, duration, suite_name, test_name
- New `DepKind::TestSibling` variant for test-to-test edges within a recipe

### Parser (`parser/`)
- New `Test` token in lexer (sibling to `Plate`)
- New `TestStep` AST node: `{ command: String, timeout: Option<u64>, should_fail: bool }`
- Grammar: `test <quoted_string> ["timeout" <integer>] ["should_fail"]` — fixed order, both optional
- New `parse_test_step()` in recipe parser, modeled on plate step parsing with trailing keyword extension

### Codegen (`codegen/`)
- New `test_step.rs` (parallel to `plate_step.rs`)
- Generates loop over outputs like plate, but emits `cook.test_layer()` calls instead of `cook.layer()`
- `cook.test_layer()` produces `CapturedUnit`s with `WorkPayload::Test`

### Runtime (`runtime/`)
- `TestResultStore`: `Rc<RefCell<BTreeMap<String, TestSuite>>>` shared across recipe VMs, following `ExportStore` pattern
- `cook.test_case(suite, name, opts)` — registered on `cook` table in `engine.rs`
- `cook.test_layer(output, command_hash, timeout, should_fail)` — capture-mode API producing test units
- JUnit XML serializer + JSON serializer, called at build completion
- Terminal summary printer

### Scheduler (`scheduler/`)
- Handle `WorkPayload::Test` in worker pool: capture stdout/stderr into buffers, enforce timeout, return `TestResult`
- `cancel_subtree` skips `DepKind::TestSibling` edges — test failures don't cancel sibling tests
- After test unit completion, write result into `TestResultStore`

### CLI (`cli/`)
- New `Command::Test` variant in `cli/mod.rs` (takes priority over `External` catch-all)
- Flags: `--filter`, `--verbose`, `--timeout-multiplier`, `--wrapper`, `--list`

### cpp Module (`cook_modules/cpp.lua`)
- `cpp.test()` function — compile + run + report (builds on existing compilation infrastructure, calls `cook.test_case()`)

## What This Does NOT Include

- Framework output parsing in Cook core (that's the module's job)
- Cook's own test framework (Cook is framework-agnostic)
- HTML report generation (CI dashboards / Cook Cloud handle this)
- Code coverage (use `--wrapper` with gcov/llvm-cov)
- Flaky test retry (nice-to-have, not MVP)
- CDash integration
