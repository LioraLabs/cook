# CLI Progress UI Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace cook's plain-text `[recipe] line` output with a modern progress UI — progress bars, active node tracking, streaming output, cache hit display, and test summaries.

**Architecture:** An event-based system where the scheduler emits structured `ProgressEvent`s instead of writing directly to stdout. A `ProgressRenderer` trait with two implementations — `TtyRenderer` (cursor-controlled progress bars) and `PlainRenderer` (line-by-line `[recipe] line` format) — consumes these events. TTY detection and `--color` flag determine which renderer is used.

**Tech Stack:** Rust, `crossterm` (terminal manipulation + color), `mpsc` channels (event delivery)

**Spec:** `docs/superpowers/specs/2026-03-16-cli-progress-ui-design.md`

---

## File Structure

### New files:
- `src/scheduler/progress.rs` — `ProgressEvent` enum, `ProgressRenderer` trait, `PlainRenderer`, `TtyRenderer`
- `src/scheduler/color.rs` — `ColorConfig` (auto/always/never), `NO_COLOR` detection, styled write helpers

### Modified files:
- `Cargo.toml` — Add `crossterm` dependency
- `src/cli/mod.rs` — Add `--color=always/never/auto` global flag
- `src/contracts/mod.rs` — Add `display_name()` to `WorkPayload`, add per-recipe node count metadata to `ExecutionDag`
- `src/scheduler/mod.rs` — Re-export `progress` and `color` modules
- `src/scheduler/dag.rs` — Add `recipe_node_counts()` method to `ExecutionDag`
- `src/scheduler/builder.rs` — No changes needed (counts derived dynamically from DAG)
- `src/scheduler/executor.rs` — Replace `SharedWriter` with event channel, emit `ProgressEvent`s at each state transition
- `src/scheduler/pool.rs` — Workers send output lines through events instead of `SharedWriter`, include `recipe_name` + `node_name` in `WorkResult`
- `src/scheduler/output.rs` — Deprecate/remove `SharedWriter` (replaced by event channel)
- `src/engine/pipeline.rs` — Create renderer, spawn render thread, pass event sender to executor
- `src/engine/test_output.rs` — Update `format_terminal_summary()` to use color and new symbols
- `tests/integration.rs` — Add progress event and renderer tests

---

## Chunk 1: Event Model & Color Config

### Task 1: Add crossterm dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add crossterm to Cargo.toml**

Add under `[dependencies]`:

```toml
crossterm = "0.28"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: Success

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add crossterm dependency for terminal UI"
```

---

### Task 2: Create ColorConfig module

**Files:**
- Create: `src/scheduler/color.rs`
- Modify: `src/scheduler/mod.rs`

- [ ] **Step 1: Write color config tests**

