# Engine + CLI Crates Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract cook-engine (the build pipeline orchestrator) and cook-cli (the user-facing binary shell), completing the DDD crate split.

**Architecture:** cook-engine is the conductor — it drives the recipe DAG, calls cook-register per wave, evaluates cache staleness via cook-cache, builds work-unit DAGs via cook-dag, and feeds ready nodes to cook-luaotp. cook-cli is the thin presentation layer — CLI args, progress UI, env resolution, and error formatting.

**Tech Stack:** Rust 2024 edition, clap, crossterm, notify

**Spec:** `docs/superpowers/specs/2026-03-20-monorepo-ddd-rewrite-design.md`

**Prerequisite:** Plan 3 (Lua Runtimes) must be completed first.

---

## File Structure

### New files to create:

```
cli/crates/
├── cook-engine/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs          # Public API: Engine, EngineEvent, EngineError
│       ├── orchestrator.rs # Run/run_workspace — the dual-DAG loop
│       ├── recipe_dag.rs   # RecipeDag — recipe-level wave scheduling
│       ├── dag_builder.rs  # RecipeUnits → Dag<WorkPayload> conversion
│       ├── cache_eval.rs   # Post-registration cache evaluation (ask cook-cache per unit)
│       ├── executor.rs     # DAG execution loop: walk topology, feed to luaotp
│       ├── analyzer.rs     # Recipe dependency resolution, topological sorting
│       └── workspace.rs    # Workspace loading (multi-Cookfile)
└── cook-cli/
    ├── Cargo.toml
    └── src/
        ├── main.rs         # Binary entry point
        ├── cli.rs          # clap argument parsing
        ├── pipeline.rs     # Glue: cook-lang → cook-luagen → cook-engine
        ├── env.rs          # Environment resolution (.env, configs, --set)
        ├── progress.rs     # Progress renderer thread, EngineEvent → cook-progress
        ├── watcher.rs      # cook serve file watcher
        ├── color.rs        # Color config (terminal detection, NO_COLOR, --color)
        ├── test_output.rs  # TestResults types, JUnit XML, terminal summary
        └── error.rs        # EngineError → user-facing error messages
```

---

## Chunk 1: cook-engine

### Task 1: Create cook-engine crate

**Files:**
- Create: `cli/crates/cook-engine/Cargo.toml`
- Create: `cli/crates/cook-engine/src/lib.rs`
- Create: `cli/crates/cook-engine/src/orchestrator.rs`
- Create: `cli/crates/cook-engine/src/recipe_dag.rs`
- Create: `cli/crates/cook-engine/src/dag_builder.rs`
- Create: `cli/crates/cook-engine/src/cache_eval.rs`
- Create: `cli/crates/cook-engine/src/executor.rs`
- Create: `cli/crates/cook-engine/src/analyzer.rs`
- Create: `cli/crates/cook-engine/src/workspace.rs`

**Source reference files:**
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/recipe_dag.rs` (179 lines)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/builder.rs` (296 lines)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/executor.rs` (484 lines)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/analyzer/` (graph.rs + mod.rs)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/engine/workspace.rs` (303 lines)
- Parts of `~/dev/remote/github.com/Alex-Gilbert/cook/src/engine/pipeline.rs` (orchestration logic only)

- [ ] **Step 1: Create Cargo.toml**

Create `cli/crates/cook-engine/Cargo.toml`:

```toml
[package]
name = "cook-engine"
version = "0.1.0"
edition = "2024"
description = "Build pipeline orchestrator — recipe DAG, cache evaluation, execution"

[dependencies]
cook-contracts = { path = "../cook-contracts" }
cook-register = { path = "../cook-register" }
cook-dag = { path = "../cook-dag" }
cook-luaotp = { path = "../cook-luaotp" }
cook-cache = { path = "../cook-cache" }
cook-lang = { path = "../cook-lang" }
glob = "0.3"
thiserror = "2"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Copy and adapt recipe_dag.rs**

```bash
cp ~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/recipe_dag.rs \
   ~/dev/cook/cli/crates/cook-engine/src/recipe_dag.rs
```

This file is self-contained (uses only `BTreeMap`). Minimal changes needed — just update any crate-level imports.

