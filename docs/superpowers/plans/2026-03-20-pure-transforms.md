# Pure Transform Crates Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract cook-cache, cook-dag, and cook-luagen — the three crates that perform pure transformations with minimal dependencies.

**Architecture:** cook-cache depends on cook-contracts. cook-dag depends on nothing. cook-luagen depends on cook-lang. These crates can be extracted in parallel since they don't depend on each other. Each performs a single transformation: caching artifacts, managing DAG topology, or generating Lua from AST.

**Tech Stack:** Rust 2024 edition, Cargo workspace

**Spec:** `docs/superpowers/specs/2026-03-20-monorepo-ddd-rewrite-design.md`

**Prerequisite:** Plan 1 (Scaffold + Leaves) must be completed first.

---

## File Structure

### New files to create:

```
cli/crates/
├── cook-cache/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs          # Public API: CacheManager, hash_str, resolve_glob
│       ├── check.rs        # Cache hit/miss logic
│       ├── manager.rs      # Cache state management
│       └── store.rs        # File-based cache storage (bincode)
├── cook-dag/
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs          # Dag<T>, Node<T>, CycleError — fully generic
└── cook-luagen/
    ├── Cargo.toml
    └── src/
        ├── lib.rs          # Public API: generate()
        ├── recipe.rs       # Recipe-level codegen
        ├── cook_step.rs    # cook step → step_group/add_unit
        ├── plate_step.rs   # plate step → add_unit
        ├── test_step.rs    # test step → add_test
        ├── template.rs     # Template expansion ({in}, {out}, {stem}, {all})
        └── lua_string.rs   # Lua string escaping
```

---

## Chunk 1: cook-cache

### Task 1: Create cook-cache crate

**Files:**
- Create: `cli/crates/cook-cache/Cargo.toml`
- Create: `cli/crates/cook-cache/src/lib.rs`
- Create: `cli/crates/cook-cache/src/check.rs`
- Create: `cli/crates/cook-cache/src/manager.rs`
- Create: `cli/crates/cook-cache/src/store.rs`

**Source reference:** `~/dev/remote/github.com/Alex-Gilbert/cook/src/cache/` (967 lines)

cook-cache absorbs:
- Everything from `src/cache/` (check.rs, manager.rs, store.rs)
- `hash_str()` from `src/contracts/mod.rs`
- `resolve_glob()` from `src/contracts/mod.rs`

- [ ] **Step 1: Create Cargo.toml**

Create `cli/crates/cook-cache/Cargo.toml`:

```toml
[package]
name = "cook-cache"
version = "0.1.0"
edition = "2024"
description = "Build artifact caching — hash computation, storage, and hit/miss logic"

[dependencies]
cook-contracts = { path = "../cook-contracts" }
xxhash-rust = { version = "0.8", features = ["xxh3"] }
serde = { version = "1", features = ["derive"] }
bincode = "1"
glob = "0.3"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Copy cache source files**

```bash
OLD=~/dev/remote/github.com/Alex-Gilbert/cook/src/cache
NEW=~/dev/cook/cli/crates/cook-cache/src

mkdir -p $NEW
cp $OLD/check.rs $NEW/check.rs
cp $OLD/manager.rs $NEW/manager.rs
cp $OLD/store.rs $NEW/store.rs
```

- [ ] **Step 3: Create lib.rs with hash_str and resolve_glob**

Create `cli/crates/cook-cache/src/lib.rs`:

```rust
mod check;
mod manager;
mod store;

pub use check::CacheChecker;
pub use manager::{CacheManager, ThreadSafeCacheManager};
pub use store::CacheStore;

use std::collections::BTreeSet;
use std::path::Path;