Create `src/scheduler/color.rs` with tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_mode_auto_respects_no_color_env() {
        // NO_COLOR set = no color
        let config = ColorConfig::resolve(ColorMode::Auto, false, true);
        assert!(!config.enabled);
    }

    #[test]
    fn test_color_mode_always_overrides() {
        let config = ColorConfig::resolve(ColorMode::Always, false, true);
        assert!(config.enabled);
    }

    #[test]
    fn test_color_mode_never() {
        let config = ColorConfig::resolve(ColorMode::Never, true, false);
        assert!(!config.enabled);
    }

    #[test]
    fn test_color_mode_auto_tty() {
        let config = ColorConfig::resolve(ColorMode::Auto, true, false);
        assert!(config.enabled);
    }

    #[test]
    fn test_color_mode_auto_no_tty() {
        let config = ColorConfig::resolve(ColorMode::Auto, false, false);
        assert!(!config.enabled);
    }

    #[test]
    fn test_symbols() {
        let sym = Symbols::new();
        assert_eq!(sym.finished, "◆");
        assert_eq!(sym.running, "◇");
        assert_eq!(sym.waiting, "○");
        assert_eq!(sym.cache_hit, "≋");
        assert_eq!(sym.success, "✓");
        assert_eq!(sym.failure, "✗");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib color`
Expected: FAIL — module doesn't exist yet

- [ ] **Step 3: Implement ColorConfig**

Write the full `src/scheduler/color.rs`:

```rust
use crossterm::style::{Stylize, StyledContent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl std::str::FromStr for ColorMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            other => Err(format!("invalid color mode: {other}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ColorConfig {
    pub enabled: bool,
}

impl ColorConfig {
    /// Resolve color config from CLI flag, TTY status, and NO_COLOR env.
    /// `is_tty` = whether stdout is a terminal.
    /// `no_color_set` = whether the NO_COLOR env var is set (any value).
    pub fn resolve(mode: ColorMode, is_tty: bool, no_color_set: bool) -> Self {
        let enabled = match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => is_tty && !no_color_set,
        };
        Self { enabled }
    }

    pub fn green(&self, text: &str) -> String {
        if self.enabled {
            format!("{}", text.green())
        } else {
            text.to_string()
        }
    }

    pub fn red(&self, text: &str) -> String {
        if self.enabled {
            format!("{}", text.red())
        } else {
            text.to_string()
        }
    }

    pub fn blue(&self, text: &str) -> String {
        if self.enabled {
            format!("{}", text.blue())
        } else {
            text.to_string()
        }
    }

    pub fn magenta(&self, text: &str) -> String {
        if self.enabled {
            format!("{}", text.magenta())
        } else {
            text.to_string()
        }
    }

    pub fn dim(&self, text: &str) -> String {
        if self.enabled {
            format!("{}", text.dark_grey())
        } else {
            text.to_string()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        if self.enabled {
            format!("{}", text.bold())
        } else {
            text.to_string()
        }
    }
}

pub struct Symbols {
    pub finished: &'static str,
    pub running: &'static str,
    pub waiting: &'static str,
    pub cache_hit: &'static str,
    pub success: &'static str,
    pub failure: &'static str,
}

impl Symbols {
    pub fn new() -> Self {
        Self {
            finished: "◆",
            running: "◇",
            waiting: "○",
            cache_hit: "≋",
            success: "✓",
            failure: "✗",
        }
    }
}
```

- [ ] **Step 4: Add module to scheduler/mod.rs**

Add `pub mod color;` to `src/scheduler/mod.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib color`
Expected: All 6 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/scheduler/color.rs src/scheduler/mod.rs
git commit -m "feat(scheduler): add ColorConfig and Symbols for terminal output"
```

---

### Task 3: Define ProgressEvent enum and display_name for WorkPayload

**Files:**
- Create: `src/scheduler/progress.rs`
- Modify: `src/scheduler/mod.rs`
- Modify: `src/contracts/mod.rs`

- [ ] **Step 1: Add display_name() to WorkPayload**

In `src/contracts/mod.rs`, add an `impl WorkPayload` block after the enum definition (after line 27):

```rust
impl WorkPayload {
    /// Human-readable name for progress UI (Layer 2 active nodes).
    pub fn display_name(&self) -> String {
        match self {
            Self::Shell { cmd, .. } => {
                // Use first 60 chars of command, truncate with ...
                if cmd.len() <= 60 {
                    cmd.clone()
                } else {
                    format!("{}...", &cmd[..57])
                }
            }
            Self::LuaChunk { .. } => "lua".to_string(),
            Self::Test { test_name, .. } => test_name.clone(),
            Self::Interactive { cmd, .. } => cmd.clone(),
        }
    }
}
```

- [ ] **Step 2: Write display_name tests**

Add a new `#[cfg(test)] mod tests { use super::*; ... }` block at the bottom of `src/contracts/mod.rs` (there isn't one yet):

```rust
#[test]
fn test_display_name_shell() {
    let p = WorkPayload::Shell { cmd: "gcc -c foo.c".into(), line: 1 };
    assert_eq!(p.display_name(), "gcc -c foo.c");
}

#[test]
fn test_display_name_shell_truncates() {
    let long = "a".repeat(100);
    let p = WorkPayload::Shell { cmd: long, line: 1 };
    let name = p.display_name();
    assert!(name.len() <= 63);
    assert!(name.ends_with("..."));
}

#[test]
fn test_display_name_lua() {
    let p = WorkPayload::LuaChunk {
        code: "x()".into(),
        input: "i".into(),
        output: "o".into(),
        ingredient_groups: vec![],
    };
    assert_eq!(p.display_name(), "lua");
}

#[test]
fn test_display_name_test() {
    let p = WorkPayload::Test {
        cmd: "./run".into(),
        line: 1,
        timeout: 30,
        should_fail: false,
        suite_name: "s".into(),
        test_name: "test_foo".into(),
    };
    assert_eq!(p.display_name(), "test_foo");
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib display_name`
Expected: All 4 tests pass

- [ ] **Step 4: Create progress.rs with ProgressEvent enum**

Create `src/scheduler/progress.rs`:

```rust
use std::time::Duration;

/// Structured events emitted by the scheduler during DAG execution.
/// The renderer consumes these to update the display.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// A recipe is queued (known but not yet started). Shown as "waiting" in UI.
    RecipeQueued {
        name: String,
        total_nodes: usize,
    },

    /// A recipe has started executing. `total_nodes` = number of DAG nodes for this recipe.
    RecipeStarted {
        name: String,
        total_nodes: usize,
    },

    /// A recipe completed successfully.
    RecipeCompleted {
        name: String,
        elapsed: Duration,
        cached_nodes: usize,
        total_nodes: usize,
    },

    /// A recipe failed (at least one node failed).
    RecipeFailed {
        name: String,
        elapsed: Duration,
        completed_nodes: usize,
        total_nodes: usize,
    },

    /// A DAG node started executing within a recipe.
    NodeStarted {
        recipe: String,
        node_name: String,
    },

    /// A DAG node completed successfully.
    NodeCompleted {
        recipe: String,
        node_name: String,
        elapsed: Duration,
    },

    /// A DAG node failed.
    NodeFailed {
        recipe: String,
        node_name: String,
        elapsed: Duration,
        error: String,
    },

    /// A DAG node was a cache hit (skipped execution).
    NodeCacheHit {
        recipe: String,
        node_name: String,
    },

    /// A DAG node was skipped (dependency failed).
    NodeSkipped {
        recipe: String,
        node_name: String,
    },

    /// A line of output from a running node.
    OutputLine {
        recipe: String,
        line: String,
        is_stderr: bool,
    },

    /// An interactive step is about to start — clear the UI.
    InteractiveStart {
        recipe: String,
    },

    /// An interactive step finished — redraw the UI.
    InteractiveEnd {
        recipe: String,
        elapsed: Duration,
        success: bool,
    },

    /// A test result was recorded.
    TestResult {
        suite: String,
        test_name: String,
        passed: bool,
        elapsed: Duration,
        output: String,
    },

    /// All DAG execution is complete.
    Finished,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_event_is_send() {
        // ProgressEvent must be Send so it can cross thread boundaries via mpsc.
        fn assert_send<T: Send>() {}
        assert_send::<ProgressEvent>();
    }

    #[test]
    fn test_progress_event_clone() {
        let evt = ProgressEvent::RecipeStarted {
            name: "lib".into(),
            total_nodes: 5,
        };
        let cloned = evt.clone();
        if let ProgressEvent::RecipeStarted { name, total_nodes } = cloned {
            assert_eq!(name, "lib");
            assert_eq!(total_nodes, 5);
        } else {
            panic!("wrong variant");
        }
    }
}
```

- [ ] **Step 5: Add module to scheduler/mod.rs**

Add `pub mod progress;` to `src/scheduler/mod.rs`.

- [ ] **Step 6: Run all tests**

Run: `cargo test --lib progress`
Expected: Both tests pass

- [ ] **Step 7: Commit**

```bash
git add src/contracts/mod.rs src/scheduler/progress.rs src/scheduler/mod.rs
git commit -m "feat(scheduler): add ProgressEvent enum and WorkPayload::display_name()"
```

---

### Task 4: Add recipe_node_counts to ExecutionDag

**Files:**
- Modify: `src/scheduler/dag.rs`
- Modify: `src/scheduler/builder.rs`

- [ ] **Step 1: Write tests for recipe_node_counts**

Add to `src/scheduler/dag.rs` test module:

```rust
#[test]
fn test_recipe_node_counts() {
    let mut dag = ExecutionDag::new();
    dag.add_node(
        WorkPayload::Shell { cmd: "a".into(), line: 1 },
        "lib".into(),
        None,
        &[],
    );
    dag.add_node(
        WorkPayload::Shell { cmd: "b".into(), line: 2 },
        "lib".into(),
        None,
        &[0],
    );
    dag.add_node(
        WorkPayload::Shell { cmd: "c".into(), line: 3 },
        "app".into(),
        None,
        &[1],
    );
    let counts = dag.recipe_node_counts();
    assert_eq!(counts.get("lib"), Some(&2));
    assert_eq!(counts.get("app"), Some(&1));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib recipe_node_counts`
Expected: FAIL — method doesn't exist

- [ ] **Step 3: Implement recipe_node_counts**

Add to the `impl ExecutionDag` block in `src/scheduler/dag.rs`:

```rust
/// Returns a map of recipe_name -> number of DAG nodes for that recipe.
pub fn recipe_node_counts(&self) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for node in &self.nodes {
        *counts.entry(node.recipe_name.clone()).or_insert(0) += 1;
    }
    counts
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib recipe_node_counts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/scheduler/dag.rs
git commit -m "feat(dag): add recipe_node_counts() for progress bar denominators"
```

---

## Chunk 2: PlainRenderer

### Task 5: Define ProgressRenderer trait and PlainRenderer

**Files:**
- Modify: `src/scheduler/progress.rs`

- [ ] **Step 1: Write PlainRenderer tests**

Add to `src/scheduler/progress.rs` test module:

```rust
#[test]
fn test_plain_renderer_recipe_started() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let color = ColorConfig::resolve(ColorMode::Never, false, false);
    let mut r = PlainRenderer::new(buf.clone(), color);
    r.handle(ProgressEvent::RecipeStarted {
        name: "lib".into(),
        total_nodes: 3,
    });
    let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(output.is_empty()); // recipe start is silent in plain mode
}

#[test]
fn test_plain_renderer_output_line() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let color = ColorConfig::resolve(ColorMode::Never, false, false);
    let mut r = PlainRenderer::new(buf.clone(), color);
    r.handle(ProgressEvent::OutputLine {
        recipe: "lib".into(),
        line: "gcc -c foo.c".into(),
        is_stderr: false,
    });
    let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert_eq!(output, "[lib] gcc -c foo.c\n");
}

#[test]
fn test_plain_renderer_recipe_completed() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let color = ColorConfig::resolve(ColorMode::Never, false, false);
    let mut r = PlainRenderer::new(buf.clone(), color);
    r.handle(ProgressEvent::RecipeCompleted {
        name: "lib".into(),
        elapsed: Duration::from_millis(1400),
        cached_nodes: 2,
        total_nodes: 6,
    });
    let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(output.contains("[lib] done (1.4s, 2/6 cached)"));
}

#[test]
fn test_plain_renderer_recipe_completed_no_cache() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let color = ColorConfig::resolve(ColorMode::Never, false, false);
    let mut r = PlainRenderer::new(buf.clone(), color);
    r.handle(ProgressEvent::RecipeCompleted {
        name: "app".into(),
        elapsed: Duration::from_millis(200),
        cached_nodes: 0,
        total_nodes: 1,
    });
    let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert_eq!(output, "[app] done (0.2s)\n");
}

#[test]
fn test_plain_renderer_node_cache_hit() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let color = ColorConfig::resolve(ColorMode::Never, false, false);
    let mut r = PlainRenderer::new(buf.clone(), color);
    r.handle(ProgressEvent::NodeCacheHit {
        recipe: "lib".into(),
        node_name: "compile a.c".into(),
    });
    let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(output.is_empty()); // cache hits silent in plain mode for nodes
}

#[test]
fn test_plain_renderer_recipe_completed_all_cached() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let color = ColorConfig::resolve(ColorMode::Never, false, false);
    let mut r = PlainRenderer::new(buf.clone(), color);
    r.handle(ProgressEvent::RecipeCompleted {
        name: "lib".into(),
        elapsed: Duration::from_millis(0),
        cached_nodes: 3,
        total_nodes: 3,
    });
    let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(output.contains("[lib] done (0.0s, cached)"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib plain_renderer`
Expected: FAIL — structs/methods don't exist

- [ ] **Step 3: Implement ProgressRenderer trait and PlainRenderer**

Add to `src/scheduler/progress.rs` (above the test module), keeping the existing `ProgressEvent` enum and imports:

```rust
use std::io::Write;
use std::sync::{Arc, Mutex};
use crate::scheduler::color::{ColorConfig, ColorMode};

/// Trait for consuming progress events and rendering output.
pub trait ProgressRenderer: Send {
    fn handle(&mut self, event: ProgressEvent);
}

/// Plain text renderer for non-TTY output.
/// Emits `[recipe] line` format, no cursor manipulation.
pub struct PlainRenderer {
    out: Arc<Mutex<Vec<u8>>>,
    color: ColorConfig,
}

impl PlainRenderer {
    pub fn new(out: Arc<Mutex<Vec<u8>>>, color: ColorConfig) -> Self {
        Self { out, color }
    }

    /// Drain the internal buffer, returning its contents. Caller writes to stderr/stdout.
    pub fn drain(&self) -> Vec<u8> {
        let mut buf = self.out.lock().unwrap();
        let data = buf.clone();
        buf.clear();
        data
    }

    fn write(&self, s: &str) {
        let mut buf = self.out.lock().unwrap();
        let _ = buf.write_all(s.as_bytes());
    }
}

impl ProgressRenderer for PlainRenderer {
    fn handle(&mut self, event: ProgressEvent) {
        match event {
            ProgressEvent::RecipeQueued { .. } => {
                // Silent in plain mode
            }
            ProgressEvent::RecipeStarted { .. } => {
                // Silent in plain mode
            }
            ProgressEvent::OutputLine { recipe, line, .. } => {
                self.write(&format!("[{recipe}] {line}\n"));
            }
            ProgressEvent::RecipeCompleted {
                name,
                elapsed,
                cached_nodes,
                total_nodes,
            } => {
                let secs = elapsed.as_secs_f64();
                let cache_info = if cached_nodes == total_nodes && total_nodes > 0 {
                    ", cached".to_string()
                } else if cached_nodes > 0 {
                    format!(", {cached_nodes}/{total_nodes} cached")
                } else {
                    String::new()
                };
                self.write(&format!("[{name}] done ({secs:.1}s{cache_info})\n"));
            }
            ProgressEvent::RecipeFailed {
                name,
                elapsed,
                completed_nodes,
                total_nodes,
            } => {
                let secs = elapsed.as_secs_f64();
                self.write(&format!(
                    "[{name}] FAILED ({completed_nodes}/{total_nodes} steps, {secs:.1}s)\n"
                ));
            }
            ProgressEvent::NodeStarted { .. } => {
                // Silent in plain mode — output lines carry the info
            }
            ProgressEvent::NodeCompleted { .. } => {
                // Silent in plain mode
            }
            ProgressEvent::NodeFailed {
                recipe,
                error,
                ..
            } => {
                self.write(&format!("[{recipe}] {error}\n"));
            }
            ProgressEvent::NodeCacheHit { .. } => {
                // Silent in plain mode
            }
            ProgressEvent::NodeSkipped { .. } => {
                // Silent in plain mode
            }
            ProgressEvent::InteractiveStart { .. } => {
                // Nothing to clear in plain mode
            }
            ProgressEvent::InteractiveEnd { .. } => {
                // Nothing to redraw in plain mode
            }
            ProgressEvent::TestResult { .. } => {
                // Test results handled separately by test summary
            }
            ProgressEvent::Finished => {
                // No footer in plain mode
            }
        }
    }
}
```

- [ ] **Step 4: Add imports at top of progress.rs**

Make sure the imports section at the top of `progress.rs` includes:

```rust
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::io::Write;
use crate::scheduler::color::{ColorConfig, ColorMode};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib plain_renderer`
Expected: All 6 tests pass

- [ ] **Step 6: Run all existing tests to check for regressions**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 7: Commit**

```bash
git add src/scheduler/progress.rs
git commit -m "feat(scheduler): add ProgressRenderer trait and PlainRenderer"
```

---

## Chunk 3: Wire Events Into Executor

### Task 6: Add event sender to executor and pool

This is the core integration — replace `SharedWriter` usage in the executor with `ProgressEvent` sends. Workers still capture output, but instead of writing through `SharedWriter`, results are sent as events.

**Files:**
- Modify: `src/scheduler/executor.rs`
- Modify: `src/scheduler/pool.rs`
- Modify: `src/scheduler/output.rs`

- [ ] **Step 1: Add node_name field to WorkResult**

In `src/scheduler/pool.rs`, add `node_name: String` to the `WorkResult` struct (around line 30):

```rust
pub struct WorkResult {
    pub id: usize,
    pub success: bool,
    pub error: Option<String>,
    pub test_output: Option<TestOutput>,
    pub node_name: String,
    pub output_lines: Vec<String>,
}
```

- [ ] **Step 2: Update workers to collect output lines instead of writing through SharedWriter**

In `src/scheduler/pool.rs`, update `execute_shell()` to collect stdout/stderr lines into a `Vec<String>` in addition to writing through `SharedWriter`. This dual-write approach avoids breaking existing behavior while enabling event emission.

Specifically:
1. Add a `let mut output_lines: Vec<String> = Vec::new();` at the top of `execute_shell`.
2. After each `writer.write_stdout_line(recipe_name, &l)` call, also push: `output_lines.push(l.clone());`
3. Same for stderr lines.
4. Populate `WorkResult::output_lines` from this vec.
5. Compute `WorkResult::node_name` by calling `payload.display_name()` before moving the payload into the match arms. Store it early: `let node_name = item.payload.display_name();`

Do the same for `execute_test` (capture output lines) and `execute_lua_chunk` (empty vec is fine — Lua chunks don't produce line output through the shell).

Apply the same pattern in `execute_work_item`: compute `node_name` from the payload before matching, pass it through to the result.

- [ ] **Step 3: Update execute_dag signature to accept event sender**

In `src/scheduler/executor.rs`, add an `event_tx` parameter to `execute_dag` while keeping all other parameters unchanged (owned types stay owned to avoid breaking existing callers in tests):

```rust
pub fn execute_dag(
    dag: ExecutionDag,
    num_workers: usize,
    working_dir: PathBuf,
    env_vars: HashMap<String, String>,
    _quiet: bool,
    cache_manager: Option<Arc<ThreadSafeCacheManager>>,
    event_tx: Option<mpsc::Sender<ProgressEvent>>,
) -> Result<(), SchedulerError>
```

**Important:** Also update all call sites in `src/scheduler/tests.rs` to pass `None` as the last argument.

- [ ] **Step 4: Emit events from executor main loop**

In the `execute_dag` main loop, emit events at each transition point:

- Before the recipe execution loop in pipeline.rs, emit `RecipeQueued` for all recipes in the execution order (so the renderer shows them as "waiting")
- When a node is submitted to the pool: `NodeStarted { recipe, node_name }`
- When `WorkResult` received with success: `NodeCompleted { recipe, node_name, elapsed }`
- When `WorkResult` received with failure: `NodeFailed { recipe, node_name, elapsed, error }`
- When a presatisfied (cached) node is processed: `NodeCacheHit { recipe, node_name }`
- When a node is cancelled: `NodeSkipped { recipe, node_name }`
- For output lines from `WorkResult::output_lines`: `OutputLine { recipe, line }`
- Before interactive step: `InteractiveStart { recipe }`
- After interactive step: `InteractiveEnd { recipe, elapsed, success }`

Helper:

```rust
fn emit(tx: &Option<mpsc::Sender<ProgressEvent>>, event: ProgressEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}
```

- [ ] **Step 5: Track per-recipe state for RecipeStarted/Completed/Failed events**

Add tracking state inside `execute_dag`:

```rust
struct RecipeState {
    start: std::time::Instant,
    total_nodes: usize,
    completed_nodes: usize,
    cached_nodes: usize,
    failed: bool,
}
let mut recipe_states: HashMap<String, RecipeState> = HashMap::new();
```

Initialize from `dag.recipe_node_counts()`. Emit `RecipeStarted` when first node of a recipe is processed. Emit `RecipeCompleted`/`RecipeFailed` when last node of a recipe finishes (or is cancelled/skipped).

- [ ] **Step 6: Update all callers of execute_dag**

In `src/engine/pipeline.rs`, update `cmd_run` and `cmd_test` calls to pass `None` for `event_tx` (preserving existing behavior for now):

```rust
execute_dag(dag, num_jobs, &cookfile_dir, &env_vars, quiet, cache_manager.as_ref(), None)?;
```

- [ ] **Step 7: Run all tests**

Run: `cargo test`
Expected: All tests pass (no behavior change yet — `event_tx` is `None`)

- [ ] **Step 8: Commit**

```bash
git add src/scheduler/executor.rs src/scheduler/pool.rs src/engine/pipeline.rs
git commit -m "feat(scheduler): emit ProgressEvent from executor main loop"
```

---

### Task 7: Wire PlainRenderer into pipeline

**Files:**
- Modify: `src/engine/pipeline.rs`
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Add --color flag to CLI**

In `src/cli/mod.rs`, add to the `Cli` struct:

```rust
/// Color output mode
#[arg(long, default_value = "auto")]
pub color: String,
```

**Important:** Since the `External` subcommand variant uses `external_subcommand` which consumes unknown args, verify that `--color` as a global flag on `Cli` is parsed before the subcommand. Test with `cook build hello --color=never` and `cook --color=never build hello` to ensure both work. If the flag gets consumed by `External`, move it to be parsed earlier or add it to the subcommand level too.

- [ ] **Step 2: Pass color mode through pipeline**

In `src/engine/pipeline.rs`, at the start of `cmd_run` and `cmd_test`:

```rust
use crate::scheduler::color::{ColorConfig, ColorMode};
use crate::scheduler::progress::{PlainRenderer, ProgressRenderer, ProgressEvent};
use std::sync::mpsc;

let color_mode: ColorMode = cli.color.parse().unwrap_or(ColorMode::Auto);
let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
let no_color = std::env::var("NO_COLOR").is_ok();
let color = ColorConfig::resolve(color_mode, is_tty, no_color);
```

- [ ] **Step 3: Create event channel and spawn renderer thread**

In `cmd_run`, before the recipe execution loop:

```rust
let (event_tx, event_rx) = mpsc::channel::<ProgressEvent>();

// For now, always use PlainRenderer (TtyRenderer comes later)
let render_thread = std::thread::spawn(move || {
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let mut renderer = PlainRenderer::new(buf.clone(), color.clone());
    while let Ok(event) = event_rx.recv() {
        let is_finished = matches!(event, ProgressEvent::Finished);
        renderer.handle(event);
        // Flush buf to stderr
        let mut out = buf.lock().unwrap();
        if !out.is_empty() {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), &out);
            out.clear();
        }
        if is_finished {
            break;
        }
    }
});
```

- [ ] **Step 4: Pass event_tx to execute_dag**

Change the `execute_dag` call to pass `Some(event_tx.clone())` and send `Finished` after the loop:

```rust
execute_dag(dag, num_jobs, &cookfile_dir, &env_vars, quiet, cache_manager.as_ref(), Some(event_tx.clone()))?;
```

After all recipes complete:
```rust
let _ = event_tx.send(ProgressEvent::Finished);
drop(event_tx);
let _ = render_thread.join();
```

- [ ] **Step 5: Run integration tests**

Run: `cargo test`
Expected: All tests pass. Output should now go through PlainRenderer (same `[recipe] line` format).

- [ ] **Step 6: Manual smoke test**

Run: `cargo build && ./target/debug/cook build` (in a project with a Cookfile)
Expected: Same output as before, through PlainRenderer pipeline.

- [ ] **Step 7: Commit**

```bash
git add src/cli/mod.rs src/engine/pipeline.rs
git commit -m "feat(pipeline): wire PlainRenderer through event channel"
```

---

## Chunk 4: TtyRenderer

### Task 8: Implement TtyRenderer with progress bars

This is the core visual task — the cursor-controlled progress UI.

**Files:**
- Modify: `src/scheduler/progress.rs`

- [ ] **Step 1: Write TtyRenderer state tests**

Add to progress.rs test module:

```rust
#[test]
fn test_tty_renderer_state_tracking() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let color = ColorConfig::resolve(ColorMode::Always, true, false);
    let mut r = TtyRenderer::new_with_buffer(buf.clone(), color, 80);

    r.handle(ProgressEvent::RecipeStarted {
        name: "lib".into(),
        total_nodes: 3,
    });
    assert_eq!(r.recipes.get("lib").unwrap().total_nodes, 3);
    assert_eq!(r.recipes.get("lib").unwrap().completed_nodes, 0);

    r.handle(ProgressEvent::NodeStarted {
        recipe: "lib".into(),
        node_name: "compile a.c".into(),
    });
    assert!(r.recipes.get("lib").unwrap().active_nodes.contains(&"compile a.c".to_string()));

    r.handle(ProgressEvent::NodeCompleted {
        recipe: "lib".into(),
        node_name: "compile a.c".into(),
        elapsed: Duration::from_millis(100),
    });
    assert!(!r.recipes.get("lib").unwrap().active_nodes.contains(&"compile a.c".to_string()));
    assert_eq!(r.recipes.get("lib").unwrap().completed_nodes, 1);
}

#[test]
fn test_tty_renderer_cache_hit_tracking() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let color = ColorConfig::resolve(ColorMode::Always, true, false);
    let mut r = TtyRenderer::new_with_buffer(buf.clone(), color, 80);

    r.handle(ProgressEvent::RecipeStarted {
        name: "lib".into(),
        total_nodes: 3,
    });
    r.handle(ProgressEvent::NodeCacheHit {
        recipe: "lib".into(),
        node_name: "compile a.c".into(),
    });

    let state = r.recipes.get("lib").unwrap();
    assert_eq!(state.completed_nodes, 1);
    assert_eq!(state.cached_nodes, 1);
    assert!(state.cached_node_names.contains("compile a.c"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib tty_renderer`
Expected: FAIL — TtyRenderer doesn't exist yet

- [ ] **Step 3: Implement TtyRenderer struct and state tracking**

Add to `src/scheduler/progress.rs`:

```rust
use std::collections::{BTreeMap, BTreeSet};
use crossterm::{cursor, terminal, ExecutableCommand, QueueableCommand};

#[derive(Debug)]
pub struct RecipeRenderState {
    pub total_nodes: usize,
    pub completed_nodes: usize,
    pub cached_nodes: usize,
    pub failed: bool,
    pub finished: bool,
    pub start: std::time::Instant,
    pub active_nodes: Vec<String>,
    pub cached_node_names: BTreeSet<String>,
    pub completed_node_names: BTreeMap<String, bool>, // name -> success
    pub skipped_node_names: BTreeSet<String>,
    pub last_output_lines: Vec<String>,
    pub error_output: Vec<String>,
}

impl RecipeRenderState {
    fn new(total_nodes: usize) -> Self {
        Self {
            total_nodes,
            completed_nodes: 0,
            cached_nodes: 0,
            failed: false,
            finished: false,
            start: std::time::Instant::now(),
            active_nodes: Vec::new(),
            cached_node_names: BTreeSet::new(),
            completed_node_names: BTreeMap::new(),
            skipped_node_names: BTreeSet::new(),
            last_output_lines: Vec::new(),
            error_output: Vec::new(),
        }
    }
}

pub struct TtyRenderer {
    out: Arc<Mutex<Vec<u8>>>,
    color: ColorConfig,
    term_width: u16,
    pub recipes: BTreeMap<String, RecipeRenderState>,
    recipe_order: Vec<String>,
    lines_drawn: usize,
    total_recipes: usize,
    finished_recipes: usize,
    running_recipes: usize,
    waiting_recipes: usize,
    total_cached: usize,
    total_nodes: usize,
}

impl TtyRenderer {
    pub fn new(color: ColorConfig) -> Self {
        let (width, _) = terminal::size().unwrap_or((80, 24));
        Self {
            out: Arc::new(Mutex::new(Vec::new())),
            color,
            term_width: width,
            recipes: BTreeMap::new(),
            recipe_order: Vec::new(),
            lines_drawn: 0,
            total_recipes: 0,
            finished_recipes: 0,
            running_recipes: 0,
            waiting_recipes: 0,
            total_cached: 0,
            total_nodes: 0,
        }
    }

    /// Constructor for testing — writes to buffer instead of stdout.
    pub fn new_with_buffer(out: Arc<Mutex<Vec<u8>>>, color: ColorConfig, width: u16) -> Self {
        Self {
            out,
            color,
            term_width: width,
            recipes: BTreeMap::new(),
            recipe_order: Vec::new(),
            lines_drawn: 0,
            total_recipes: 0,
            finished_recipes: 0,
            running_recipes: 0,
            waiting_recipes: 0,
            total_cached: 0,
            total_nodes: 0,
        }
    }

    fn update_state(&mut self, event: &ProgressEvent) {
        match event {
            ProgressEvent::RecipeQueued { name, total_nodes } => {
                if !self.recipes.contains_key(name) {
                    self.recipes.insert(name.clone(), RecipeRenderState::new(*total_nodes));
                    self.recipe_order.push(name.clone());
                    self.total_recipes += 1;
                    self.waiting_recipes += 1;
                    self.total_nodes += total_nodes;
                }
            }
            ProgressEvent::RecipeStarted { name, total_nodes } => {
                if !self.recipes.contains_key(name) {
                    self.recipes.insert(name.clone(), RecipeRenderState::new(*total_nodes));
                    self.recipe_order.push(name.clone());
                    self.total_recipes += 1;
                    self.total_nodes += total_nodes;
                }
                // Transition from queued/waiting to running
                self.waiting_recipes = self.waiting_recipes.saturating_sub(1);
                self.running_recipes += 1;
            }
            ProgressEvent::NodeStarted { recipe, node_name } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.active_nodes.push(node_name.clone());
                }
            }
            ProgressEvent::NodeCompleted { recipe, node_name, .. } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.active_nodes.retain(|n| n != node_name);
                    state.completed_nodes += 1;
                    state.completed_node_names.insert(node_name.clone(), true);
                }
            }
            ProgressEvent::NodeFailed { recipe, node_name, error, .. } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.active_nodes.retain(|n| n != node_name);
                    state.completed_nodes += 1;
                    state.failed = true;
                    state.completed_node_names.insert(node_name.clone(), false);
                    state.error_output.push(error.clone());
                }
            }
            ProgressEvent::NodeCacheHit { recipe, node_name } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.completed_nodes += 1;
                    state.cached_nodes += 1;
                    state.cached_node_names.insert(node_name.clone());
                    self.total_cached += 1;
                }
            }
            ProgressEvent::NodeSkipped { recipe, node_name } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.completed_nodes += 1;
                    state.skipped_node_names.insert(node_name.clone());
                }
            }
            ProgressEvent::OutputLine { recipe, line, .. } => {
                if let Some(state) = self.recipes.get_mut(recipe) {
                    state.last_output_lines.push(line.clone());
                    if state.last_output_lines.len() > 2 {
                        state.last_output_lines.remove(0);
                    }
                }
            }
            ProgressEvent::RecipeCompleted { name, .. } => {
                if let Some(state) = self.recipes.get_mut(name) {
                    state.finished = true;
                    state.active_nodes.clear();
                    state.last_output_lines.clear();
                    self.finished_recipes += 1;
                    self.running_recipes = self.running_recipes.saturating_sub(1);
                }
            }
            ProgressEvent::RecipeFailed { name, .. } => {
                if let Some(state) = self.recipes.get_mut(name) {
                    state.finished = true;
                    state.failed = true;
                    self.finished_recipes += 1;
                    self.running_recipes = self.running_recipes.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    fn render(&mut self) {
        // Re-query terminal size to handle resize
        if let Ok((w, _)) = terminal::size() {
            self.term_width = w;
        }

        // Clear previously drawn lines
        let mut out = self.out.lock().unwrap();
        if self.lines_drawn > 0 {
            for _ in 0..self.lines_drawn {
                let _ = out.write_all(b"\x1b[A\x1b[2K");
            }
        }

        let mut lines = Vec::new();
        let sym = crate::scheduler::color::Symbols::new();

        for name in &self.recipe_order {
            let state = &self.recipes[name];
            if state.finished && !state.failed {
                // Collapsed success line
                let elapsed = state.start.elapsed().as_secs_f64();
                let cache_info = if state.cached_nodes == state.total_nodes && state.total_nodes > 0 {
                    " (cached)".to_string()
                } else if state.cached_nodes > 0 {
                    format!(" ({}/{} cached)", state.cached_nodes, state.total_nodes)
                } else {
                    String::new()
                };
                lines.push(format!(
                    "{} {} {} {:.1}s {}{}",
                    self.color.green(sym.finished),
                    self.color.bold(name),
                    self.render_bar(state.total_nodes, state.total_nodes, false),
                    elapsed,
                    self.color.green(sym.success),
                    self.color.dim(&cache_info),
                ));
            } else if state.finished && state.failed {
                // Expanded failure
                let n = state.completed_nodes;
                let t = state.total_nodes;
                lines.push(format!(
                    "{} {} {} {}/{} {}",
                    self.color.red(sym.finished),
                    self.color.bold(name),
                    self.render_bar(n, t, true),
                    n, t,
                    self.color.red(sym.failure),
                ));
                // Layer 2: node statuses
                let mut node_line = String::from("  ");
                for (nname, success) in &state.completed_node_names {
                    if state.cached_node_names.contains(nname) {
                        node_line.push_str(&format!("{} {} ", self.color.dim(sym.cache_hit), self.color.dim(nname)));
                    } else if *success {
                        node_line.push_str(&format!("{} {} {} ", self.color.green(sym.finished), nname, self.color.green(sym.success)));
                    } else {
                        node_line.push_str(&format!("{} {} {} ", self.color.red(sym.finished), nname, self.color.red(sym.failure)));
                    }
                }
                for sname in &state.skipped_node_names {
                    node_line.push_str(&format!("{} {} ", self.color.dim(sym.waiting), self.color.dim(&format!("{sname} skipped"))));
                }
                lines.push(node_line);
                // Layer 3: error output
                for err in &state.error_output {
                    for eline in err.lines() {
                        lines.push(format!("  {} {}", self.color.red("│"), eline));
                    }
                }
            } else if state.active_nodes.is_empty() && state.completed_nodes == 0 {
                // Waiting
                lines.push(format!(
                    "{} {} {} {}",
                    self.color.dim(sym.waiting),
                    self.color.dim(name),
                    self.render_bar(0, state.total_nodes, false),
                    self.color.dim("waiting"),
                ));
            } else {
                // Running — full 3-layer display
                let n = state.completed_nodes;
                let t = state.total_nodes;
                let elapsed = state.start.elapsed().as_secs_f64();
                lines.push(format!(
                    "{} {} {} {}/{} · {:.1}s",
                    self.color.blue(sym.running),
                    self.color.bold(name),
                    self.render_bar(n, t, false),
                    n, t, elapsed,
                ));
                // Layer 2: active nodes + cached
                if !state.active_nodes.is_empty() || !state.cached_node_names.is_empty() {
                    let mut node_line = String::from("  ");
                    for cname in &state.cached_node_names {
                        node_line.push_str(&format!("{} {} ", self.color.dim(sym.cache_hit), self.color.dim(cname)));
                    }
                    for (nname, success) in &state.completed_node_names {
                        if !state.cached_node_names.contains(nname) {
                            if *success {
                                node_line.push_str(&format!("{} {} {} ", self.color.green(sym.finished), nname, self.color.green(sym.success)));
                            }
                        }
                    }
                    for aname in &state.active_nodes {
                        node_line.push_str(&format!("{} {} ", self.color.magenta(sym.running), aname));
                    }
                    lines.push(node_line);
                }
                // Layer 3: last output lines
                for oline in &state.last_output_lines {
                    lines.push(format!("  {}", self.color.dim(oline)));
                }
            }
        }

        // Status footer
        let footer = self.render_footer();
        if !footer.is_empty() {
            lines.push(String::new());
            lines.push(footer);
        }

        for line in &lines {
            let _ = out.write_all(line.as_bytes());
            let _ = out.write_all(b"\n");
        }
        self.lines_drawn = lines.len();
    }

    fn render_bar(&self, completed: usize, total: usize, failed: bool) -> String {
        let bar_width = 20usize;
        let filled = if total > 0 {
            (completed * bar_width) / total
        } else {
            bar_width
        };
        let empty = bar_width - filled;
        let filled_str = "━".repeat(filled);
        let empty_str = "━".repeat(empty);
        if failed {
            format!("{}{}", self.color.dim(&filled_str), self.color.dim(&empty_str))
        } else {
            format!("{}{}", self.color.green(&filled_str), self.color.dim(&empty_str))
        }
    }

    fn render_footer(&self) -> String {
        let mut parts = vec![format!("{} recipes", self.total_recipes)];
        if self.finished_recipes > 0 {
            parts.push(self.color.green(&format!("{} done", self.finished_recipes)));
        }
        if self.running_recipes > 0 {
            parts.push(self.color.blue(&format!("{} running", self.running_recipes)));
        }
        let waiting = self.total_recipes - self.finished_recipes - self.running_recipes;
        if waiting > 0 {
            parts.push(format!("{waiting} waiting"));
        }
        if self.total_cached > 0 {
            parts.push(format!("{}/{} cached", self.total_cached, self.total_nodes));
        }
        let sep = self.color.dim(" │ ");
        parts.join(&sep)
    }

    fn clear_display(&mut self) {
        let mut out = self.out.lock().unwrap();
        if self.lines_drawn > 0 {
            for _ in 0..self.lines_drawn {
                let _ = out.write_all(b"\x1b[A\x1b[2K");
            }
            self.lines_drawn = 0;
        }
    }

    /// Drain the internal buffer, returning its contents. Caller writes to stderr.
    pub fn drain(&self) -> Vec<u8> {
        let mut buf = self.out.lock().unwrap();
        let data = buf.clone();
        buf.clear();
        data
    }
}

impl ProgressRenderer for TtyRenderer {
    fn handle(&mut self, event: ProgressEvent) {
        let is_finished = matches!(event, ProgressEvent::Finished);
        let is_interactive_start = matches!(event, ProgressEvent::InteractiveStart { .. });
        let is_interactive_end = matches!(event, ProgressEvent::InteractiveEnd { .. });

        self.update_state(&event);

        if is_interactive_start {
            self.clear_display();
            return;
        }

        if is_finished {
            self.render(); // Final render
            return;
        }

        self.render();
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib tty_renderer`
Expected: Both tests pass

- [ ] **Step 5: Run full test suite**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add src/scheduler/progress.rs
git commit -m "feat(scheduler): implement TtyRenderer with progress bars and state tracking"
```

---

### Task 9: Switch pipeline to use TtyRenderer when TTY detected

**Files:**
- Modify: `src/engine/pipeline.rs`

- [ ] **Step 1: Update renderer selection in cmd_run**

In the render thread setup, switch based on TTY detection:

```rust
let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
let render_color = color.clone();
let render_thread = std::thread::spawn(move || {
    if is_tty {
        let mut renderer = TtyRenderer::new(render_color);
        while let Ok(event) = event_rx.recv() {
            let is_finished = matches!(event, ProgressEvent::Finished);
            renderer.handle(event);
            let data = renderer.drain();
            if !data.is_empty() {
                let _ = std::io::Write::write_all(&mut std::io::stderr(), &data);
            }
            if is_finished { break; }
        }
    } else {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let mut renderer = PlainRenderer::new(buf.clone(), render_color);
        while let Ok(event) = event_rx.recv() {
            let is_finished = matches!(event, ProgressEvent::Finished);
            renderer.handle(event);
            let data = renderer.drain();
            if !data.is_empty() {
                let _ = std::io::Write::write_all(&mut std::io::stderr(), &data);
            }
            if is_finished { break; }
        }
    }
});
```

- [ ] **Step 2: Apply same changes to cmd_test**

Mirror the renderer setup in `cmd_test`.

- [ ] **Step 3: Run all tests**

Run: `cargo test`
Expected: All pass

- [ ] **Step 4: Manual smoke test — TTY mode**

Run: `cargo build && ./target/debug/cook build` in a terminal
Expected: Progress bars visible with symbols

- [ ] **Step 5: Manual smoke test — piped mode**

Run: `./target/debug/cook build 2>&1 | cat`
Expected: Plain `[recipe] line` format

- [ ] **Step 6: Commit**

```bash
git add src/engine/pipeline.rs
git commit -m "feat(pipeline): select TtyRenderer vs PlainRenderer based on TTY detection"
```

---

## Chunk 5: Test Output & Interactive Handling

### Task 10: Update test terminal summary with new symbols and colors

**Files:**
- Modify: `src/engine/test_output.rs`

- [ ] **Step 1: Update format_terminal_summary to use ColorConfig and Symbols**

Change the function signature to accept a `ColorConfig` parameter:

```rust
pub fn format_terminal_summary(results: &TestResults, color: &ColorConfig) -> String
```

Update the formatting to use the new symbols and colors per the spec's test output design.

- [ ] **Step 2: Update callers in pipeline.rs**

Pass the `ColorConfig` to `format_terminal_summary`.

- [ ] **Step 3: Update existing tests**

Update any tests that call `format_terminal_summary` to pass a `ColorConfig`.

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add src/engine/test_output.rs src/engine/pipeline.rs
git commit -m "feat(test-output): add colored symbols to terminal test summary"
```

---

### Task 11: Handle interactive steps in TtyRenderer

**Files:**
- Modify: `src/scheduler/executor.rs`

- [ ] **Step 1: Emit InteractiveStart/End events around interactive execution**

In the interactive batch section of `execute_dag`, wrap the interactive execution:

```rust
emit(&event_tx, ProgressEvent::InteractiveStart {
    recipe: recipe_name.clone(),
});
let result = run_interactive_on_main(cmd, line, working_dir, env_vars);
let elapsed = start.elapsed();
emit(&event_tx, ProgressEvent::InteractiveEnd {
    recipe: recipe_name.clone(),
    elapsed,
    success: result.is_ok(),
});
```

The TtyRenderer already handles `InteractiveStart` by calling `clear_display()` and `InteractiveEnd` triggers a re-render.

- [ ] **Step 2: Run tests**

Run: `cargo test`
Expected: All pass

- [ ] **Step 3: Commit**

```bash
git add src/scheduler/executor.rs
git commit -m "feat(scheduler): emit interactive start/end events for UI handoff"
```

---

## Chunk 6: Remove SharedWriter & Integration Tests

### Task 12: Remove SharedWriter dependency from pool workers

**Files:**
- Modify: `src/scheduler/pool.rs`
- Modify: `src/scheduler/output.rs`
- Modify: `src/scheduler/executor.rs`

- [ ] **Step 1: Update WorkerPool to not require SharedWriter**

Remove the `writer: SharedWriter` parameter from `WorkerPool::spawn()`. Workers now only collect output into `WorkResult::output_lines` and don't write to stdout/stderr directly.

- [ ] **Step 2: Remove SharedWriter usage from execute_shell and execute_test**

In `execute_shell`, remove the `writer.write_stdout_line` / `writer.write_stderr_line` calls. Accumulate lines in the `output_lines` vec instead.

- [ ] **Step 3: Clean up output.rs**

Remove `SharedWriter` struct (or mark deprecated). Keep `PrefixedWriter` if it's used elsewhere for buffering, otherwise remove it too.

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add src/scheduler/pool.rs src/scheduler/output.rs src/scheduler/executor.rs
git commit -m "refactor(scheduler): remove SharedWriter, workers collect output in WorkResult"
```

---

### Task 13: Add integration tests for progress events

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: Write test for plain mode output format**

```rust
#[test]
fn test_cook_build_plain_output() {
    // Run cook build with --color=never and verify [recipe] line format
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cookfile"), r#"
recipe hello {
    $ echo "hello world"
}
"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cook"))
        .args(["build", "hello", "--color=never"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[hello]"));
    assert!(stderr.contains("hello world"));
}
```

- [ ] **Step 2: Write test for --color flag parsing**

```rust
#[test]
fn test_cook_color_flag_always() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cookfile"), r#"
recipe hello {
    $ echo "hi"
}
"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cook"))
        .args(["build", "hello", "--color=always"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
}
```

- [ ] **Step 3: Run integration tests**

Run: `cargo test --test integration`
Expected: All pass

- [ ] **Step 4: Commit**

```bash
git add tests/integration.rs
git commit -m "test: add integration tests for progress UI output modes"
```

---

### Task 14: Final verification and cleanup

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All unit and integration tests pass

- [ ] **Step 2: Manual verification — build with TTY**

Run: `cargo build && ./target/debug/cook build` on a real project
Expected: Progress bars, active nodes, streaming output, cache hit display as designed

- [ ] **Step 3: Manual verification — build piped**

Run: `./target/debug/cook build 2>&1 | cat`
Expected: Clean `[recipe] line` format

- [ ] **Step 4: Manual verification — cook test**

Run: `./target/debug/cook test`
Expected: Progress bars during execution, test summary with symbols/colors

- [ ] **Step 5: Manual verification — --color=never**

Run: `./target/debug/cook build --color=never`
Expected: No ANSI escapes in output

- [ ] **Step 6: Final commit**

```bash
git commit --allow-empty -m "feat: CLI progress UI implementation complete"
```