- [ ] **Step 3: Copy and adapt dag_builder.rs**

```bash
cp ~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/builder.rs \
   ~/dev/cook/cli/crates/cook-engine/src/dag_builder.rs
```

Update imports:
- `use crate::contracts::*` → `use cook_contracts::*`
- `use super::dag::ExecutionDag` → `use cook_dag::Dag`
- Adapt `build_dag()` to construct a `cook_dag::Dag<WorkPayload>` instead of the old `ExecutionDag`

The builder converts `RecipeUnits` (with step groups and barriers) into a flat `Dag<WorkPayload>` with proper dependency edges. The barrier/step-group logic stays the same — it just targets the generic `Dag<T>` API.

- [ ] **Step 4: Create cache_eval.rs — post-registration cache evaluation**

This is NEW code. In the old architecture, cache checking happened inside `capture.rs` during registration. In the new architecture, it happens here in cook-engine AFTER registration.

```rust
use cook_cache::CacheManager;
use cook_contracts::{CapturedUnit, RecipeUnits};

/// Evaluate cache staleness for all units in a RecipeUnits.
/// Units that are cache-fresh get their payload set to None
/// (pre-satisfied) so the DAG builder can skip them.
pub fn evaluate_cache(
    units: &mut RecipeUnits,
    cache: &CacheManager,
) {
    for unit in &mut units.units {
        if let Some(meta) = &unit.cache_meta {
            if cache.is_fresh(meta) {
                // Mark as pre-satisfied — DAG builder will create
                // a no-op node that completes immediately
                unit.payload = None; // NOTE: WorkPayload may need to become Option<WorkPayload>
            }
        }
    }
}
```

Note: This may require `CapturedUnit.payload` to become `Option<WorkPayload>`. Check the current `ExecutionDag` node — it already uses `Option<WorkPayload>` for pre-satisfied nodes. The change should propagate to cook-contracts.

- [ ] **Step 5: Create executor.rs — DAG execution loop**

Adapt from `~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/executor.rs` (484 lines).

The executor walks the `Dag<WorkPayload>`, feeds ready nodes to cook-luaotp's worker pool, and handles completions/failures/cancellations. This is Cook-specific orchestration (not generic DAG traversal) because it needs to:

- Emit `EngineEvent`s (recipe started/completed/failed, node started/completed, etc.)
- Handle interactive commands on the main thread
- Update cache after successful execution
- Cancel subtrees on failure
- Track recipe-level progress

Update imports:
- `use crate::contracts::WorkPayload` → `use cook_contracts::WorkPayload`
- `use super::dag::ExecutionDag` → `use cook_dag::Dag`
- `use super::pool::*` → `use cook_luaotp::*`
- `use crate::cache::*` → `use cook_cache::*`

- [ ] **Step 6: Copy and adapt analyzer.rs**

```bash
# Copy both files from the analyzer module
cp ~/dev/remote/github.com/Alex-Gilbert/cook/src/analyzer/graph.rs \
   ~/dev/cook/cli/crates/cook-engine/src/analyzer.rs
```

Merge the contents of `analyzer/mod.rs` (which has `build_recipe_info`, `build_workspace_recipe_info`, `find_full_prefix`) into `analyzer.rs`.

Update imports:
- `use crate::parser::ast::*` → `use cook_lang::ast::*`
- `use crate::engine::workspace::*` → `use crate::workspace::*`

- [ ] **Step 7: Copy and adapt workspace.rs**

```bash
cp ~/dev/remote/github.com/Alex-Gilbert/cook/src/engine/workspace.rs \
   ~/dev/cook/cli/crates/cook-engine/src/workspace.rs
```

