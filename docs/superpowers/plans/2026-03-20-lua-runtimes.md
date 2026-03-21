# Lua Runtime Crates Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract cook-luaotp (worker pool of Lua VMs) and cook-register (capture-mode Lua VM with Cook API bindings).

**Architecture:** cook-luaotp is the execution engine — a pool of worker threads, each with its own Lua VM, pulling work items from a shared queue. cook-register is the registration engine — a single-threaded Lua VM that runs generated Cookfile Lua in capture mode to discover work units. cook-register depends only on cook-contracts (NOT on cook-cache). Cache evaluation is deferred to cook-engine in the next phase.

**Tech Stack:** Rust 2024 edition, mlua (Lua 5.4, vendored)

**Spec:** `docs/superpowers/specs/2026-03-20-monorepo-ddd-rewrite-design.md`

**Prerequisite:** Plan 2 (Pure Transforms) must be completed first.

---

## File Structure

### New files to create:

```
cli/crates/
├── cook-luaotp/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs          # Public API: WorkerPool, WorkItem, WorkResult
│       └── pool.rs         # Worker thread implementation with per-thread Lua VMs
└── cook-register/
    ├── Cargo.toml
    └── src/
        ├── lib.rs          # Public API: Registry, register_recipe()
        ├── capture.rs      # CaptureState, capture-mode API bindings (layer, begin_step, etc.)
        ├── unit_api.rs     # cook.add_unit(), cook.step_group() bindings
        ├── export_api.rs   # cook.export(), cook.import() bindings
        ├── fs_api.rs       # fs.glob(), fs.exists(), fs.read(), fs.write(), fs.mkdir_p()
        ├── path_api.rs     # path.stem(), path.name(), path.ext(), path.dir()
        ├── platform_api.rs # cook.platform info
        ├── module_cache.rs # cook.cache.get/set (registration-time k/v cache)
        ├── module_loader.rs # Lua require() implementation for cook_modules/
        └── engine.rs       # Lua VM setup, recipe discovery, registration orchestration
```

---

## Chunk 1: cook-luaotp

### Task 1: Create cook-luaotp crate

**Files:**
- Create: `cli/crates/cook-luaotp/Cargo.toml`
- Create: `cli/crates/cook-luaotp/src/lib.rs`
- Create: `cli/crates/cook-luaotp/src/pool.rs`

**Source reference:** `~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/pool.rs` (719 lines)

cook-luaotp extracts the worker pool from `scheduler/pool.rs`. Each worker thread creates its own `mlua::Lua` VM and processes `WorkItem`s from a shared queue, returning `WorkResult`s via mpsc channel.

- [ ] **Step 1: Create Cargo.toml**

Create `cli/crates/cook-luaotp/Cargo.toml`:

```toml
[package]
name = "cook-luaotp"
version = "0.1.0"
edition = "2024"
description = "Worker pool of Lua VMs for parallel work execution"

[dependencies]
cook-contracts = { path = "../cook-contracts" }
mlua = { version = "0.10", features = ["lua54", "vendored"] }
```

- [ ] **Step 2: Copy pool.rs**

```bash
cp ~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/pool.rs \
   ~/dev/cook/cli/crates/cook-luaotp/src/pool.rs
```

- [ ] **Step 3: Create lib.rs**

Create `cli/crates/cook-luaotp/src/lib.rs`:

```rust
mod pool;

pub use pool::{WorkerPool, WorkItem, WorkResult, TestOutput};
```

- [ ] **Step 4: Update imports in pool.rs**

Replace:
- `use crate::contracts::WorkPayload` → `use cook_contracts::WorkPayload`
- Remove any `use crate::runtime::*` imports — the worker pool currently registers Cook APIs (`cook.sh`, `cook.exec`, `fs.*`, `path.*`) inside each worker thread. These registrations need to stay in cook-luaotp since the workers need them for execution.

Examine the current `pool.rs` carefully. It likely has a `worker_loop` function that:
1. Creates a Lua VM
2. Registers Cook APIs (fs, path, cook.exec, etc.)
3. Pulls work items from the queue
4. Executes them (shell commands via `/bin/sh -c`, Lua chunks via `lua.load()`, tests with timeouts)