/// Hash a string (for command templates, env vars, etc.)
pub fn hash_str(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

/// Resolve a glob pattern into a set of files relative to root.
pub fn resolve_glob(root: &Path, pattern: &str) -> BTreeSet<String> {
    let full_pattern = root.join(pattern);
    let prefix = root.to_string_lossy().to_string();

    let paths = match glob::glob(&full_pattern.to_string_lossy()) {
        Ok(p) => p,
        Err(_) => return BTreeSet::new(),
    };

    paths
        .filter_map(Result::ok)
        .filter_map(|p| {
            let path_str = p.to_string_lossy().to_string();
            Some(
                path_str
                    .strip_prefix(&prefix)
                    .unwrap_or(&path_str)
                    .trim_start_matches('/')
                    .to_string(),
            )
        })
        .collect()
}
```

- [ ] **Step 4: Update internal imports**

In the copied files, update all `use crate::contracts::*` references to `use cook_contracts::*` and all `use crate::cache::*` references to `use crate::*`.

Key imports to find and replace:
- `crate::contracts::hash_str` → `crate::hash_str`
- `crate::contracts::resolve_glob` → `crate::resolve_glob`
- `crate::contracts::CacheMeta` → `cook_contracts::CacheMeta`
- Any `use super::` paths should work within the crate.

- [ ] **Step 5: Run tests**

```bash
cd ~/dev/cook/cli
cargo test -p cook-cache
```

Expected: all cache tests pass. Fix import issues as needed.

- [ ] **Step 6: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-cache/
git commit -m "feat: add cook-cache crate — build artifact caching and hash computation"
```

---

## Chunk 2: cook-dag

### Task 2: Create cook-dag crate

**Files:**
- Create: `cli/crates/cook-dag/Cargo.toml`
- Create: `cli/crates/cook-dag/src/lib.rs`

**Source reference:** `~/dev/remote/github.com/Alex-Gilbert/cook/src/scheduler/dag.rs` (295 lines)

cook-dag is the fully generic DAG data structure. It provides topology management only — no executor, no Cook-specific types. Extract from `scheduler/dag.rs` and make the types generic.

- [ ] **Step 1: Create Cargo.toml**

Create `cli/crates/cook-dag/Cargo.toml`:

```toml
[package]
name = "cook-dag"
version = "0.1.0"
edition = "2024"
description = "Generic DAG data structure with topological traversal"
```

No dependencies — this crate is fully standalone.

- [ ] **Step 2: Write failing test for generic Dag**

Create `cli/crates/cook-dag/src/lib.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_dag() {
        let dag: Dag<String> = Dag::new();
        assert!(dag.is_empty());
        assert_eq!(dag.len(), 0);
        assert!(dag.initial_ready().is_empty());
    }

    #[test]
    fn test_single_node() {
        let mut dag: Dag<String> = Dag::new();
        let id = dag.add_node("task_a".into(), &[]);
        assert_eq!(dag.len(), 1);
        assert_eq!(dag.initial_ready(), vec![id]);
    }

    #[test]
    fn test_linear_chain() {
        let mut dag: Dag<&str> = Dag::new();
        let a = dag.add_node("a", &[]);
        let b = dag.add_node("b", &[a]);
        let c = dag.add_node("c", &[b]);

        assert_eq!(dag.initial_ready(), vec![a]);

        let ready = dag.complete(a);
        assert_eq!(ready, vec![b]);

        let ready = dag.complete(b);
        assert_eq!(ready, vec![c]);

        let ready = dag.complete(c);
        assert!(ready.is_empty());
    }

    #[test]
    fn test_diamond() {
        let mut dag: Dag<&str> = Dag::new();
        let a = dag.add_node("a", &[]);
        let b = dag.add_node("b", &[a]);
        let c = dag.add_node("c", &[a]);
        let d = dag.add_node("d", &[b, c]);

        assert_eq!(dag.initial_ready(), vec![a]);

        let ready = dag.complete(a);
        assert_eq!(ready.len(), 2); // b and c

        let ready = dag.complete(b);
        assert!(ready.is_empty()); // d still waiting on c

        let ready = dag.complete(c);
        assert_eq!(ready, vec![d]);
    }

    #[test]
    fn test_parallel_roots() {
        let mut dag: Dag<&str> = Dag::new();
        let a = dag.add_node("a", &[]);
        let b = dag.add_node("b", &[]);
        let c = dag.add_node("c", &[a, b]);

        let roots = dag.initial_ready();
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn test_node_access() {
        let mut dag: Dag<String> = Dag::new();
        let id = dag.add_node("hello".into(), &[]);
        assert_eq!(dag.node(id).payload, "hello");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cd ~/dev/cook/cli
cargo test -p cook-dag
```

Expected: compilation error — `Dag` not defined yet.

- [ ] **Step 4: Implement generic Dag**

Add implementation above the tests in `cli/crates/cook-dag/src/lib.rs`:

```rust
use std::cell::Cell;

/// A node in the DAG.
pub struct Node<T> {
    pub id: usize,
    pub payload: T,
    pub dependents: Vec<usize>,
    remaining_deps: Cell<usize>,
}

/// A directed acyclic graph with topological traversal support.
///
/// Nodes track dependency counts internally. Call `complete(id)` when a node
/// finishes to decrement dependents and discover newly-ready nodes.
pub struct Dag<T> {
    nodes: Vec<Node<T>>,
}

impl<T> Dag<T> {
    /// Create an empty DAG.
    pub fn new() -> Self {
        Dag { nodes: Vec::new() }
    }

    /// Add a node with the given payload. `depends_on` lists the IDs of nodes
    /// that must complete before this node becomes ready.
    /// Returns the new node's ID.
    pub fn add_node(&mut self, payload: T, depends_on: &[usize]) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            id,
            payload,
            dependents: Vec::new(),
            remaining_deps: Cell::new(depends_on.len()),
        });
        for &dep in depends_on {
            self.nodes[dep].dependents.push(id);
        }
        id
    }

    /// Return IDs of all nodes with zero remaining dependencies.
    pub fn initial_ready(&self) -> Vec<usize> {
        self.nodes
            .iter()
            .filter(|n| n.remaining_deps.get() == 0)
            .map(|n| n.id)
            .collect()
    }

    /// Mark a node as completed. Decrements remaining_deps on all dependents
    /// and returns the IDs of any nodes that became ready.
    pub fn complete(&self, id: usize) -> Vec<usize> {
        let mut newly_ready = Vec::new();
        for &dep_id in &self.nodes[id].dependents {
            let node = &self.nodes[dep_id];
            let old = node.remaining_deps.get();
            node.remaining_deps.set(old - 1);
            if old - 1 == 0 {
                newly_ready.push(dep_id);
            }
        }
        newly_ready
    }

    /// Access a node by ID.
    pub fn node(&self, id: usize) -> &Node<T> {
        &self.nodes[id]
    }

    /// Number of nodes in the DAG.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the DAG has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

impl<T> Default for Dag<T> {
    fn default() -> Self {
        Self::new()
    }
}
```

Note: Uses `Cell<usize>` for `remaining_deps` so `complete()` can take `&self` instead of `&mut self`, matching the pattern in the current codebase where the DAG is shared across threads.

- [ ] **Step 5: Run tests**

```bash
cd ~/dev/cook/cli
cargo test -p cook-dag
```

Expected: all 6 tests pass.

- [ ] **Step 6: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-dag/
git commit -m "feat: add cook-dag crate — generic DAG with topological traversal"
```

---

## Chunk 3: cook-luagen

### Task 3: Create cook-luagen crate

**Files:**
- Create: `cli/crates/cook-luagen/Cargo.toml`
- Create: `cli/crates/cook-luagen/src/lib.rs`
- Create: `cli/crates/cook-luagen/src/recipe.rs`
- Create: `cli/crates/cook-luagen/src/cook_step.rs`
- Create: `cli/crates/cook-luagen/src/plate_step.rs`
- Create: `cli/crates/cook-luagen/src/test_step.rs`
- Create: `cli/crates/cook-luagen/src/template.rs`
- Create: `cli/crates/cook-luagen/src/lua_string.rs`

**Source reference:** `~/dev/remote/github.com/Alex-Gilbert/cook/src/codegen/` (1,026 lines)

**IMPORTANT:** This is the phase where the Lua API unification happens. The old codegen emits `cook.begin_step()`/`cook.end_step()`/`cook.layer()`/`cook.exec()`. The new cook-luagen must emit `cook.step_group(fn)`/`cook.add_unit({...})`/`cook.add_test({...})` instead.

This is the highest-risk transformation in the migration. The codegen must be rewritten, not just copied.

- [ ] **Step 1: Create Cargo.toml**

Create `cli/crates/cook-luagen/Cargo.toml`:

```toml
[package]
name = "cook-luagen"
version = "0.1.0"
edition = "2024"
description = "Cookfile AST to Lua code generator — targets step_group/add_unit API"

[dependencies]
cook-lang = { path = "../cook-lang" }

[dev-dependencies]
```

- [ ] **Step 2: Copy codegen source files as starting point**

```bash
OLD=~/dev/remote/github.com/Alex-Gilbert/cook/src/codegen
NEW=~/dev/cook/cli/crates/cook-luagen/src

mkdir -p $NEW
cp $OLD/recipe.rs $NEW/recipe.rs
cp $OLD/cook_step.rs $NEW/cook_step.rs
cp $OLD/plate_step.rs $NEW/plate_step.rs
cp $OLD/test_step.rs $NEW/test_step.rs
cp $OLD/template.rs $NEW/template.rs
cp $OLD/lua_string.rs $NEW/lua_string.rs
cp $OLD/tests.rs $NEW/tests.rs
```

- [ ] **Step 3: Create lib.rs**

Create `cli/crates/cook-luagen/src/lib.rs`:

```rust
mod recipe;
mod cook_step;
mod plate_step;
mod test_step;
mod template;
mod lua_string;

#[cfg(test)]
mod tests;

use cook_lang::ast::Cookfile;

/// Generate Lua source code from a parsed Cookfile AST.
/// The generated code targets the step_group/add_unit API.
pub fn generate(cookfile: &Cookfile) -> String {
    recipe::generate(cookfile)
}
```

- [ ] **Step 4: Update imports in all files**

Replace all `use crate::parser::ast::*` with `use cook_lang::ast::*` and all `use crate::codegen::*` with `use crate::*` in the copied files.

- [ ] **Step 5: Rewrite cook_step.rs to emit step_group/add_unit**

This is the core rewrite. The old code emits:
```lua
cook.begin_step()
cook.layer(input, output, hash, function()
    cook.exec(cmd)
end)
cook.end_step()
```

The new code must emit:
```lua
cook.step_group(function()
    cook.add_unit({
        inputs = { input },
        output = output,
        command = cmd,
    })
end)
```

Adapt `cook_step.rs` to generate `cook.step_group(function() ... end)` wrapping and `cook.add_unit({inputs = ..., output = ..., command = ...})` calls instead of `cook.layer()`.

Key changes:
- Remove `cook.begin_step()` / `cook.end_step()` — replaced by `cook.step_group(function() ... end)`
- Remove `cook.layer(in, out, hash, fn)` — replaced by `cook.add_unit({inputs, output, command})`
- Remove `cook.exec(cmd)` inside layer bodies — command becomes a field in add_unit table
- Hash computation during registration is no longer codegen's concern — cook-register handles it

- [ ] **Step 6: Rewrite plate_step.rs to emit add_unit**

Old: `cook.exec(cmd)` as a sequential step
New: `cook.add_unit({ inputs = ..., output = nil, command = cmd, cache = false })`

- [ ] **Step 7: Rewrite test_step.rs to emit add_test**

Old: `cook.test_layer(out, hash, timeout, should_fail, fn)` inside `cook.begin_step()/end_step()`
New:
```lua
cook.step_group(function()
    cook.add_test({
        command = cmd,
        timeout = timeout,
        should_fail = false,
    })
end)
```

- [ ] **Step 8: Update tests.rs**

The existing codegen tests (559 lines) assert against the OLD Lua output format. Every assertion must be updated to match the new `step_group`/`add_unit`/`add_test` output format. This is tedious but critical — these tests are the safety net for the rewrite.

Update each test to expect the new format. For example:

Old expected output:
```lua
cook.begin_step()
cook.layer("a.c", "a.o", 12345, function()
    cook.exec("gcc -c a.c -o a.o")
end)
cook.end_step()
```

New expected output:
```lua
cook.step_group(function()
    cook.add_unit({
        inputs = { "a.c" },
        output = "a.o",
        command = "gcc -c a.c -o a.o",
    })
end)
```

- [ ] **Step 9: Run tests**

```bash
cd ~/dev/cook/cli
cargo test -p cook-luagen
```

Expected: all codegen tests pass with the new output format.

- [ ] **Step 10: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-luagen/
git commit -m "feat: add cook-luagen crate — AST to Lua targeting step_group/add_unit API"
```

---

## Chunk 4: Workspace Verification

### Task 4: Update workspace and verify

- [ ] **Step 1: Update workspace Cargo.toml**

Add the three new crates to `cli/Cargo.toml`:

```toml
[workspace]
members = [
    "crates/cook-contracts",
    "crates/cook-lang",
    "crates/cook-progress",
    "crates/cook-cache",
    "crates/cook-dag",
    "crates/cook-luagen",
]
resolver = "2"
```

- [ ] **Step 2: Full workspace build and test**

```bash
cd ~/dev/cook/cli
cargo build
cargo test
```

Expected: all 6 crates compile, all tests pass.

- [ ] **Step 3: Verify dependency isolation**

```bash
cd ~/dev/cook/cli
cargo tree -p cook-dag  # should show ZERO cook-* dependencies
cargo tree -p cook-cache  # should show only cook-contracts
cargo tree -p cook-luagen  # should show only cook-lang
```

- [ ] **Step 4: Commit**

```bash
cd ~/dev/cook
git add cli/Cargo.toml
git commit -m "chore: add cook-cache, cook-dag, cook-luagen to workspace"
```

---

## Completion Criteria

- [ ] cook-cache compiles, tests pass, depends only on cook-contracts
- [ ] cook-dag compiles, tests pass, depends on nothing
- [ ] cook-luagen compiles, tests pass, depends only on cook-lang
- [ ] cook-luagen emits `step_group`/`add_unit`/`add_test` (NOT `begin_step`/`end_step`/`layer`/`exec`)
- [ ] Full workspace (`cargo test`) passes
- [ ] `cargo tree` confirms no unexpected cross-crate dependencies

---

## Review Errata (from plan review)

These issues were identified during plan review. The executing agent MUST address them:

### cook-cache

1. **The type names in lib.rs are WRONG.** `CacheManager`, `CacheChecker`, and `CacheStore` do not exist in the source. The actual types are:
   - `manager.rs` exports: `CacheState`, `SharedCacheState`, `ThreadSafeCacheManager`
   - `check.rs` exports: free functions (`needs_rebuild_cook`, `needs_rebuild_plate`, `stat_mtime`, `hash_file`, `hash_env`, `hash_secondary_inputs`) and enums (`RebuildResult`, `RebuildReason`)
   - `store.rs` exports: `RecipeCache`, `StepEntry`, `FileRecord`, `CACHE_VERSION`

   Fix lib.rs re-exports to match real type names. Also make `check` module `pub` or re-export its public types.

2. **`hash_env` in `check.rs` takes `&HashMap<String, String>`.** Change to `&BTreeMap<String, String>` per the spec's deterministic output convention.

### cook-dag

3. **Missing `validate()` and `CycleError`.** The spec requires `pub fn validate(&self) -> Result<(), CycleError>` for cycle detection. Add a `CycleError` type and implement cycle detection (DFS-based or Kahn's algorithm).

4. **`Cell<usize>` is wrong for `remaining_deps`.** `Cell` is `!Sync` so `Dag<T>` cannot be shared across threads. The current codebase uses `AtomicUsize` for thread safety. Use `AtomicUsize` with `Ordering::SeqCst` to match the existing pattern, or use plain `usize` with `complete(&mut self)` and let the caller handle synchronization.

### cook-luagen

5. **`tests.rs` is missing from the File Structure section.** Add it to the tree diagram.

6. **Shell steps and interactive steps are NOT addressed.** The plan rewrites cook/plate/test steps but does not mention shell steps. Current `recipe.rs` emits `cook.exec(cmd, line)` for shell lines. After the rewrite, these must become `cook.add_unit({ command = cmd, cache = false })`. Interactive steps similarly need rewriting. Add explicit steps for these.

7. **The example output in Step 5 is oversimplified.** Use the spec's full example (with `for` loop over `recipe.ingredients`, `path.stem()` calls, string concatenation) to avoid ambiguity about what the codegen must produce for one-to-one patterns.