Update imports:
- `use crate::parser` → `use cook_lang`
- `use crate::codegen` → remove (cook-engine doesn't do codegen — cook-cli does)

The workspace loader reads Cookfiles and parses them. It may need to call `cook_lang::parse()` directly. If it also calls `codegen::generate()`, that part should be moved to cook-cli's pipeline glue.

- [ ] **Step 8: Create orchestrator.rs — the dual-DAG loop**

Extract the orchestration logic from `engine/pipeline.rs` (lines 114-250 for `cmd_run` and lines 252-425 for `cmd_run_workspace`).

This is the core of cook-engine — the dual-DAG loop:

```rust
use cook_contracts::RecipeUnits;
use cook_register::{Registry, ExportStore};
use cook_dag::Dag;
use cook_luaotp::WorkerPool;
use cook_cache::CacheManager;

use crate::recipe_dag::RecipeDag;
use crate::dag_builder;
use crate::cache_eval;
use crate::executor;

pub fn run_workspace(
    registries: &BTreeMap<String, Registry>,
    recipe_dag: &mut RecipeDag,
    cache: &CacheManager,
    pool: &WorkerPool,
    on_event: impl Fn(EngineEvent),
) -> Result<(), EngineError> {
    let mut export_store = ExportStore::new();

    loop {
        let ready = recipe_dag.pop_ready();
        if ready.is_empty() {
            break;
        }

        let mut wave_units: Vec<RecipeUnits> = Vec::new();

        for name in &ready {
            let registry = registries.get(name)
                .ok_or_else(|| EngineError::Other(format!("no registry for '{name}'")))?;

            let mut units = registry.register_recipe(lua_source, name, &mut export_store)?;
            cache_eval::evaluate_cache(&mut units, cache);
            wave_units.push(units);
        }

        let dag = dag_builder::build_dag(wave_units);
        executor::execute(dag, pool, cache, &on_event)?;

        recipe_dag.mark_done(&ready);
    }

    Ok(())
}
```

Strip all CLI concerns (TTY detection, color, renderer spawning, error formatting) — those belong in cook-cli.

- [ ] **Step 9: Create lib.rs**

Create `cli/crates/cook-engine/src/lib.rs`:

```rust
pub mod orchestrator;
pub mod recipe_dag;
pub mod dag_builder;
pub mod cache_eval;
pub mod executor;
pub mod analyzer;
pub mod workspace;

pub use recipe_dag::RecipeDag;
pub use orchestrator::{run, run_workspace};

/// Events emitted during engine execution.
/// cook-cli subscribes to these for progress rendering.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    RecipeQueued { name: String, total_nodes: usize },
    RecipeStarted { name: String, total_nodes: usize },
    RecipeCompleted { name: String, elapsed: std::time::Duration, cached_nodes: usize, total_nodes: usize },
    RecipeFailed { name: String, elapsed: std::time::Duration, completed_nodes: usize, total_nodes: usize },
    NodeStarted { recipe: String, node_name: String },
    NodeCompleted { recipe: String, node_name: String, elapsed: std::time::Duration },
    NodeFailed { recipe: String, node_name: String, elapsed: std::time::Duration, error: String },
    NodeCacheHit { recipe: String, node_name: String },
    NodeSkipped { recipe: String, node_name: String },
    InteractiveStart { recipe: String },
    InteractiveEnd { recipe: String, elapsed: std::time::Duration, success: bool },
    OutputLine { recipe: String, line: String, is_stderr: bool },
    Finished,
}

/// Engine-level errors.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("recipe not found: {0}")]
    RecipeNotFound(String),
    #[error("command failed: {0}")]
    CommandFailed(String),
    #[error("test failure: {0} test(s) failed")]
    TestFailure(usize),
    #[error("{0}")]
    Other(String),
}
```

- [ ] **Step 10: Verify compilation**

```bash
cd ~/dev/cook/cli
cargo build -p cook-engine
```

Fix import issues iteratively. This is the most complex extraction since cook-engine touches many modules.

- [ ] **Step 11: Run tests**

```bash
cd ~/dev/cook/cli
cargo test -p cook-engine
```

- [ ] **Step 12: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-engine/
git commit -m "feat: add cook-engine crate — build pipeline orchestrator

Owns recipe DAG, cache evaluation, DAG building, execution
orchestration, analyzer, and workspace loading."
```

---

## Chunk 2: cook-cli

### Task 2: Create cook-cli binary crate

**Files:**
- Create: `cli/crates/cook-cli/Cargo.toml`
- Create: `cli/crates/cook-cli/src/main.rs`
- Create: `cli/crates/cook-cli/src/cli.rs`
- Create: `cli/crates/cook-cli/src/pipeline.rs`
- Create: `cli/crates/cook-cli/src/env.rs`
- Create: `cli/crates/cook-cli/src/progress.rs`
- Create: `cli/crates/cook-cli/src/watcher.rs`
- Create: `cli/crates/cook-cli/src/color.rs`
- Create: `cli/crates/cook-cli/src/test_output.rs`
- Create: `cli/crates/cook-cli/src/error.rs`

**Source reference files:**
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/cli/mod.rs` (103 lines)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/engine/pipeline.rs` (CLI glue parts)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/engine/commands.rs` (119 lines)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/engine/error.rs` (34 lines)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/engine/test_output.rs` (294 lines)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/env/mod.rs`
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/watcher/mod.rs`
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/color.rs` (205 lines)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/progress.rs` (909 lines)
- `~/dev/remote/github.com/Alex-Gilbert/cook/src/main.rs` (8 lines)

- [ ] **Step 1: Create Cargo.toml**

Create `cli/crates/cook-cli/Cargo.toml`:

```toml
[package]
name = "cook-cli"
version = "0.1.0"
edition = "2024"
description = "Cook build system CLI"

[[bin]]
name = "cook"
path = "src/main.rs"

[dependencies]
cook-engine = { path = "../cook-engine" }
cook-lang = { path = "../cook-lang" }
cook-luagen = { path = "../cook-luagen" }
cook-progress = { path = "../cook-progress" }
cook-contracts = { path = "../cook-contracts" }
clap = { version = "4", features = ["derive"] }
crossterm = "0.28"
notify = "7"
dotenvy = "0.15"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create main.rs**

```rust
fn main() {
    if let Err(e) = cook_cli::run() {
        eprintln!("cook: {e}");
        std::process::exit(1);
    }
}
```

Wait — the binary name is `cook` (set in `[[bin]]`), but the crate is `cook-cli`. The `lib.rs` exports the `run()` function. Alternatively, keep it simple like the old repo:

```rust
mod cli;
mod pipeline;
mod env;
mod progress;
mod watcher;
mod color;
mod test_output;
mod error;

fn main() {
    if let Err(e) = cli::run() {
        eprintln!("cook: {e}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 3: Copy and adapt cli.rs (clap args)**

```bash
cp ~/dev/remote/github.com/Alex-Gilbert/cook/src/cli/mod.rs \
   ~/dev/cook/cli/crates/cook-cli/src/cli.rs
```

Update: remove `pub mod` declarations, just keep the `Cli` struct and `run()` function. The `run()` function dispatches to pipeline functions.

- [ ] **Step 4: Create pipeline.rs — the glue**

Extract from `engine/pipeline.rs` the parts that are CLI concerns:
- Cookfile discovery and parsing: calls `cook_lang::parse()`
- Code generation: calls `cook_luagen::generate()`
- Environment resolution: calls `env::resolve_env()`
- Progress renderer spawning
- Passing Lua source + env to `cook_engine::run()`

This is where cook-lang → cook-luagen → cook-engine gets wired together.

- [ ] **Step 5: Create env.rs**

```bash
cp ~/dev/remote/github.com/Alex-Gilbert/cook/src/env/mod.rs \
   ~/dev/cook/cli/crates/cook-cli/src/env.rs
```

Also extract `resolve_env()` from `engine/pipeline.rs` into this file. Environment resolution (system env → Cookfile vars → config → .env → --set) is a CLI concern.

- [ ] **Step 6: Create progress.rs**

Adapt from `scheduler/progress.rs` (909 lines). This handles:
- `ProgressEvent` → maps from `EngineEvent`
- `TtyRenderer` / `PlainRenderer`
- Renderer thread spawning

This is the bridge between `cook_engine::EngineEvent` and `cook_progress` rendering primitives.

- [ ] **Step 7: Create color.rs**

```bash
cp ~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/color.rs \
   ~/dev/cook/cli/crates/cook-cli/src/color.rs
```

Self-contained — terminal detection, NO_COLOR, --color flag.

- [ ] **Step 8: Create test_output.rs**

```bash
cp ~/dev/remote/github.com/Alex-Gilbert/cook/src/engine/test_output.rs \
   ~/dev/cook/cli/crates/cook-cli/src/test_output.rs
```

This file also absorbs the test result types (`TestResults`, `TestCaseResult`, etc.) from contracts, plus JUnit XML writing and terminal summary formatting.

- [ ] **Step 9: Create watcher.rs**

```bash
cp ~/dev/remote/github.com/Alex-Gilbert/cook/src/watcher/mod.rs \
   ~/dev/cook/cli/crates/cook-cli/src/watcher.rs
```

File watcher for `cook serve` mode.

- [ ] **Step 10: Create error.rs**

Extract `CookError` from `engine/error.rs` and adapt to wrap `EngineError`:

```rust
use cook_engine::EngineError;

#[derive(Debug, thiserror::Error)]
pub enum CookError {
    #[error("{0}")]
    Engine(#[from] EngineError),
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("{0}")]
    Other(String),
}
```

- [ ] **Step 11: Verify compilation**

```bash
cd ~/dev/cook/cli
cargo build -p cook-cli
```

- [ ] **Step 12: Run the binary**

```bash
cd ~/dev/cook/cli
cargo run -p cook-cli -- --help
```

Expected: Cook CLI help output with run, test, init, menu, serve subcommands.

- [ ] **Step 13: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-cli/
git commit -m "feat: add cook-cli crate — user-facing binary shell"
```

---

## Chunk 3: Integration Tests

### Task 3: Move and adapt integration tests

**Files:**
- Create: `cli/tests/integration.rs` (from old repo's `tests/integration.rs`)
- Create: `cli/tests/import_integration.rs` (from old repo's `tests/import_integration.rs`)

- [ ] **Step 1: Copy integration tests**

```bash
cp ~/dev/remote/github.com/Alex-Gilbert/cook/tests/integration.rs \
   ~/dev/cook/cli/tests/integration.rs
cp ~/dev/remote/github.com/Alex-Gilbert/cook/tests/import_integration.rs \
   ~/dev/cook/cli/tests/import_integration.rs
```

- [ ] **Step 2: Update test imports and paths**

Integration tests invoke the `cook` binary via `std::process::Command`. Update paths to point to the new binary location. The binary is built as `target/debug/cook` (since `[[bin]] name = "cook"`).

Also update any hardcoded paths to example Cookfiles to use `../../examples/` (relative to `cli/tests/`).

- [ ] **Step 3: Run integration tests**

```bash
cd ~/dev/cook/cli
cargo test --test integration
cargo test --test import_integration
```

Fix failures iteratively. Common issues:
- Path references to examples/
- Binary name changes
- Import path changes

- [ ] **Step 4: Commit**

```bash
cd ~/dev/cook
git add cli/tests/
git commit -m "feat: add integration tests for cook-cli"
```

---

## Chunk 4: Final Workspace Verification

### Task 4: Full workspace verification

- [ ] **Step 1: Update workspace Cargo.toml**

Final `cli/Cargo.toml`:

```toml
[workspace]
members = [
    "crates/cook-contracts",
    "crates/cook-lang",
    "crates/cook-progress",
    "crates/cook-cache",
    "crates/cook-dag",
    "crates/cook-luagen",
    "crates/cook-luaotp",
    "crates/cook-register",
    "crates/cook-engine",
    "crates/cook-cli",
]
resolver = "2"
```

- [ ] **Step 2: Full build**

```bash
cd ~/dev/cook/cli
cargo build
```

- [ ] **Step 3: Full test suite**

```bash
cd ~/dev/cook/cli
cargo test
```

Expected: ALL tests pass — unit tests in each crate + integration tests.

- [ ] **Step 4: Verify dependency graph**

```bash
cd ~/dev/cook/cli
cargo tree -p cook-cli
```

Verify the dependency tree matches the spec:
- cook-cli → cook-engine, cook-lang, cook-luagen, cook-progress, cook-contracts
- cook-engine → cook-contracts, cook-register, cook-dag, cook-luaotp, cook-cache
- cook-register → cook-contracts (NO cook-cache)
- cook-dag → nothing
- cook-lang → nothing
- cook-progress → nothing

- [ ] **Step 5: Run cook against example projects**

```bash
cd ~/dev/cook/examples
../cli/target/debug/cook run build
```

```bash
cd ~/dev/cook/examples/cpp-project
../../cli/target/debug/cook run app
```

Expected: both example projects build successfully.

- [ ] **Step 6: Final commit**

```bash
cd ~/dev/cook
git add -A
git commit -m "chore: complete 10-crate DDD workspace — all tests passing"
```

---

## Completion Criteria

- [ ] All 10 crates compile: cook-contracts, cook-lang, cook-progress, cook-cache, cook-dag, cook-luagen, cook-luaotp, cook-register, cook-engine, cook-cli
- [ ] `cargo test` passes for entire workspace (unit + integration)
- [ ] `cook` binary works against example projects
- [ ] Dependency graph matches the spec exactly
- [ ] cook-register has NO dependency on cook-cache
- [ ] cook-dag depends on nothing
- [ ] No code from cook-cli leaks into library crates
- [ ] `EngineEvent` is the sole interface between cook-engine and cook-cli for progress
- [ ] Test result types live in cook-cli, not cook-contracts
- [ ] `CaptureState` lives in cook-register, not cook-contracts
- [ ] `ExportStore` is defined in cook-engine (NOT cook-register), passed to register per call

---

## Review Errata (from plan review)

These issues were identified during plan review. The executing agent MUST address them:

### cook-engine

1. **workspace.rs belongs in cook-cli, NOT cook-engine.** The spec explicitly states under "cook-engine does NOT own: Workspace discovery and loading (cook-cli)". Move `workspace.rs` to cook-cli. This also removes the `cook-lang` dependency from cook-engine's Cargo.toml (workspace calls `cook_lang::parse()` which is cook-cli glue). Remove `glob` from cook-engine deps too.

2. **Create lib.rs (Step 9) BEFORE Steps 2-8.** The `EngineEvent` and `EngineError` types defined in lib.rs are used by orchestrator.rs and executor.rs. Without them, nothing compiles. Move Step 9 to Step 1.

3. **`scheduler/dag.rs` → `cook_dag::Dag<T>` structural mismatch.** The current `ExecutionDag` nodes carry Cook-specific fields (`recipe_name`, `cache_meta`, `working_dir`, `env_vars`). The generic `Dag<T>` has none of these. Solution: define a `WorkNode` struct in cook-engine that bundles `Option<WorkPayload>` + metadata, then use `Dag<WorkNode>`.

4. **`engine/commands.rs` is unassigned.** Its functions (`cmd_menu`, `cmd_init`, `cmd_serve`) belong in cook-cli — absorb into `cli.rs` or `pipeline.rs`.

5. **`cmd_test` and `cmd_test_workspace` flows are not addressed.** These are ~475 lines in `pipeline.rs` covering test discovery, execution, result collection, and output. They span cook-engine (execution) and cook-cli (result formatting). Add explicit steps for splitting this logic.

6. **Add new crates to workspace Cargo.toml BEFORE trying to build them.** `cargo build -p cook-engine` in Chunk 1 Step 10 will fail if cook-engine is not yet a workspace member. Add a step to update `cli/Cargo.toml` at the start of each chunk.

7. **Migrate `scheduler/tests.rs` (168 lines) to cook-engine.** These tests cover the full execute_dag pipeline and are the primary tests for the orchestration layer.

8. **`ExportStore` should be defined in cook-engine**, not imported from cook-register. The spec assigns it here. Use `serde_json::Value` as the inner type (not `mlua::Value`).

### cook-cli

9. **Preserve exit code behavior.** The current `CookError::exit_code()` returns different codes (1 for command/test failures, 2 for parse errors, 3 for recipe not found). The plan's `main.rs` uses `exit(1)` for all errors — restore the differentiated exit codes.

10. **Integration tests may not exist to copy.** Verify `tests/integration.rs` and `tests/import_integration.rs` exist in the old repo. If they don't, Chunk 3 needs to create them from scratch rather than copy.

11. **`split_recipe_name` helper** (from `pipeline.rs:430-436`) is used by workspace flows. Place it in cook-cli's `pipeline.rs`.

### Fresh repo assumption

12. **This plan operates on a fresh `~/dev/cook/` repo** (not the old repo). There is no old source code to remove. All code is being created anew from copies. Make sure all `cp` commands reference the OLD repo as source and the NEW monorepo as destination.