The API registrations in the worker (for execution, NOT registration) should be self-contained within cook-luaotp. If they reference functions from `src/runtime/`, those functions need to be moved here or duplicated (they're typically simple Lua function bindings).

- [ ] **Step 5: Verify compilation**

```bash
cd ~/dev/cook/cli
cargo build -p cook-luaotp
```

Fix any remaining import issues. The key challenge is extracting the Lua API bindings that workers need during execution.

- [ ] **Step 6: Run tests**

```bash
cd ~/dev/cook/cli
cargo test -p cook-luaotp
```

- [ ] **Step 7: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-luaotp/
git commit -m "feat: add cook-luaotp crate — worker pool of Lua VMs"
```

---

## Chunk 2: cook-register

### Task 2: Create cook-register crate

**Files:**
- Create: `cli/crates/cook-register/Cargo.toml`
- Create: `cli/crates/cook-register/src/lib.rs`
- Create: `cli/crates/cook-register/src/capture.rs`
- Create: `cli/crates/cook-register/src/unit_api.rs`
- Create: `cli/crates/cook-register/src/export_api.rs`
- Create: `cli/crates/cook-register/src/fs_api.rs`
- Create: `cli/crates/cook-register/src/path_api.rs`
- Create: `cli/crates/cook-register/src/platform_api.rs`
- Create: `cli/crates/cook-register/src/module_cache.rs`
- Create: `cli/crates/cook-register/src/module_loader.rs`
- Create: `cli/crates/cook-register/src/engine.rs`

**Source reference:** `~/dev/remote/github.com/Alex-Gilbert/cook/src/runtime/` (2,182 lines)

cook-register extracts the entire `runtime/` module. This is the capture-mode Lua VM that runs generated Cookfile Lua and discovers work units.

**CRITICAL DDD CONSTRAINT:** cook-register does NOT depend on cook-cache. In the current codebase, `runtime/capture.rs` calls into cache to check hit/miss and produces pre-satisfied units. In cook-register, ALL units are captured without cache filtering. Cache evaluation moves to cook-engine.

- [ ] **Step 1: Create Cargo.toml**

Create `cli/crates/cook-register/Cargo.toml`:

```toml
[package]
name = "cook-register"
version = "0.1.0"
edition = "2024"
description = "Capture-mode Lua VM — runs generated Cookfile Lua, discovers work units"

[dependencies]
cook-contracts = { path = "../cook-contracts" }
mlua = { version = "0.10", features = ["lua54", "vendored"] }
glob = "0.3"
xxhash-rust = { version = "0.8", features = ["xxh3"] }
serde = { version = "1", features = ["derive"] }
bincode = "1"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Copy runtime source files**

```bash
OLD=~/dev/remote/github.com/Alex-Gilbert/cook/src/runtime
NEW=~/dev/cook/cli/crates/cook-register/src

mkdir -p $NEW
cp $OLD/capture.rs $NEW/capture.rs
cp $OLD/unit_api.rs $NEW/unit_api.rs
cp $OLD/export_api.rs $NEW/export_api.rs
cp $OLD/fs_api.rs $NEW/fs_api.rs
cp $OLD/path_api.rs $NEW/path_api.rs
cp $OLD/platform_api.rs $NEW/platform_api.rs
cp $OLD/module_cache.rs $NEW/module_cache.rs
cp $OLD/module_loader.rs $NEW/module_loader.rs
cp $OLD/engine.rs $NEW/engine.rs
cp $OLD/context.rs $NEW/context.rs
cp $OLD/tests.rs $NEW/tests.rs
```

- [ ] **Step 3: Move CaptureState and SharedCaptureState into this crate**

In the old codebase, `CaptureState` and `SharedCaptureState` live in `contracts/mod.rs`. In the new design, they belong in cook-register. They're already being copied via `capture.rs` — ensure they're defined here, not imported from cook-contracts.

Add to `capture.rs` (or a new file):

```rust
use std::{cell::RefCell, rc::Rc};
use cook_contracts::{CapturedUnit, DepKind};

/// Shared state accumulated during capture-mode execution.
pub struct CaptureState {
    pub inside_layer: bool,
    pub layer_commands: Vec<(String, usize)>,
    pub units: Vec<CapturedUnit>,
    pub current_group: Option<usize>,
    pub step_groups: Vec<Vec<usize>>,
}

impl CaptureState {
    pub fn new() -> Self {
        Self {
            inside_layer: false,
            layer_commands: Vec::new(),
            units: Vec::new(),
            current_group: None,
            step_groups: Vec::new(),
        }
    }
}

pub type SharedCaptureState = Rc<RefCell<CaptureState>>;
```

- [ ] **Step 4: Create lib.rs with Registry API**

Create `cli/crates/cook-register/src/lib.rs`:

```rust
mod capture;
mod unit_api;
mod export_api;
mod fs_api;
mod path_api;
mod platform_api;
mod module_cache;
mod module_loader;
mod engine;
mod context;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::PathBuf;

use cook_contracts::RecipeUnits;

pub use capture::{CaptureState, SharedCaptureState};

/// Errors that can occur during recipe registration.
#[derive(Debug)]
pub enum RegisterError {
    RecipeNotFound(String),
    Lua(String),
    CommandFailed {
        command: String,
        line: usize,
        code: i32,
    },
}

/// Opaque store for cross-recipe export/import data.
/// Owned by cook-engine, passed to Registry per call.
pub struct ExportStore {
    inner: BTreeMap<String, mlua::Value<'static>>,
}

impl ExportStore {
    pub fn new() -> Self {
        Self { inner: BTreeMap::new() }
    }
}

/// The registration runtime. Runs generated Lua in capture mode
/// to discover work units for a recipe.
pub struct Registry {
    working_dir: PathBuf,
    env_vars: BTreeMap<String, String>,
}

impl Registry {
    pub fn new(working_dir: PathBuf, env_vars: BTreeMap<String, String>) -> Self {
        Self { working_dir, env_vars }
    }

    /// Execute generated Lua and capture all registered work units.
    pub fn register_recipe(
        &self,
        lua_source: &str,
        recipe_name: &str,
        export_store: &mut ExportStore,
    ) -> Result<RecipeUnits, RegisterError> {
        engine::register_recipe(
            &self.working_dir,
            &self.env_vars,
            lua_source,
            recipe_name,
            export_store,
        )
    }

    pub fn working_dir(&self) -> &PathBuf {
        &self.working_dir
    }
}
```

Note: The `ExportStore` type wraps Lua values. The `'static` lifetime for `mlua::Value` may need adjustment — check how the current `export_api.rs` stores values. It may use `mlua::RegistryKey` instead. Adapt accordingly.

- [ ] **Step 5: Remove cache dependency from capture.rs**

This is the critical DDD fix. In the current `capture.rs`, the `register_layer_api_capture` function checks the cache to determine if a unit is pre-satisfied. Remove this logic:

- Remove all imports from `crate::cache::*`
- In the layer registration function, ALWAYS create a `CapturedUnit` with the full payload — never produce a pre-satisfied (empty payload) unit
- Remove the `CacheState` wrapper that's used during registration
- The `command_hash` field in `CacheMeta` should still be computed (using xxhash) — this is metadata about the unit, not a cache check

After this change, `register_recipe()` returns ALL units with full payloads. cook-engine (in the next phase) will handle cache filtering.

- [ ] **Step 6: Update all internal imports**

In all copied files, replace:
- `use crate::contracts::*` → `use cook_contracts::*`
- `use crate::runtime::*` → `use crate::*`
- `use crate::cache::*` → REMOVE (cook-register has no cache dependency)
- `use super::*` paths should work within the crate

- [ ] **Step 7: Run tests**

```bash
cd ~/dev/cook/cli
cargo test -p cook-register
```

Expected: all registration/runtime tests pass. Tests that previously checked cache-hit behavior may need to be updated since cook-register now captures ALL units.

- [ ] **Step 8: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-register/
git commit -m "feat: add cook-register crate — capture-mode Lua VM for work unit discovery

BREAKING: cook-register does not filter by cache. All units are
captured. Cache evaluation deferred to cook-engine."
```

---

## Chunk 3: Workspace Verification

### Task 3: Update workspace and verify

- [ ] **Step 1: Update workspace Cargo.toml**

Add to `cli/Cargo.toml` members:

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
]
resolver = "2"
```

- [ ] **Step 2: Full workspace build and test**

```bash
cd ~/dev/cook/cli
cargo build
cargo test
```

Expected: all 8 crates compile, all tests pass.

- [ ] **Step 3: Verify dependency isolation**

```bash
cd ~/dev/cook/cli
cargo tree -p cook-luaotp  # should show only cook-contracts + mlua
cargo tree -p cook-register  # should show cook-contracts + mlua, NO cook-cache
```

**CRITICAL CHECK:** Verify cook-register does NOT depend on cook-cache:

```bash
cd ~/dev/cook/cli
cargo tree -p cook-register | grep cook-cache
```

Expected: no output. If cook-cache appears, there's a dependency leak that must be fixed.

- [ ] **Step 4: Commit**

```bash
cd ~/dev/cook
git add cli/Cargo.toml
git commit -m "chore: add cook-luaotp and cook-register to workspace"
```

---

## Completion Criteria

- [ ] cook-luaotp compiles, tests pass, depends only on cook-contracts + mlua
- [ ] cook-register compiles, tests pass, depends only on cook-contracts + mlua (NO cook-cache)
- [ ] `CaptureState`/`SharedCaptureState` live in cook-register, not cook-contracts
- [ ] cook-register produces ALL units (no cache filtering)
- [ ] `ExportStore` type defined in cook-register
- [ ] Full workspace (`cargo test`) passes for all 8 crates
- [ ] `cargo tree` confirms no cook-cache dependency in cook-register

---

## Review Errata (from plan review)

These issues were identified during plan review. The executing agent MUST address them:

### ExportStore ownership

1. **ExportStore MUST NOT be defined in cook-register.** The spec assigns ExportStore to cook-engine. Define the type in cook-engine (or cook-contracts if needed by both). cook-register's `register_recipe` takes `&mut ExportStore` as a parameter — it does not own the type.

2. **ExportStore inner type is `serde_json::Value`, NOT `mlua::Value`.** The current `export_api.rs` already uses `serde_json::Value` via `lua_value_to_json` conversion. This avoids Lua VM lifetime coupling. Add `serde_json = "1"` to the relevant Cargo.toml.

### Missing files

3. **`test_api.rs` is missing from the plan.** The current `runtime/test_api.rs` (42 lines) registers `cook.test_case()`. It must either be copied to cook-register or explicitly moved to cook-cli. `engine.rs` calls `super::test_api::register_test_api(...)` — this will fail if the file is not present.

### context.rs cache dependency

4. **`context.rs` depends on `crate::cache`.** It imports `SharedCacheState`, calls `hash_env()`, `hash_secondary_inputs()`, and `state.invalidate_recipe()`. Since cook-register must NOT depend on cook-cache, these cache operations must be stripped from `context.rs` and moved to cook-engine. The plan's Step 5 only discusses cache removal from `capture.rs` — `context.rs` needs the same treatment.

### hash_str location

5. **`hash_str` moves to cook-cache per spec, but `module_loader.rs` and `unit_api.rs` use it.** Since cook-register cannot depend on cook-cache, either: (a) inline the xxhash call directly (`xxhash_rust::xxh3::xxh3_64(s.as_bytes())` — it's one line), or (b) duplicate `hash_str` in cook-register. The Cargo.toml already includes `xxhash-rust`, so option (a) is simplest.

### ProgressEvent dependency

6. **`capture.rs` references `crate::scheduler::progress::ProgressEvent`.** The `register_cook_api_capture` function takes `event_tx: Option<mpsc::Sender<ProgressEvent>>`. In cook-register, there is no access to this type. Either: (a) replace with a generic callback `on_event: Option<Box<dyn Fn(RegisterEvent)>>`, (b) define a `RegisterEvent` enum in cook-register, or (c) move the event emission to cook-engine's orchestration layer.

### Dead codegen API

7. **The old codegen API functions (`register_layer_api_capture`, `register_test_layer_api_capture`, `cook.begin_step`, `cook.end_step`, `cook.layer`, `cook.exec`) are dead code after API unification.** Since cook-luagen now targets `step_group`/`add_unit` exclusively, these functions are never called. Either remove them now or keep them with a `#[deprecated]` annotation and a TODO to remove.

### Missing dependencies

8. **Add `serde_json = "1"` to cook-register's Cargo.toml.** Required for `export_api.rs` (which uses `serde_json::Value`) and `module_loader.rs` (which has `lua_value_to_json`).

9. **Consider adding `thiserror = "2"` for `RegisterError`** to match the project's error handling pattern.
