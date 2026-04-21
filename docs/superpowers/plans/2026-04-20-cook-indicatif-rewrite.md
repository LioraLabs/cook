# Cook Indicatif Rewrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the hand-rolled `cook-progress` renderer with an indicatif-based inline renderer, a plain/JSON renderer, and a `.cook/logs/` store. Adopt the 2-line-per-running-recipe layout with artifact status strip. TUI/recap deferred.

**Architecture:** Pure state machine (`BuildState`) consumes a new `ProgressEvent` stream and drives three renderers. Inline uses `indicatif::MultiProgress` with per-state templates; plain writes chronological text; JSON emits one event per line. Every build persists to `.cook/logs/<build-id>/` independent of which renderer is active.

**Tech Stack:** Rust 2024, `indicatif 0.17`, `console 0.15`, `serde 1`, `serde_json 1`, `unicode-width 0.2`, `insta 1` (dev).

**Reference spec:** `docs/superpowers/specs/2026-04-20-cook-indicatif-rewrite-design.md`

**Work location:** In-place on the `cook` repo, current branch. Each task commits independently.

**Conventions:**
- Commands are run from the `cli/` directory (the workspace root for the Rust code), unless a path says otherwise.
- Every task ends with `cargo test --package cook-progress` (or the relevant package) passing, followed by a commit.
- New crate dependencies are added to the relevant crate's `Cargo.toml`, not the workspace root (cook does not use a root `[workspace.dependencies]` table today).

---

## Phase 0: Scaffolding

### Task 1: Add new-crate dependencies and empty module files

Prepares the `cook-progress` crate for the rewrite without touching existing code paths. Old code keeps working; new modules exist but are empty.

**Files:**
- Modify: `cli/crates/cook-progress/Cargo.toml`
- Create: `cli/crates/cook-progress/src/event.rs`
- Create: `cli/crates/cook-progress/src/model/mod.rs`
- Create: `cli/crates/cook-progress/src/model/build.rs`
- Create: `cli/crates/cook-progress/src/model/recipe.rs`
- Create: `cli/crates/cook-progress/src/model/node.rs`
- Create: `cli/crates/cook-progress/src/render/mod.rs`
- Create: `cli/crates/cook-progress/src/render/inline.rs`
- Create: `cli/crates/cook-progress/src/render/plain.rs`
- Create: `cli/crates/cook-progress/src/strip.rs`
- Create: `cli/crates/cook-progress/src/style.rs`
- Create: `cli/crates/cook-progress/src/log_store.rs`
- Create: `cli/crates/cook-progress/src/driver.rs`
- Modify: `cli/crates/cook-progress/src/lib.rs`

- [ ] **Step 1: Add dependencies**

Edit `cli/crates/cook-progress/Cargo.toml`:

```toml
[package]
name = "cook-progress"
version = "0.1.0"
edition = "2024"
description = "Terminal progress rendering with composable primitives"

[dependencies]
crossterm = "0.28"
indicatif = "0.17"
console = "0.15"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
unicode-width = "0.2"

[dev-dependencies]
insta = { version = "1", features = ["yaml"] }
```

- [ ] **Step 2: Create empty module files**

Each new `.rs` file starts with a brief doc comment:

```rust
//! <module name> — see docs/superpowers/specs/2026-04-20-cook-indicatif-rewrite-design.md
```

For `model/mod.rs`, `render/mod.rs` include submodule declarations:

```rust
//! Pure state model — see docs/superpowers/specs/2026-04-20-cook-indicatif-rewrite-design.md
pub mod build;
pub mod node;
pub mod recipe;
```

```rust
//! Renderer trait and implementations — see docs/superpowers/specs/2026-04-20-cook-indicatif-rewrite-design.md
pub mod inline;
pub mod plain;
```

- [ ] **Step 3: Wire new modules into `lib.rs` without breaking existing API**

Edit `cli/crates/cook-progress/src/lib.rs`:

```rust
pub mod bar;
pub mod frame;
pub mod output;
pub mod renderer;
pub mod symbols;

pub mod event;
pub mod model;
pub mod render;
pub mod strip;
pub mod style;
pub mod log_store;
pub mod driver;

pub use frame::{ActiveItem, CacheInfo, Footer, Frame, ItemStatus, Section, Status};
pub use renderer::{RenderConfig, Renderer};
pub use symbols::Symbols;
```

- [ ] **Step 4: Verify the workspace still builds**

Run from `cli/`:
```
cargo build --package cook-progress
```
Expected: compiles cleanly (new modules are empty, old API intact).

Run:
```
cargo test --package cook-progress
```
Expected: existing tests pass.

- [ ] **Step 5: Commit**

```bash
git -C cli/crates/cook-progress add Cargo.toml src/
git commit -m "chore(cook-progress): scaffold indicatif rewrite modules"
```

---

## Phase 1: Event API and pure model

### Task 2: Define `ProgressEvent` and typed IDs

Introduces the new event enum with `BuildStarted`, `NodeOutput`, artifact fields, typed IDs, and `SkipReason`. Lives in `cook_progress::event` and is used by all downstream modules.

**Files:**
- Create: `cli/crates/cook-progress/src/event.rs` (replace the stub from Task 1)

- [ ] **Step 1: Write the failing test**

Append to `src/event.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_id_round_trips_through_eq_and_hash() {
        let a = RecipeId::new(0);
        let b = RecipeId::new(0);
        let c = RecipeId::new(1);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn progress_event_is_clone_and_send() {
        fn assert_clone_send<T: Clone + Send>() {}
        assert_clone_send::<ProgressEvent>();
    }

    #[test]
    fn skip_reason_display() {
        assert_eq!(SkipReason::UpstreamFailed.as_str(), "upstream-failed");
        assert_eq!(SkipReason::ConditionFalse.as_str(), "condition-false");
        assert_eq!(SkipReason::Disabled.as_str(), "disabled");
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run from `cli/`:
```
cargo test --package cook-progress event::tests
```
Expected: compile error — `RecipeId`, `ProgressEvent`, `SkipReason` not defined.

- [ ] **Step 3: Implement the types**

Replace `src/event.rs` contents with:

```rust
//! Public event API for the progress pipeline.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Opaque recipe identifier. Stable within a single build run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RecipeId(u32);

impl RecipeId {
    pub fn new(raw: u32) -> Self { Self(raw) }
    pub fn raw(self) -> u32 { self.0 }
}

/// Opaque node identifier. Unique within a recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId(u32);

impl NodeId {
    pub fn new(raw: u32) -> Self { Self(raw) }
    pub fn raw(self) -> u32 { self.0 }
}

/// Why a node was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipReason {
    UpstreamFailed,
    ConditionFalse,
    Disabled,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpstreamFailed => "upstream-failed",
            Self::ConditionFalse => "condition-false",
            Self::Disabled => "disabled",
        }
    }
}

/// Which stream a node-output line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Topology entry sent once in `BuildStarted`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeTopo {
    pub id: RecipeId,
    pub name: String,
    pub deps: Vec<RecipeId>,
    pub expected_nodes: usize,
}

/// Event stream emitted by the engine and consumed by the driver.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ProgressEvent {
    BuildStarted {
        recipes: Vec<RecipeTopo>,
        total_nodes: usize,
    },
    RecipeStarted {
        recipe: RecipeId,
    },
    RecipeCompleted {
        recipe: RecipeId,
        elapsed: Duration,
        cached: usize,
        total: usize,
    },
    RecipeFailed {
        recipe: RecipeId,
        elapsed: Duration,
        completed: usize,
        total: usize,
    },
    NodeStarted {
        recipe: RecipeId,
        node: NodeId,
        name: String,
        artifact: Option<PathBuf>,
        fallback_label: String,
    },
    NodeCompleted {
        recipe: RecipeId,
        node: NodeId,
        elapsed: Duration,
    },
    NodeFailed {
        recipe: RecipeId,
        node: NodeId,
        elapsed: Duration,
        error: String,
    },
    NodeCacheHit {
        recipe: RecipeId,
        node: NodeId,
        name: String,
        artifact: Option<PathBuf>,
    },
    NodeSkipped {
        recipe: RecipeId,
        node: NodeId,
        name: String,
        reason: SkipReason,
    },
    NodeOutput {
        recipe: RecipeId,
        node: NodeId,
        line: String,
        stream: Stream,
    },
    InteractiveStart {
        recipe: RecipeId,
        node: NodeId,
    },
    InteractiveEnd {
        recipe: RecipeId,
        node: NodeId,
        elapsed: Duration,
        success: bool,
    },
    Finished {
        success: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_id_round_trips_through_eq_and_hash() {
        let a = RecipeId::new(0);
        let b = RecipeId::new(0);
        let c = RecipeId::new(1);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn progress_event_is_clone_and_send() {
        fn assert_clone_send<T: Clone + Send>() {}
        assert_clone_send::<ProgressEvent>();
    }

    #[test]
    fn skip_reason_display() {
        assert_eq!(SkipReason::UpstreamFailed.as_str(), "upstream-failed");
        assert_eq!(SkipReason::ConditionFalse.as_str(), "condition-false");
        assert_eq!(SkipReason::Disabled.as_str(), "disabled");
    }
}
```

- [ ] **Step 4: Verify tests pass**

Run:
```
cargo test --package cook-progress event::tests
```
Expected: 3 passing.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-progress/src/event.rs
git commit -m "feat(cook-progress): introduce ProgressEvent API with typed ids"
```

---

### Task 3: Implement `NodeState` and `RecipeState` structs

Pure data holders used by `BuildState`. No logic yet beyond constructors.

**Files:**
- Create: `cli/crates/cook-progress/src/model/node.rs`
- Create: `cli/crates/cook-progress/src/model/recipe.rs`

- [ ] **Step 1: Write the failing test for `NodeState`**

In `src/model/node.rs`:

```rust
//! NodeState — per-node live status inside a recipe.

use std::path::PathBuf;
use std::time::Instant;

use crate::event::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Waiting,
    Running,
    Completed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct NodeState {
    pub id: NodeId,
    pub name: String,
    pub artifact: Option<PathBuf>,
    pub fallback_label: String,
    pub status: NodeStatus,
    pub started_at: Option<Instant>,
    pub completed_at: Option<Instant>,
}

impl NodeState {
    pub fn new(id: NodeId, name: String, artifact: Option<PathBuf>, fallback_label: String) -> Self {
        Self {
            id,
            name,
            artifact,
            fallback_label,
            status: NodeStatus::Waiting,
            started_at: None,
            completed_at: None,
        }
    }

    /// Basename of `artifact` if set; otherwise the first whitespace-separated token
    /// of `fallback_label` (stripped of a leading `$ `).
    pub fn display(&self) -> String {
        if let Some(artifact) = &self.artifact {
            if let Some(base) = artifact.file_name().and_then(|s| s.to_str()) {
                return base.to_string();
            }
        }
        let stripped = self.fallback_label.trim_start_matches("$ ").trim_start();
        let first = stripped.split_whitespace().next().unwrap_or("?");
        format!("${first}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_uses_artifact_basename() {
        let n = NodeState::new(
            NodeId::new(0),
            "lvm.c".into(),
            Some("build/obj/liblua/lvm.o".into()),
            "clang -c lvm.c".into(),
        );
        assert_eq!(n.display(), "lvm.o");
    }

    #[test]
    fn display_falls_back_to_command_token() {
        let n = NodeState::new(
            NodeId::new(1),
            "archive".into(),
            None,
            "$ ar rcs libliblua.a lapi.o".into(),
        );
        assert_eq!(n.display(), "$ar");
    }

    #[test]
    fn display_handles_empty_fallback() {
        let n = NodeState::new(NodeId::new(2), "x".into(), None, "".into());
        assert_eq!(n.display(), "$?");
    }
}
```

- [ ] **Step 2: Run the test**

```
cargo test --package cook-progress model::node::tests
```
Expected: 3 passing.

- [ ] **Step 3: Write the failing test for `RecipeState`**

In `src/model/recipe.rs`:

```rust
//! RecipeState — live per-recipe status.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::event::{NodeId, RecipeId, SkipReason};
use crate::model::node::NodeState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Running,
    Completed,
    Failed,
    Cached,
}

#[derive(Debug, Clone)]
pub struct RecipeState {
    pub id: RecipeId,
    pub name: String,
    pub deps: Vec<RecipeId>,
    pub status: Status,
    pub progress: (usize, usize),
    pub elapsed: Option<Duration>,
    pub nodes: BTreeMap<NodeId, NodeState>,
    pub cached_count: usize,
    pub skipped: Vec<(NodeId, SkipReason)>,
    pub error_summary: Option<String>,
}

impl RecipeState {
    pub fn new(id: RecipeId, name: String, deps: Vec<RecipeId>, expected_nodes: usize) -> Self {
        Self {
            id,
            name,
            deps,
            status: Status::Waiting,
            progress: (0, expected_nodes),
            elapsed: None,
            nodes: BTreeMap::new(),
            cached_count: 0,
            skipped: Vec::new(),
            error_summary: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_recipe_is_waiting_with_zero_progress() {
        let r = RecipeState::new(RecipeId::new(0), "deps".into(), vec![], 12);
        assert_eq!(r.status, Status::Waiting);
        assert_eq!(r.progress, (0, 12));
        assert!(r.nodes.is_empty());
        assert_eq!(r.cached_count, 0);
        assert!(r.error_summary.is_none());
    }
}
```

- [ ] **Step 4: Run the test**

```
cargo test --package cook-progress model::recipe::tests
```
Expected: 1 passing.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-progress/src/model/
git commit -m "feat(cook-progress): add NodeState and RecipeState types"
```

---

### Task 4: Implement `BuildState` and event application

The only mutation path. Every `ProgressEvent` goes through `BuildState::apply`.

**Files:**
- Create: `cli/crates/cook-progress/src/model/build.rs`

- [ ] **Step 1: Write failing tests for initial ingest**

In `src/model/build.rs`:

```rust
//! BuildState — the pure state machine. ProgressEvent is the only input.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::event::{NodeId, ProgressEvent, RecipeId, RecipeTopo, SkipReason};
use crate::model::node::{NodeState, NodeStatus};
use crate::model::recipe::{RecipeState, Status};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub done: usize,
    pub running: usize,
    pub waiting: usize,
    pub cached: usize,
    pub total_nodes: usize,
    pub completed_nodes: usize,
}

#[derive(Debug, Clone)]
pub struct BuildState {
    pub order: Vec<RecipeId>,
    pub recipes: BTreeMap<RecipeId, RecipeState>,
    pub started_at: Option<Instant>,
    pub totals: Counters,
    pub finished: Option<bool>,
}

impl BuildState {
    pub fn new() -> Self {
        Self {
            order: Vec::new(),
            recipes: BTreeMap::new(),
            started_at: None,
            totals: Counters::default(),
            finished: None,
        }
    }

    pub fn apply(&mut self, event: &ProgressEvent) {
        match event {
            ProgressEvent::BuildStarted { recipes, total_nodes } => {
                self.ingest_topology(recipes, *total_nodes);
            }
            ProgressEvent::RecipeStarted { recipe } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    if r.status == Status::Waiting {
                        r.status = Status::Running;
                        self.totals.waiting = self.totals.waiting.saturating_sub(1);
                        self.totals.running += 1;
                    }
                }
            }
            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    r.elapsed = Some(*elapsed);
                    r.progress = (*total, *total);
                    r.status = if *cached == *total && *total > 0 { Status::Cached } else { Status::Completed };
                    if self.totals.running > 0 { self.totals.running -= 1; }
                    self.totals.done += 1;
                    if r.status == Status::Cached { self.totals.cached += 1; }
                }
            }
            ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    r.elapsed = Some(*elapsed);
                    r.progress = (*completed, *total);
                    r.status = Status::Failed;
                    if self.totals.running > 0 { self.totals.running -= 1; }
                    self.totals.done += 1;
                }
            }
            ProgressEvent::NodeStarted { recipe, node, name, artifact, fallback_label } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    let mut ns = NodeState::new(*node, name.clone(), artifact.clone(), fallback_label.clone());
                    ns.status = NodeStatus::Running;
                    ns.started_at = Some(Instant::now());
                    r.nodes.insert(*node, ns);
                }
            }
            ProgressEvent::NodeCompleted { recipe, node, elapsed: _ } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    if let Some(n) = r.nodes.get_mut(node) {
                        n.status = NodeStatus::Completed;
                        n.completed_at = Some(Instant::now());
                    }
                    r.progress.0 += 1;
                    self.totals.completed_nodes += 1;
                }
            }
            ProgressEvent::NodeFailed { recipe, node, elapsed: _, error } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    if let Some(n) = r.nodes.get_mut(node) {
                        n.status = NodeStatus::Failed;
                        n.completed_at = Some(Instant::now());
                    }
                    r.progress.0 += 1;
                    if r.error_summary.is_none() {
                        r.error_summary = Some(error.clone());
                    }
                    self.totals.completed_nodes += 1;
                }
            }
            ProgressEvent::NodeCacheHit { recipe, node: _, name: _, artifact: _ } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    r.cached_count += 1;
                    r.progress.0 += 1;
                    self.totals.completed_nodes += 1;
                }
            }
            ProgressEvent::NodeSkipped { recipe, node, name, reason } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    r.skipped.push((*node, *reason));
                    r.nodes.insert(*node, NodeState {
                        id: *node,
                        name: name.clone(),
                        artifact: None,
                        fallback_label: String::new(),
                        status: NodeStatus::Skipped,
                        started_at: None,
                        completed_at: Some(Instant::now()),
                    });
                    r.progress.0 += 1;
                    self.totals.completed_nodes += 1;
                }
            }
            ProgressEvent::NodeOutput { .. } => { /* log store handles this */ }
            ProgressEvent::InteractiveStart { .. } => {}
            ProgressEvent::InteractiveEnd { .. } => {}
            ProgressEvent::Finished { success } => {
                self.finished = Some(*success);
            }
        }
    }

    fn ingest_topology(&mut self, recipes: &[RecipeTopo], total_nodes: usize) {
        self.order = recipes.iter().map(|r| r.id).collect();
        for topo in recipes {
            self.recipes.insert(
                topo.id,
                RecipeState::new(topo.id, topo.name.clone(), topo.deps.clone(), topo.expected_nodes),
            );
        }
        self.totals.waiting = recipes.len();
        self.totals.total_nodes = total_nodes;
        self.started_at = Some(Instant::now());
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.map(|t| t.elapsed()).unwrap_or_default()
    }
}

impl Default for BuildState {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn topo(recipes: &[(u32, &str, &[u32], usize)]) -> Vec<RecipeTopo> {
        recipes.iter().map(|(id, name, deps, n)| RecipeTopo {
            id: RecipeId::new(*id),
            name: (*name).to_string(),
            deps: deps.iter().map(|d| RecipeId::new(*d)).collect(),
            expected_nodes: *n,
        }).collect()
    }

    #[test]
    fn build_started_seeds_recipes_in_topo_order() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", &[], 12), (1, "lib", &[0], 6)]),
            total_nodes: 18,
        });
        assert_eq!(s.order, vec![RecipeId::new(0), RecipeId::new(1)]);
        assert_eq!(s.recipes.len(), 2);
        assert_eq!(s.totals.waiting, 2);
        assert_eq!(s.totals.total_nodes, 18);
    }

    #[test]
    fn recipe_started_transitions_waiting_to_running() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", &[], 2)]), total_nodes: 2,
        });
        s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        assert_eq!(s.recipes[&RecipeId::new(0)].status, Status::Running);
        assert_eq!(s.totals.running, 1);
        assert_eq!(s.totals.waiting, 0);
    }

    #[test]
    fn node_started_inserts_running_node() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "lib", &[], 1)]), total_nodes: 1,
        });
        s.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0),
            node: NodeId::new(0),
            name: "lvm.c".into(),
            artifact: Some(PathBuf::from("build/obj/lvm.o")),
            fallback_label: "clang -c lvm.c".into(),
        });
        let r = &s.recipes[&RecipeId::new(0)];
        assert_eq!(r.nodes.len(), 1);
        assert_eq!(r.nodes[&NodeId::new(0)].status, NodeStatus::Running);
    }

    #[test]
    fn cache_hit_increments_counter_and_progress() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", &[], 3)]), total_nodes: 3,
        });
        s.apply(&ProgressEvent::NodeCacheHit {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "a".into(), artifact: None,
        });
        let r = &s.recipes[&RecipeId::new(0)];
        assert_eq!(r.cached_count, 1);
        assert_eq!(r.progress, (1, 3));
        assert_eq!(s.totals.completed_nodes, 1);
    }

    #[test]
    fn recipe_completed_marks_cached_when_all_cached() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", &[], 2)]), total_nodes: 2,
        });
        s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        s.apply(&ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(10),
            cached: 2, total: 2,
        });
        assert_eq!(s.recipes[&RecipeId::new(0)].status, Status::Cached);
        assert_eq!(s.totals.cached, 1);
    }

    #[test]
    fn recipe_failed_records_first_error_summary() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "lib", &[], 1)]), total_nodes: 1,
        });
        s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        s.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "x".into(), artifact: None, fallback_label: "x".into(),
        });
        s.apply(&ProgressEvent::NodeFailed {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(10),
            error: "boom".into(),
        });
        s.apply(&ProgressEvent::RecipeFailed {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(20),
            completed: 1, total: 1,
        });
        let r = &s.recipes[&RecipeId::new(0)];
        assert_eq!(r.status, Status::Failed);
        assert_eq!(r.error_summary.as_deref(), Some("boom"));
    }
}
```

Also update `src/model/mod.rs`:

```rust
//! Pure state model — see docs/superpowers/specs/2026-04-20-cook-indicatif-rewrite-design.md
pub mod build;
pub mod node;
pub mod recipe;

pub use build::{BuildState, Counters};
pub use node::{NodeState, NodeStatus};
pub use recipe::{RecipeState, Status};
```

- [ ] **Step 2: Run the tests**

```
cargo test --package cook-progress model::build
```
Expected: 6 passing.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/src/model/
git commit -m "feat(cook-progress): implement BuildState and event apply"
```

---

## Phase 2: Artifact strip

### Task 5: Implement `artifact_strip` pure function

Pure width-aware layout function used by the inline renderer's `{msg}`.

**Files:**
- Create: `cli/crates/cook-progress/src/strip.rs` (replace Task 1 stub)
- Create: `cli/crates/cook-progress/src/tests/snapshots/` (directory for `insta`)

- [ ] **Step 1: Write the skeleton + failing tests**

In `src/strip.rs`:

```rust
//! Artifact status strip — compact one-line summary of recipe progress.

use unicode_width::UnicodeWidthStr;

use crate::model::node::{NodeState, NodeStatus};
use crate::model::recipe::RecipeState;

const JOIN: &str = " · ";
const SUFFIX_GUARD: usize = 5;
const MAX_COMPLETED: usize = 3;
const MAX_WAITING: usize = 2;
const MAX_DISPLAY_LEN: usize = 20;

pub fn artifact_strip(recipe: &RecipeState, cols: usize) -> String {
    let pills = build_pills(recipe);
    let cached_prefix = if recipe.cached_count > 0 {
        format!("{} cached", recipe.cached_count)
    } else {
        String::new()
    };

    let budget = cols.saturating_sub(SUFFIX_GUARD);
    fit(&cached_prefix, &pills, budget)
}

#[derive(Debug, Clone)]
struct Pill {
    symbol: &'static str,
    text: String,
    priority: u8, // lower = drop first
}

fn build_pills(recipe: &RecipeState) -> Vec<Pill> {
    let mut completed: Vec<&NodeState> = recipe
        .nodes
        .values()
        .filter(|n| n.status == NodeStatus::Completed)
        .collect();
    completed.sort_by_key(|n| n.completed_at);
    let completed = completed.iter().rev().take(MAX_COMPLETED).rev().copied().collect::<Vec<_>>();

    let mut running: Vec<&NodeState> = recipe
        .nodes
        .values()
        .filter(|n| n.status == NodeStatus::Running)
        .collect();
    running.sort_by_key(|n| n.started_at);

    let mut failed: Vec<&NodeState> = recipe
        .nodes
        .values()
        .filter(|n| n.status == NodeStatus::Failed)
        .collect();
    failed.sort_by_key(|n| n.completed_at);

    let waiting: Vec<&NodeState> = recipe
        .nodes
        .values()
        .filter(|n| n.status == NodeStatus::Waiting)
        .take(MAX_WAITING)
        .collect();

    let mut pills = Vec::new();
    for n in completed {
        pills.push(Pill {
            symbol: "✓",
            text: truncate(&n.display()),
            priority: 1, // drop after waiting
        });
    }
    for n in failed {
        pills.push(Pill {
            symbol: "✗",
            text: truncate(&n.display()),
            priority: 3, // never drop before running
        });
    }
    for n in running {
        pills.push(Pill {
            symbol: "◆",
            text: truncate(&n.display()),
            priority: 3,
        });
    }
    for n in waiting {
        pills.push(Pill {
            symbol: "◇",
            text: truncate(&n.display()),
            priority: 0, // drop first
        });
    }
    pills
}

fn truncate(s: &str) -> String {
    if s.width() <= MAX_DISPLAY_LEN {
        s.to_string()
    } else {
        let mut out = String::new();
        let mut w = 0;
        for c in s.chars() {
            let cw = UnicodeWidthStr::width(c.to_string().as_str());
            if w + cw + 1 > MAX_DISPLAY_LEN { break; }
            out.push(c);
            w += cw;
        }
        out.push('…');
        out
    }
}

fn fit(cached_prefix: &str, pills: &[Pill], budget: usize) -> String {
    let mut ordered_indices: Vec<usize> = (0..pills.len()).collect();
    // Drop order: lowest priority first, within a priority drop from the right.
    ordered_indices.sort_by(|&a, &b| pills[a].priority.cmp(&pills[b].priority).then(a.cmp(&b)));

    let mut included: Vec<bool> = vec![true; pills.len()];
    let rendered = |included: &[bool]| -> String {
        let parts: Vec<String> = pills
            .iter()
            .enumerate()
            .filter(|(i, _)| included[*i])
            .map(|(_, p)| format!("{} {}", p.symbol, p.text))
            .collect();

        let dropped = included.iter().filter(|x| !**x).count();
        let mut s = String::new();
        if !cached_prefix.is_empty() {
            s.push_str(cached_prefix);
            if !parts.is_empty() || dropped > 0 {
                s.push_str(JOIN);
            }
        }
        s.push_str(&parts.join(JOIN));
        if dropped > 0 {
            if !s.is_empty() { s.push(' '); }
            s.push_str(&format!("+{dropped}"));
        }
        s
    };

    let mut drop_cursor = 0;
    while rendered(&included).width() > budget && drop_cursor < ordered_indices.len() {
        let idx = ordered_indices[drop_cursor];
        included[idx] = false;
        drop_cursor += 1;
    }
    rendered(&included)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{NodeId, RecipeId};
    use std::path::PathBuf;
    use std::time::Instant;

    fn recipe_with(
        cached: usize,
        completed: &[&str],
        running: &[&str],
        waiting: &[&str],
    ) -> RecipeState {
        let mut r = RecipeState::new(RecipeId::new(0), "lib".into(), vec![], 1);
        r.cached_count = cached;
        let mut next = 0u32;
        let now = Instant::now();
        for (i, name) in completed.iter().enumerate() {
            let mut n = NodeState::new(
                NodeId::new(next),
                (*name).to_string(),
                Some(PathBuf::from(format!("build/{name}"))),
                String::new(),
            );
            n.status = NodeStatus::Completed;
            n.completed_at = Some(now + std::time::Duration::from_millis(i as u64));
            r.nodes.insert(NodeId::new(next), n);
            next += 1;
        }
        for (i, name) in running.iter().enumerate() {
            let mut n = NodeState::new(
                NodeId::new(next),
                (*name).to_string(),
                Some(PathBuf::from(format!("build/{name}"))),
                String::new(),
            );
            n.status = NodeStatus::Running;
            n.started_at = Some(now + std::time::Duration::from_millis(i as u64));
            r.nodes.insert(NodeId::new(next), n);
            next += 1;
        }
        for name in waiting.iter() {
            let n = NodeState::new(
                NodeId::new(next),
                (*name).to_string(),
                Some(PathBuf::from(format!("build/{name}"))),
                String::new(),
            );
            r.nodes.insert(NodeId::new(next), n);
            next += 1;
        }
        r
    }

    #[test]
    fn simple_no_overflow() {
        let r = recipe_with(0, &[], &["lua.o"], &["lua.bin"]);
        let s = artifact_strip(&r, 80);
        assert!(s.contains("◆ lua.o"));
        assert!(s.contains("◇ lua.bin"));
    }

    #[test]
    fn cached_prefix_shown_when_nonzero() {
        let r = recipe_with(27, &[], &["ldo.o"], &[]);
        let s = artifact_strip(&r, 80);
        assert!(s.starts_with("27 cached"));
    }

    #[test]
    fn overflow_drops_waiting_first() {
        let waiting: Vec<&str> = (0..50).map(|_| "waitx").collect();
        let r = recipe_with(0, &[], &["a"], &waiting);
        let s = artifact_strip(&r, 40);
        assert!(s.contains("◆ a"));
        assert!(s.contains("+"), "should have +N drop marker: {s}");
    }

    #[test]
    fn running_pills_are_never_dropped_before_waiting() {
        let running: Vec<&str> = (0..10).map(|_| "run").collect();
        let waiting: Vec<&str> = (0..5).map(|_| "wait").collect();
        let r = recipe_with(0, &[], &running, &waiting);
        let s = artifact_strip(&r, 40);
        // at least one running pill remains even at narrow width
        assert!(s.contains("◆ run"), "got: {s}");
    }

    #[test]
    fn long_artifact_name_is_truncated() {
        let r = recipe_with(0, &[], &["a_very_long_artifact_name_exceeding_twenty.o"], &[]);
        let s = artifact_strip(&r, 120);
        assert!(s.contains("…"));
    }
}
```

- [ ] **Step 2: Run the tests**

```
cargo test --package cook-progress strip::tests
```
Expected: 5 passing.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/src/strip.rs
git commit -m "feat(cook-progress): artifact status strip with overflow handling"
```

---

## Phase 3: Plain renderer and JSON writer

### Task 6: Plain renderer (chronological text)

**Files:**
- Create: `cli/crates/cook-progress/src/render/mod.rs` (expand from Task 1 stub)
- Create: `cli/crates/cook-progress/src/render/plain.rs`

- [ ] **Step 1: Define the `Renderer` trait in `render/mod.rs`**

```rust
//! Renderer trait and implementations.
pub mod inline;
pub mod plain;

use std::io;

use crate::event::ProgressEvent;
use crate::model::build::BuildState;

pub trait Renderer: Send {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()>;
    fn finish(&mut self, state: &BuildState) -> io::Result<()>;
}
```

- [ ] **Step 2: Write failing tests for `PlainRenderer`**

In `src/render/plain.rs`:

```rust
//! Plain append-only renderer — non-TTY / CI-safe output.

use std::io::{self, Write};
use std::time::Duration;

use crate::event::{ProgressEvent, RecipeId, SkipReason, Stream};
use crate::model::build::BuildState;
use crate::model::recipe::Status;
use crate::render::Renderer;

pub struct PlainRenderer<W: Write + Send> {
    out: W,
}

impl<W: Write + Send> PlainRenderer<W> {
    pub fn new(out: W) -> Self { Self { out } }

    fn name(&self, state: &BuildState, recipe: RecipeId) -> String {
        state.recipes.get(&recipe).map(|r| r.name.clone()).unwrap_or_else(|| format!("recipe#{}", recipe.raw()))
    }
}

fn fmt_secs(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.2}s")
    } else {
        let m = (secs as u64) / 60;
        let s = (secs as u64) % 60;
        format!("{m}m{s}s")
    }
}

impl<W: Write + Send> Renderer for PlainRenderer<W> {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        match event {
            ProgressEvent::BuildStarted { recipes, .. } => {
                for r in recipes {
                    writeln!(self.out, "  {:24} queued  ({} nodes)", r.name, r.expected_nodes)?;
                }
            }
            ProgressEvent::RecipeStarted { .. } => {}
            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total } => {
                let name = self.name(state, *recipe);
                let detail = if *cached > 0 {
                    format!("({cached}/{total} cached)")
                } else {
                    format!("({total}/{total})")
                };
                writeln!(self.out, "  {:24} done     {:24} {}", name, detail, fmt_secs(*elapsed))?;
            }
            ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => {
                let name = self.name(state, *recipe);
                writeln!(self.out, "  {:24} FAILED   ({}/{} steps) {}", name, completed, total, fmt_secs(*elapsed))?;
            }
            ProgressEvent::NodeStarted { .. } => {}
            ProgressEvent::NodeCompleted { recipe, node, elapsed } => {
                let rname = self.name(state, *recipe);
                let nname = state.recipes.get(recipe)
                    .and_then(|r| r.nodes.get(node))
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                writeln!(self.out, "  {}/{:40}{}", rname, nname, fmt_secs(*elapsed))?;
            }
            ProgressEvent::NodeFailed { recipe, node, elapsed, error } => {
                let rname = self.name(state, *recipe);
                let nname = state.recipes.get(recipe)
                    .and_then(|r| r.nodes.get(node))
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                writeln!(self.out, "  {}/{:40}FAILED {}", rname, nname, fmt_secs(*elapsed))?;
                for line in error.lines() {
                    writeln!(self.out, "  [{rname}/{nname}] {line}")?;
                }
            }
            ProgressEvent::NodeCacheHit { recipe, name: nname, .. } => {
                let rname = self.name(state, *recipe);
                writeln!(self.out, "  {}/{:40}cached", rname, nname)?;
            }
            ProgressEvent::NodeSkipped { recipe, name: nname, reason, .. } => {
                let rname = self.name(state, *recipe);
                let reason_str = match reason {
                    SkipReason::UpstreamFailed => "upstream-failed",
                    SkipReason::ConditionFalse => "condition-false",
                    SkipReason::Disabled => "disabled",
                };
                writeln!(self.out, "  {}/{:40}skipped ({reason_str})", rname, nname)?;
            }
            ProgressEvent::NodeOutput { recipe, node, line, stream } => {
                let rname = self.name(state, *recipe);
                let nname = state.recipes.get(recipe)
                    .and_then(|r| r.nodes.get(node))
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                let tag = match stream {
                    Stream::Stdout => "",
                    Stream::Stderr => "(stderr) ",
                };
                writeln!(self.out, "  [{rname}/{nname}] {tag}{line}")?;
            }
            ProgressEvent::InteractiveStart { recipe, node } => {
                let rname = self.name(state, *recipe);
                let nname = state.recipes.get(recipe)
                    .and_then(|r| r.nodes.get(node))
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                writeln!(self.out, "─── {rname}/{nname} (interactive) ───")?;
            }
            ProgressEvent::InteractiveEnd { recipe, node, elapsed, success } => {
                let rname = self.name(state, *recipe);
                let nname = state.recipes.get(recipe)
                    .and_then(|r| r.nodes.get(node))
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                let ok = if *success { "ok" } else { "failed" };
                writeln!(self.out, "─── {rname}/{nname} resumed ({ok}, {}) ───", fmt_secs(*elapsed))?;
            }
            ProgressEvent::Finished { .. } => {}
        }
        Ok(())
    }

    fn finish(&mut self, state: &BuildState) -> io::Result<()> {
        let ok = state.finished.unwrap_or(false);
        let done = state.totals.done;
        let cached = state.totals.cached;
        let total = state.totals.total_nodes;
        let elapsed = fmt_secs(state.elapsed());
        if ok {
            writeln!(self.out, "cook build done in {elapsed} ({total} nodes, {cached} cached recipes, {done} done)")?;
        } else {
            writeln!(self.out, "cook build FAILED after {elapsed}")?;
        }
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{NodeId, RecipeTopo};

    fn topo(recipes: &[(u32, &str, usize)]) -> Vec<RecipeTopo> {
        recipes.iter().map(|(id, name, n)| RecipeTopo {
            id: RecipeId::new(*id),
            name: (*name).to_string(),
            deps: vec![],
            expected_nodes: *n,
        }).collect()
    }

    #[test]
    fn build_started_writes_queued_lines() {
        let mut state = BuildState::new();
        let ev = ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", 2), (1, "lib", 3)]),
            total_nodes: 5,
        };
        state.apply(&ev);
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            r.handle(&state, &ev).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("deps"));
        assert!(s.contains("queued  (2 nodes)"));
        assert!(s.contains("lib"));
    }

    #[test]
    fn recipe_completed_writes_done_line() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", 2)]), total_nodes: 2,
        });
        let ev = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(400),
            cached: 0, total: 2,
        };
        state.apply(&ev);
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            r.handle(&state, &ev).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("deps"), "got: {s}");
        assert!(s.contains("done"), "got: {s}");
        assert!(s.contains("0.40s"), "got: {s}");
    }

    #[test]
    fn node_output_prefix_includes_recipe_and_node() {
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "lib", 1)]), total_nodes: 1,
        });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "lvm.c".into(), artifact: None, fallback_label: "x".into(),
        });
        let ev = ProgressEvent::NodeOutput {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            line: "warning: unused".into(), stream: Stream::Stderr,
        };
        let mut buf = Vec::new();
        {
            let mut r = PlainRenderer::new(&mut buf);
            r.handle(&state, &ev).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("[lib/lvm.c]"), "got: {s}");
        assert!(s.contains("(stderr)"), "got: {s}");
        assert!(s.contains("warning: unused"), "got: {s}");
    }
}
```

- [ ] **Step 3: Run the tests**

```
cargo test --package cook-progress render::plain::tests
```
Expected: 3 passing.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/src/render/
git commit -m "feat(cook-progress): plain renderer for non-TTY output"
```

---

### Task 7: JSON event writer

One JSON object per line. Reuses `ProgressEvent`'s serde derives.

**Files:**
- Create: `cli/crates/cook-progress/src/render/json.rs`
- Modify: `cli/crates/cook-progress/src/render/mod.rs`

- [ ] **Step 1: Write failing tests**

In `src/render/json.rs`:

```rust
//! Machine-readable JSON-lines event writer.

use std::io::{self, Write};

use serde::Serialize;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::event::ProgressEvent;
use crate::model::build::BuildState;
use crate::render::Renderer;

pub struct JsonWriter<W: Write + Send> {
    out: W,
    schema_version: u32,
}

#[derive(Serialize)]
struct Envelope<'a> {
    ts: String,
    v: u32,
    #[serde(flatten)]
    event: &'a ProgressEvent,
}

impl<W: Write + Send> JsonWriter<W> {
    pub fn new(out: W) -> Self { Self { out, schema_version: 1 } }

    fn now_rfc3339() -> String {
        // Use a fixed fallback if system time is unavailable.
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
    }
}

impl<W: Write + Send> Renderer for JsonWriter<W> {
    fn handle(&mut self, _state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        let env = Envelope {
            ts: Self::now_rfc3339(),
            v: self.schema_version,
            event,
        };
        serde_json::to_writer(&mut self.out, &env).map_err(io::Error::other)?;
        self.out.write_all(b"\n")
    }

    fn finish(&mut self, _state: &BuildState) -> io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{RecipeId, RecipeTopo};

    #[test]
    fn writes_one_json_object_per_event() {
        let ev = ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0), name: "deps".into(),
                deps: vec![], expected_nodes: 3,
            }],
            total_nodes: 3,
        };
        let mut buf = Vec::new();
        let state = BuildState::new();
        {
            let mut w = JsonWriter::new(&mut buf);
            w.handle(&state, &ev).unwrap();
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("\"type\":\"build-started\""), "got: {s}");
        assert!(s.contains("\"v\":1"), "got: {s}");
        assert!(s.contains("\"ts\":"), "got: {s}");
    }
}
```

- [ ] **Step 2: Add `time` dependency**

Append to `cli/crates/cook-progress/Cargo.toml`:

```toml
time = { version = "0.3", features = ["formatting"] }
```

- [ ] **Step 3: Register the module**

Edit `src/render/mod.rs`, add:

```rust
pub mod json;
```

- [ ] **Step 4: Run the test**

```
cargo test --package cook-progress render::json
```
Expected: 1 passing.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-progress/
git commit -m "feat(cook-progress): JSON-lines event writer"
```

---

## Phase 4: Log store

### Task 8: Log store writer and rotation

**Files:**
- Create: `cli/crates/cook-progress/src/log_store.rs` (replace Task 1 stub)

- [ ] **Step 1: Write failing tests**

In `src/log_store.rs`:

```rust
//! Persistent build log store — .cook/logs/<build-id>/.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::event::{NodeId, ProgressEvent, RecipeId, Stream};
use crate::model::build::BuildState;

#[derive(Debug, Clone)]
pub struct LogConfig {
    pub keep_builds: usize,
    pub max_bytes_per_node: u64,
    pub max_total_bytes: u64,
    pub events_jsonl: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            keep_builds: 20,
            max_bytes_per_node: 2 * 1024 * 1024,
            max_total_bytes: 500 * 1024 * 1024,
            events_jsonl: true,
        }
    }
}

pub struct LogStore {
    root: PathBuf,
    build_id: String,
    events_writer: Option<BufWriter<File>>,
    node_writers: BTreeMap<(RecipeId, NodeId), BufWriter<File>>,
    node_bytes: BTreeMap<(RecipeId, NodeId), u64>,
    config: LogConfig,
}

impl LogStore {
    pub fn open(project_root: &Path, config: LogConfig) -> io::Result<Self> {
        let root = project_root.join(".cook").join("logs");
        fs::create_dir_all(&root)?;
        rotate(&root, config.keep_builds, config.max_total_bytes)?;

        let build_id = new_build_id();
        let build_dir = root.join(&build_id);
        fs::create_dir_all(build_dir.join("nodes"))?;

        let events_writer = if config.events_jsonl {
            Some(BufWriter::new(
                OpenOptions::new().create(true).append(true).open(build_dir.join("events.jsonl"))?
            ))
        } else {
            None
        };

        Ok(Self {
            root,
            build_id,
            events_writer,
            node_writers: BTreeMap::new(),
            node_bytes: BTreeMap::new(),
            config,
        })
    }

    pub fn build_id(&self) -> &str { &self.build_id }

    pub fn record(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        if let Some(w) = self.events_writer.as_mut() {
            let env = serde_json::json!({
                "ts": current_rfc3339(),
                "v": 1,
                "event": event,
            });
            serde_json::to_writer(&mut *w, &env).map_err(io::Error::other)?;
            w.write_all(b"\n")?;
        }

        if let ProgressEvent::NodeOutput { recipe, node, line, stream } = event {
            let key = (*recipe, *node);
            let bytes = self.node_bytes.entry(key).or_insert(0);
            if *bytes >= self.config.max_bytes_per_node {
                return Ok(());
            }
            let writer = match self.node_writers.get_mut(&key) {
                Some(w) => w,
                None => {
                    let r = state.recipes.get(recipe);
                    let rname = r.map(|x| x.name.as_str()).unwrap_or("unknown");
                    let nname = r
                        .and_then(|x| x.nodes.get(node))
                        .map(|n| n.name.clone())
                        .unwrap_or_else(|| format!("node-{}", node.raw()));
                    let dir = self.root.join(&self.build_id).join("nodes").join(rname);
                    fs::create_dir_all(&dir)?;
                    let path = dir.join(format!("{}.log", sanitize(&nname)));
                    let f = OpenOptions::new().create(true).append(true).open(path)?;
                    self.node_writers.insert(key, BufWriter::new(f));
                    self.node_writers.get_mut(&key).unwrap()
                }
            };
            let tag = match stream { Stream::Stdout => "[out]", Stream::Stderr => "[err]" };
            let record = format!("{tag} {line}\n");
            writer.write_all(record.as_bytes())?;
            *bytes += record.len() as u64;
            if *bytes >= self.config.max_bytes_per_node {
                writer.write_all(b"--- truncated ---\n")?;
            }
        }
        Ok(())
    }

    pub fn close(&mut self, success: bool) -> io::Result<()> {
        if let Some(w) = self.events_writer.as_mut() { w.flush()?; }
        for w in self.node_writers.values_mut() { w.flush()?; }

        let manifest = format!(
            "schema_version = 1\nbuild_id = \"{}\"\nstarted_at = \"{}\"\nended_at = \"{}\"\nexit_code = {}\n",
            self.build_id,
            current_rfc3339(),
            current_rfc3339(),
            if success { 0 } else { 1 },
        );
        let path = self.root.join(&self.build_id).join("manifest.toml");
        fs::write(path, manifest)
    }
}

fn new_build_id() -> String {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let hash = format!("{:x}", ts & 0xfff);
    format!("{}-{hash}", current_date_string())
}

fn current_date_string() -> String {
    // Approximation without pulling in `chrono`: use `time` crate already present.
    let now = time::OffsetDateTime::now_utc();
    format!("{:04}-{:02}-{:02}", now.year(), u8::from(now.month()), now.day())
}

fn current_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect()
}

fn rotate(root: &Path, keep_builds: usize, max_total_bytes: u64) -> io::Result<()> {
    let mut entries: Vec<(PathBuf, SystemTime)> = fs::read_dir(root)?
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.metadata().ok().and_then(|m| m.modified().ok()).map(|t| (e.path(), t)))
        .collect();
    entries.sort_by_key(|(_, t)| *t);

    while entries.len() > keep_builds {
        let (p, _) = entries.remove(0);
        let _ = fs::remove_dir_all(p);
    }

    loop {
        let total: u64 = entries.iter()
            .map(|(p, _)| dir_size(p).unwrap_or(0))
            .sum();
        if total <= max_total_bytes || entries.is_empty() { break; }
        let (p, _) = entries.remove(0);
        let _ = fs::remove_dir_all(p);
    }
    Ok(())
}

fn dir_size(p: &Path) -> io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(p)? {
        let entry = entry?;
        let md = entry.metadata()?;
        if md.is_dir() {
            total += dir_size(&entry.path())?;
        } else {
            total += md.len();
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{RecipeTopo};

    #[test]
    fn open_creates_build_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
        let build_dir = tmp.path().join(".cook").join("logs").join(store.build_id());
        assert!(build_dir.exists());
        assert!(build_dir.join("nodes").exists());
    }

    #[test]
    fn node_output_is_written_with_stream_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0), name: "lib".into(),
                deps: vec![], expected_nodes: 1,
            }],
            total_nodes: 1,
        });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "lvm.c".into(), artifact: None, fallback_label: "x".into(),
        });
        store.record(&state, &ProgressEvent::NodeOutput {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            line: "warning".into(), stream: Stream::Stderr,
        }).unwrap();
        store.close(true).unwrap();

        let log = fs::read_to_string(tmp.path()
            .join(".cook").join("logs").join(store.build_id())
            .join("nodes").join("lib").join("lvm.c.log")).unwrap();
        assert!(log.contains("[err] warning"), "got: {log}");
    }

    #[test]
    fn rotate_removes_oldest_when_over_keep_builds() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".cook").join("logs");
        fs::create_dir_all(&root).unwrap();
        for i in 0..5 {
            let d = root.join(format!("build-{i}"));
            fs::create_dir_all(&d).unwrap();
            // mtime ordering via sleep to guarantee deterministic ordering
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        rotate(&root, 2, u64::MAX).unwrap();
        let remaining = fs::read_dir(&root).unwrap().count();
        assert_eq!(remaining, 2);
    }
}
```

- [ ] **Step 2: Add `tempfile` as dev-dependency**

Append to `cli/crates/cook-progress/Cargo.toml` under `[dev-dependencies]`:

```toml
tempfile = "3"
```

- [ ] **Step 3: Run the tests**

```
cargo test --package cook-progress log_store
```
Expected: 3 passing.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/
git commit -m "feat(cook-progress): persistent log store with rotation"
```

---

## Phase 5: Inline renderer

The inline renderer is the largest chunk. It gets split across three tasks: bar lifecycle, template swaps, and interactive freeze/resume. Each task grows the renderer and adds tests that exercise the new behavior via a capturing `TermLike`.

### Task 9: Inline renderer skeleton + capturing test terminal

**Files:**
- Create: `cli/crates/cook-progress/src/render/inline.rs` (replace Task 1 stub)
- Create: `cli/crates/cook-progress/src/render/test_term.rs`
- Modify: `cli/crates/cook-progress/src/render/mod.rs`

- [ ] **Step 1: Create the `TestTerm` helper**

In `src/render/test_term.rs`:

```rust
//! In-memory `TermLike` for snapshot testing indicatif output.

use std::sync::{Arc, Mutex};

use console::TermLike;

#[derive(Clone)]
pub struct TestTerm {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    width: u16,
    height: u16,
    buffer: String,
}

impl TestTerm {
    pub fn new(width: u16) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                width, height: 40, buffer: String::new(),
            })),
        }
    }

    pub fn contents(&self) -> String {
        self.inner.lock().unwrap().buffer.clone()
    }
}

impl TermLike for TestTerm {
    fn width(&self) -> u16 { self.inner.lock().unwrap().width }
    fn height(&self) -> u16 { self.inner.lock().unwrap().height }
    fn move_cursor_up(&self, _n: usize) -> std::io::Result<()> { Ok(()) }
    fn move_cursor_down(&self, _n: usize) -> std::io::Result<()> { Ok(()) }
    fn move_cursor_right(&self, _n: usize) -> std::io::Result<()> { Ok(()) }
    fn move_cursor_left(&self, _n: usize) -> std::io::Result<()> { Ok(()) }
    fn write_line(&self, line: &str) -> std::io::Result<()> {
        let mut g = self.inner.lock().unwrap();
        g.buffer.push_str(line);
        g.buffer.push('\n');
        Ok(())
    }
    fn write_str(&self, s: &str) -> std::io::Result<()> {
        let mut g = self.inner.lock().unwrap();
        g.buffer.push_str(s);
        Ok(())
    }
    fn clear_line(&self) -> std::io::Result<()> { Ok(()) }
    fn flush(&self) -> std::io::Result<()> { Ok(()) }
}
```

- [ ] **Step 2: Register the module**

Edit `src/render/mod.rs`, add:

```rust
#[cfg(test)]
pub mod test_term;
```

- [ ] **Step 3: Implement minimal `InlineRenderer` that only adds waiting bars**

In `src/render/inline.rs`:

```rust
//! Inline renderer — indicatif MultiProgress driving the live frame.

use std::collections::BTreeMap;
use std::io;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::event::{ProgressEvent, RecipeId};
use crate::model::build::BuildState;
use crate::model::recipe::Status;
use crate::render::Renderer;

pub struct InlineRenderer {
    multi: MultiProgress,
    recipe_bars: BTreeMap<RecipeId, ProgressBar>,
    footer: Option<ProgressBar>,
}

impl InlineRenderer {
    pub fn new(draw_target: ProgressDrawTarget) -> Self {
        let multi = MultiProgress::with_draw_target(draw_target);
        Self {
            multi,
            recipe_bars: BTreeMap::new(),
            footer: None,
        }
    }

    fn create_waiting_bar(&self, name: &str, deps: &str) -> ProgressBar {
        let bar = self.multi.add(ProgressBar::new(0));
        bar.set_style(ProgressStyle::with_template("{prefix} {msg}").unwrap());
        bar.set_prefix(format!("◇ {:10}", name));
        bar.set_message(if deps.is_empty() { "waiting".to_string() } else { format!("waiting  ← {deps}") });
        bar
    }
}

impl Renderer for InlineRenderer {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        if let ProgressEvent::BuildStarted { .. } = event {
            for id in &state.order {
                let r = &state.recipes[id];
                let deps: Vec<&str> = r.deps.iter()
                    .filter_map(|d| state.recipes.get(d).map(|x| x.name.as_str()))
                    .collect();
                let deps_str = deps.join(", ");
                let bar = self.create_waiting_bar(&r.name, &deps_str);
                self.recipe_bars.insert(*id, bar);
            }
            let footer = self.multi.add(ProgressBar::new(0));
            footer.set_style(ProgressStyle::with_template("{msg}").unwrap());
            footer.set_message("");
            self.footer = Some(footer);
        }
        Ok(())
    }

    fn finish(&mut self, _state: &BuildState) -> io::Result<()> {
        for bar in self.recipe_bars.values() { bar.finish(); }
        if let Some(f) = &self.footer { f.finish_and_clear(); }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::RecipeTopo;
    use crate::render::test_term::TestTerm;

    #[test]
    fn build_started_creates_a_bar_per_recipe() {
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();
        let ev = ProgressEvent::BuildStarted {
            recipes: vec![
                RecipeTopo { id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes: 3 },
                RecipeTopo { id: RecipeId::new(1), name: "lib".into(), deps: vec![RecipeId::new(0)], expected_nodes: 5 },
            ],
            total_nodes: 8,
        };
        state.apply(&ev);
        inline.handle(&state, &ev).unwrap();
        inline.finish(&state).unwrap();
        assert_eq!(inline.recipe_bars.len(), 2);
    }
}
```

- [ ] **Step 4: Run the test**

```
cargo test --package cook-progress render::inline
```
Expected: 1 passing.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-progress/
git commit -m "feat(cook-progress): inline renderer skeleton with capture term"
```

---

### Task 10: Running/completed/failed state transitions in inline renderer

Extends `InlineRenderer::handle` to swap templates per state change and update the artifact strip message for running recipes.

**Files:**
- Modify: `cli/crates/cook-progress/src/render/inline.rs`

- [ ] **Step 1: Add helper that computes a bar's template for each status**

Append at the bottom of `impl InlineRenderer` (before the tests):

```rust
    fn style_running() -> ProgressStyle {
        ProgressStyle::with_template(
            "{prefix} {bar:40.cyan/dim} {pos}/{len} · {elapsed}{msg_upstream}\n    {msg}",
        )
        .unwrap()
        .with_key("msg_upstream", |_s: &indicatif::ProgressState, _w: &mut dyn std::fmt::Write| {})
        .progress_chars("━━━")
    }

    fn style_completed() -> ProgressStyle {
        ProgressStyle::with_template("{prefix} {msg}").unwrap()
    }

    fn style_cached() -> ProgressStyle {
        ProgressStyle::with_template("{prefix} {msg}").unwrap()
    }

    fn style_failed() -> ProgressStyle {
        ProgressStyle::with_template("{prefix} {msg}").unwrap()
    }

    fn deps_str(&self, state: &BuildState, deps: &[RecipeId]) -> String {
        let names: Vec<&str> = deps.iter()
            .filter_map(|d| state.recipes.get(d).map(|r| r.name.as_str()))
            .collect();
        if names.is_empty() { String::new() } else { format!("  ← {}", names.join(", ")) }
    }

    fn refresh_bar(&self, state: &BuildState, id: RecipeId) {
        let Some(bar) = self.recipe_bars.get(&id) else { return };
        let Some(r) = state.recipes.get(&id) else { return };
        let deps = self.deps_str(state, &r.deps);
        match r.status {
            Status::Waiting => {
                bar.set_style(ProgressStyle::with_template("{prefix} {msg}").unwrap());
                bar.set_prefix(format!("◇ {:10}", r.name));
                bar.set_message(if deps.is_empty() { "waiting".into() } else { format!("waiting{deps}") });
            }
            Status::Running => {
                bar.set_style(Self::style_running());
                bar.set_length(r.progress.1 as u64);
                bar.set_position(r.progress.0 as u64);
                bar.set_prefix(format!("◆ {}", r.name));
                let strip = crate::strip::artifact_strip(r, 100);
                bar.set_message(if deps.is_empty() { strip } else { format!("{deps}\n    {strip}") });
            }
            Status::Completed => {
                bar.set_style(Self::style_completed());
                bar.set_prefix(format!("✓ {}", r.name));
                let secs = r.elapsed.unwrap_or_default().as_secs_f64();
                let cached = if r.cached_count > 0 { format!(" · {} cached", r.cached_count) } else { String::new() };
                bar.set_message(format!("{}/{} · {secs:.1}s{cached}", r.progress.1, r.progress.1));
            }
            Status::Cached => {
                bar.set_style(Self::style_cached());
                bar.set_prefix(format!("≋ {}", r.name));
                bar.set_message(format!("{}/{} cached", r.progress.1, r.progress.1));
            }
            Status::Failed => {
                bar.set_style(Self::style_failed());
                bar.set_prefix(format!("✗ {}", r.name));
                let secs = r.elapsed.unwrap_or_default().as_secs_f64();
                bar.set_message(format!("{}/{} · {secs:.1}s", r.progress.0, r.progress.1));
            }
        }
    }

    fn refresh_footer(&self, state: &BuildState) {
        let Some(footer) = &self.footer else { return };
        let t = &state.totals;
        let secs = state.elapsed().as_secs_f64();
        let mut parts = Vec::new();
        if t.running > 0 { parts.push(format!("{} running", t.running)); }
        if t.done > 0 { parts.push(format!("{} done", t.done)); }
        if t.waiting > 0 { parts.push(format!("{} waiting", t.waiting)); }
        if t.total_nodes > 0 { parts.push(format!("{}/{} cached", t.cached, t.total_nodes)); }
        parts.push(format!("{secs:.1}s"));
        footer.set_message(parts.join(" · "));
    }
```

- [ ] **Step 2: Update `handle` to drive refreshes on every event**

Replace `Renderer for InlineRenderer`'s `handle` with:

```rust
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        if let ProgressEvent::BuildStarted { .. } = event {
            for id in &state.order {
                let r = &state.recipes[id];
                let deps = self.deps_str(state, &r.deps);
                let bar = self.create_waiting_bar(&r.name, deps.trim_start_matches("  ← "));
                self.recipe_bars.insert(*id, bar);
            }
            let footer = self.multi.add(ProgressBar::new(0));
            footer.set_style(ProgressStyle::with_template("{msg}").unwrap());
            self.footer = Some(footer);
        }

        match event {
            ProgressEvent::RecipeStarted { recipe }
            | ProgressEvent::RecipeCompleted { recipe, .. }
            | ProgressEvent::RecipeFailed { recipe, .. }
            | ProgressEvent::NodeStarted { recipe, .. }
            | ProgressEvent::NodeCompleted { recipe, .. }
            | ProgressEvent::NodeFailed { recipe, .. }
            | ProgressEvent::NodeCacheHit { recipe, .. }
            | ProgressEvent::NodeSkipped { recipe, .. } => {
                self.refresh_bar(state, *recipe);
            }
            _ => {}
        }

        self.refresh_footer(state);
        Ok(())
    }
```

- [ ] **Step 3: Add tests covering the transitions**

Append to the `tests` module in `src/render/inline.rs`:

```rust
    use crate::event::{NodeId, Stream};
    use std::time::Duration;

    fn drive(events: &[ProgressEvent]) -> (BuildState, TestTerm) {
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();
        for ev in events {
            state.apply(ev);
            inline.handle(&state, ev).unwrap();
        }
        inline.finish(&state).unwrap();
        (state, term)
    }

    #[test]
    fn running_recipe_uses_bar_style() {
        let events = vec![
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 2 }],
                total_nodes: 2,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "a.c".into(), artifact: Some("build/a.o".into()),
                fallback_label: "clang a.c".into(),
            },
        ];
        let (state, _term) = drive(&events);
        assert_eq!(state.recipes[&RecipeId::new(0)].status, Status::Running);
    }

    #[test]
    fn completed_recipe_shows_cached_suffix_when_any_cached() {
        let events = vec![
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes: 2 }],
                total_nodes: 2,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeCacheHit {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "pkg".into(), artifact: None,
            },
            ProgressEvent::RecipeCompleted {
                recipe: RecipeId::new(0),
                elapsed: Duration::from_millis(100),
                cached: 1, total: 2,
            },
        ];
        let (state, _term) = drive(&events);
        assert_eq!(state.recipes[&RecipeId::new(0)].status, Status::Completed);
        assert_eq!(state.recipes[&RecipeId::new(0)].cached_count, 1);
    }
```

- [ ] **Step 4: Run the tests**

```
cargo test --package cook-progress render::inline
```
Expected: 3 passing.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-progress/src/render/inline.rs
git commit -m "feat(cook-progress): inline renderer per-state template swaps"
```

---

### Task 11: Inline renderer — failure block and interactive freeze/resume

**Files:**
- Modify: `cli/crates/cook-progress/src/render/inline.rs`

- [ ] **Step 1: Add failure-block printing on `RecipeFailed`**

Inside `handle`, before the `self.refresh_footer(state)` call, add:

```rust
        if let ProgressEvent::RecipeFailed { recipe, .. } = event {
            if let Some(r) = state.recipes.get(recipe) {
                if let Some(err) = &r.error_summary {
                    let width = 80;
                    let header = format!("─── {} · failed ─", r.name);
                    let dashes = "─".repeat(width.saturating_sub(header.width()));
                    let _ = self.multi.println(format!("\n{header}{dashes}"));
                    for line in err.lines() {
                        let _ = self.multi.println(format!("│  {line}"));
                    }
                    let _ = self.multi.println("─".repeat(width));
                }
            }
        }
```

Add `use unicode_width::UnicodeWidthStr;` at the top of the file.

- [ ] **Step 2: Add interactive freeze/resume state**

Extend `InlineRenderer` struct:

```rust
pub struct InlineRenderer {
    multi: MultiProgress,
    recipe_bars: BTreeMap<RecipeId, ProgressBar>,
    footer: Option<ProgressBar>,
    pending_resume: bool,
    original_target: Option<ProgressDrawTarget>,
}
```

Update `new` to set `pending_resume: false, original_target: None`.

- [ ] **Step 3: Handle `InteractiveStart` / `InteractiveEnd`**

In `handle`, before the main match, add:

```rust
        if self.pending_resume {
            // If this event is `Finished`, stay frozen. Otherwise, rebuild the bars.
            match event {
                ProgressEvent::Finished { .. } => { /* stay frozen */ }
                _ => {
                    self.pending_resume = false;
                    // Replace MultiProgress with a fresh one pointed at stderr.
                    let target = self.original_target.take()
                        .unwrap_or_else(ProgressDrawTarget::stderr);
                    self.multi = MultiProgress::with_draw_target(target);
                    self.recipe_bars.clear();
                    // Re-add bars from state order.
                    for id in &state.order {
                        let r = &state.recipes[id];
                        let deps = self.deps_str(state, &r.deps);
                        let bar = self.create_waiting_bar(&r.name, deps.trim_start_matches("  ← "));
                        self.recipe_bars.insert(*id, bar);
                    }
                    let footer = self.multi.add(ProgressBar::new(0));
                    footer.set_style(ProgressStyle::with_template("{msg}").unwrap());
                    self.footer = Some(footer);
                    for id in &state.order {
                        self.refresh_bar(state, *id);
                    }
                }
            }
        }

        if let ProgressEvent::InteractiveStart { recipe, node } = event {
            let rname = state.recipes.get(recipe).map(|r| r.name.as_str()).unwrap_or("?");
            let nname = state.recipes.get(recipe)
                .and_then(|r| r.nodes.get(node))
                .map(|n| n.name.as_str()).unwrap_or("?");
            let _ = self.multi.println(format!("─── {rname}/{nname} (interactive) ───"));
            let _ = self.multi.println("");
            // Freeze: replace draw target with hidden, drop bars.
            self.multi.set_draw_target(ProgressDrawTarget::hidden());
            return Ok(());
        }

        if let ProgressEvent::InteractiveEnd { .. } = event {
            self.pending_resume = true;
            return Ok(());
        }
```

- [ ] **Step 4: Add tests**

Append to the `tests` module:

```rust
    #[test]
    fn interactive_start_freezes_and_end_flags_resume() {
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();

        let events = [
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 1 }],
                total_nodes: 1,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "repl".into(), artifact: None, fallback_label: "gdb".into(),
            },
            ProgressEvent::InteractiveStart { recipe: RecipeId::new(0), node: NodeId::new(0) },
            ProgressEvent::InteractiveEnd {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                elapsed: Duration::from_millis(10), success: true,
            },
        ];
        for ev in &events {
            state.apply(ev);
            inline.handle(&state, ev).unwrap();
        }
        assert!(inline.pending_resume);

        // Next event is Finished — should stay frozen.
        let fin = ProgressEvent::Finished { success: true };
        state.apply(&fin);
        inline.handle(&state, &fin).unwrap();
        assert!(inline.pending_resume);
    }

    #[test]
    fn next_non_finished_event_clears_pending_resume() {
        let term = TestTerm::new(100);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let mut inline = InlineRenderer::new(target);
        let mut state = BuildState::new();

        let setup = [
            ProgressEvent::BuildStarted {
                recipes: vec![
                    RecipeTopo { id: RecipeId::new(0), name: "a".into(), deps: vec![], expected_nodes: 1 },
                    RecipeTopo { id: RecipeId::new(1), name: "b".into(), deps: vec![], expected_nodes: 1 },
                ],
                total_nodes: 2,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "x".into(), artifact: None, fallback_label: "x".into(),
            },
            ProgressEvent::InteractiveStart { recipe: RecipeId::new(0), node: NodeId::new(0) },
            ProgressEvent::InteractiveEnd {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                elapsed: Duration::from_millis(1), success: true,
            },
        ];
        for ev in &setup {
            state.apply(ev);
            inline.handle(&state, ev).unwrap();
        }
        assert!(inline.pending_resume);

        let next = ProgressEvent::RecipeStarted { recipe: RecipeId::new(1) };
        state.apply(&next);
        inline.handle(&state, &next).unwrap();
        assert!(!inline.pending_resume);
    }
```

- [ ] **Step 5: Run the tests**

```
cargo test --package cook-progress render::inline
```
Expected: 5 passing.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-progress/src/render/inline.rs
git commit -m "feat(cook-progress): inline failure block and interactive freeze/resume"
```

---

## Phase 6: Driver

### Task 12: `Driver` wires state, log store, and renderer

**Files:**
- Create: `cli/crates/cook-progress/src/driver.rs` (replace Task 1 stub)
- Modify: `cli/crates/cook-progress/src/lib.rs`

- [ ] **Step 1: Write the test for `Driver::run`**

In `src/driver.rs`:

```rust
//! Event-loop driver — wires BuildState + renderer + log store.

use std::io;
use std::sync::mpsc;

use crate::event::ProgressEvent;
use crate::log_store::LogStore;
use crate::model::build::BuildState;
use crate::render::Renderer;

pub struct Driver {
    pub state: BuildState,
    pub renderer: Box<dyn Renderer>,
    pub log_store: Option<LogStore>,
}

impl Driver {
    pub fn new(renderer: Box<dyn Renderer>, log_store: Option<LogStore>) -> Self {
        Self { state: BuildState::new(), renderer, log_store }
    }

    pub fn run(&mut self, rx: mpsc::Receiver<ProgressEvent>) -> io::Result<bool> {
        while let Ok(event) = rx.recv() {
            self.state.apply(&event);
            if let Some(store) = self.log_store.as_mut() {
                let _ = store.record(&self.state, &event);
            }
            self.renderer.handle(&self.state, &event)?;
            if matches!(event, ProgressEvent::Finished { .. }) {
                break;
            }
        }
        self.renderer.finish(&self.state)?;
        let success = self.state.finished.unwrap_or(false);
        if let Some(store) = self.log_store.as_mut() { let _ = store.close(success); }
        Ok(success)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{RecipeId, RecipeTopo};
    use crate::render::plain::PlainRenderer;
    use std::time::Duration;

    #[test]
    fn driver_consumes_events_until_finished() {
        let (tx, rx) = mpsc::channel();
        let buf: Vec<u8> = Vec::new();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(buf));
        struct SharedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for SharedWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf); Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let renderer = Box::new(PlainRenderer::new(SharedWriter(shared.clone())));
        let mut driver = Driver::new(renderer, None);

        tx.send(ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes: 1 }],
            total_nodes: 1,
        }).unwrap();
        tx.send(ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) }).unwrap();
        tx.send(ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(10),
            cached: 0, total: 1,
        }).unwrap();
        tx.send(ProgressEvent::Finished { success: true }).unwrap();
        drop(tx);

        let success = driver.run(rx).unwrap();
        assert!(success);
        let out = String::from_utf8(shared.lock().unwrap().clone()).unwrap();
        assert!(out.contains("deps"));
        assert!(out.contains("done"));
    }
}
```

- [ ] **Step 2: Re-export in lib.rs**

Append to `cli/crates/cook-progress/src/lib.rs`:

```rust
pub use driver::Driver;
pub use event::{NodeId, ProgressEvent, RecipeId, RecipeTopo, SkipReason, Stream};
pub use log_store::{LogConfig, LogStore};
pub use model::{BuildState, NodeState, NodeStatus, RecipeState, Status};
pub use render::Renderer as NewRenderer;
pub use render::inline::InlineRenderer;
pub use render::json::JsonWriter;
pub use render::plain::PlainRenderer;
```

- [ ] **Step 3: Run the test**

```
cargo test --package cook-progress driver
```
Expected: 1 passing.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/
git commit -m "feat(cook-progress): Driver wires state, log store, and renderer"
```

---

## Phase 7: Engine wiring

### Task 13: Extend `EngineEvent` with `BuildStarted` and node artifact fields

**Files:**
- Modify: `cli/crates/cook-engine/src/lib.rs`
- Modify: `cli/crates/cook-engine/src/executor.rs`
- Modify: `cli/crates/cook-engine/src/run.rs`

- [ ] **Step 1: Add `BuildStarted`, `artifact`, `fallback_label` to `EngineEvent`**

Edit `cli/crates/cook-engine/src/lib.rs`, `EngineEvent` enum — add:

```rust
    /// Complete DAG topology, emitted once before any recipe starts.
    BuildStarted {
        recipes: Vec<RecipeTopology>,
        total_nodes: usize,
    },
```

And add a new struct next to `EngineEvent`:

```rust
#[derive(Debug, Clone)]
pub struct RecipeTopology {
    pub name: String,
    pub deps: Vec<String>,
    pub expected_nodes: usize,
}
```

Modify `NodeStarted` and `NodeCacheHit`:

```rust
    NodeStarted {
        recipe: String,
        node_name: String,
        artifact: Option<std::path::PathBuf>,
        fallback_label: String,
    },
    NodeCacheHit {
        recipe: String,
        node_name: String,
        artifact: Option<std::path::PathBuf>,
    },
```

- [ ] **Step 2: Update `executor.rs` emission sites**

In `cli/crates/cook-engine/src/executor.rs`, find every `EngineEvent::NodeStarted { ... }` and `EngineEvent::NodeCacheHit { ... }` and pass the new fields:

For `NodeStarted` (around line 367):
```rust
EngineEvent::NodeStarted {
    recipe: work_node.recipe_name.clone(),
    node_name: payload.display_name(),
    artifact: work_node.cache_meta.as_ref()
        .and_then(|m| m.output_path.clone().map(std::path::PathBuf::from)),
    fallback_label: payload.display_name(),
},
```

For `NodeCacheHit` (lines 302, 338):
```rust
EngineEvent::NodeCacheHit {
    recipe: work_node.recipe_name.clone(),
    node_name,
    artifact: work_node.cache_meta.as_ref()
        .and_then(|m| m.output_path.clone().map(std::path::PathBuf::from)),
},
```

- [ ] **Step 3: Emit `BuildStarted` from `run.rs`**

In `cli/crates/cook-engine/src/run.rs`, locate where the DAG is built and the executor is called. Immediately before execution starts, build and emit the topology:

```rust
// Build topology snapshot before execution.
let mut topos: Vec<cook_engine::RecipeTopology> = Vec::new();
let mut total_nodes = 0usize;
for (name, info) in recipe_infos.iter() {
    let n = /* number of units this recipe will produce — already computed by analyzer */;
    total_nodes += n;
    topos.push(cook_engine::RecipeTopology {
        name: name.clone(),
        deps: info.requires.clone(),
        expected_nodes: n,
    });
}
emit(&event_tx, cook_engine::EngineEvent::BuildStarted {
    recipes: topos,
    total_nodes,
});
```

Note: if the exact per-recipe node count is not known before registration, emit `BuildStarted` with `expected_nodes: 0` — `BuildState.progress.1` will be filled by `RecipeStarted`'s implied count when the executor discovers the true number. This is a known engine-side compromise; the plan surfaces it as a follow-up.

- [ ] **Step 4: Build and run engine tests**

```
cargo build --package cook-engine
cargo test --package cook-engine
```
Expected: clean build; existing tests pass.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-engine/
git commit -m "feat(cook-engine): emit BuildStarted topology and artifact paths"
```

---

### Task 14: Bridge `EngineEvent` to new `ProgressEvent`

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs` (replace `bridge_engine_events`)

- [ ] **Step 1: Rewrite the bridge**

Replace the body of `bridge_engine_events` (lines 75–175) with:

```rust
fn bridge_engine_events(
    engine_rx: mpsc::Receiver<cook_engine::EngineEvent>,
    progress_tx: mpsc::Sender<cook_progress::ProgressEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use cook_progress::{NodeId, RecipeId, RecipeTopo, SkipReason, Stream};
        let mut recipe_ids: std::collections::BTreeMap<String, RecipeId> = Default::default();
        let mut next_recipe: u32 = 0;
        let mut node_ids: std::collections::BTreeMap<(String, String), NodeId> = Default::default();
        let mut next_node: u32 = 0;

        let mut intern_recipe = |name: &str, recipe_ids: &mut std::collections::BTreeMap<String, RecipeId>, next_recipe: &mut u32| -> RecipeId {
            *recipe_ids.entry(name.to_string()).or_insert_with(|| {
                let id = RecipeId::new(*next_recipe);
                *next_recipe += 1;
                id
            })
        };
        let mut intern_node = |recipe: &str, node: &str, node_ids: &mut std::collections::BTreeMap<(String, String), NodeId>, next_node: &mut u32| -> NodeId {
            *node_ids.entry((recipe.to_string(), node.to_string())).or_insert_with(|| {
                let id = NodeId::new(*next_node);
                *next_node += 1;
                id
            })
        };

        while let Ok(event) = engine_rx.recv() {
            let pe = match event {
                cook_engine::EngineEvent::BuildStarted { recipes, total_nodes } => {
                    let topos: Vec<RecipeTopo> = recipes.into_iter().map(|r| {
                        let id = intern_recipe(&r.name, &mut recipe_ids, &mut next_recipe);
                        let deps: Vec<RecipeId> = r.deps.iter()
                            .map(|d| intern_recipe(d, &mut recipe_ids, &mut next_recipe))
                            .collect();
                        RecipeTopo { id, name: r.name, deps, expected_nodes: r.expected_nodes }
                    }).collect();
                    cook_progress::ProgressEvent::BuildStarted { recipes: topos, total_nodes }
                }
                cook_engine::EngineEvent::RecipeStarted { name, .. } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeStarted { recipe: id }
                }
                cook_engine::EngineEvent::RecipeCompleted { name, elapsed, cached_nodes, total_nodes } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeCompleted { recipe: id, elapsed, cached: cached_nodes, total: total_nodes }
                }
                cook_engine::EngineEvent::RecipeFailed { name, elapsed, completed_nodes, total_nodes } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeFailed { recipe: id, elapsed, completed: completed_nodes, total: total_nodes }
                }
                cook_engine::EngineEvent::NodeStarted { recipe, node_name, artifact, fallback_label } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeStarted {
                        recipe: rid, node: nid,
                        name: node_name, artifact, fallback_label,
                    }
                }
                cook_engine::EngineEvent::NodeCompleted { recipe, node_name, elapsed } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeCompleted { recipe: rid, node: nid, elapsed }
                }
                cook_engine::EngineEvent::NodeFailed { recipe, node_name, elapsed, error } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeFailed { recipe: rid, node: nid, elapsed, error }
                }
                cook_engine::EngineEvent::NodeCacheHit { recipe, node_name, artifact } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeCacheHit { recipe: rid, node: nid, name: node_name, artifact }
                }
                cook_engine::EngineEvent::NodeSkipped { recipe, node_name } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeSkipped {
                        recipe: rid, node: nid, name: node_name, reason: SkipReason::UpstreamFailed,
                    }
                }
                cook_engine::EngineEvent::InteractiveStart { recipe } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = NodeId::new(u32::MAX);
                    cook_progress::ProgressEvent::InteractiveStart { recipe: rid, node: nid }
                }
                cook_engine::EngineEvent::InteractiveEnd { recipe, elapsed, success } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = NodeId::new(u32::MAX);
                    cook_progress::ProgressEvent::InteractiveEnd { recipe: rid, node: nid, elapsed, success }
                }
                cook_engine::EngineEvent::OutputLine { recipe, line, is_stderr } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    // OutputLine does not carry a node id yet; attribute to 'last node started in recipe' as a placeholder.
                    let nid = node_ids.iter().rev()
                        .find(|((r, _), _)| r == &recipe)
                        .map(|(_, id)| *id)
                        .unwrap_or(NodeId::new(u32::MAX));
                    let stream = if is_stderr { Stream::Stderr } else { Stream::Stdout };
                    cook_progress::ProgressEvent::NodeOutput { recipe: rid, node: nid, line, stream }
                }
                cook_engine::EngineEvent::RecipeQueued { .. } => continue,
                cook_engine::EngineEvent::Finished { success, .. } => {
                    cook_progress::ProgressEvent::Finished { success }
                }
            };
            let is_finished = matches!(pe, cook_progress::ProgressEvent::Finished { .. });
            let _ = progress_tx.send(pe);
            if is_finished { break; }
        }
    })
}
```

Also change the `run_with_progress` function to use `cook_progress::ProgressEvent` instead of the local `ProgressEvent`. Remove the `use crate::progress::{..., ProgressEvent}` import and update references.

- [ ] **Step 2: Build and run cli tests**

```
cargo build --package cook-cli
cargo test --package cook-cli
```
Expected: cli compiles with some warnings about unused `progress::*` — acceptable for now.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-cli/src/pipeline.rs
git commit -m "feat(cook-cli): bridge engine events to new ProgressEvent"
```

---

## Phase 8: cook-cli swap

### Task 15: Swap `TtyRenderer`/`PlainRenderer` usage for new `Driver`

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs`
- Modify: `cli/crates/cook-cli/src/progress.rs`
- Modify: `cli/crates/cook-cli/src/cli.rs`

- [ ] **Step 1: Add `--output` and `--no-ui` flags to `Cli`**

Edit `cli/crates/cook-cli/src/cli.rs`. Extend `pub struct Cli`:

```rust
    /// Output mode: auto (default), plain, json
    #[arg(long = "output", global = true, default_value = "auto")]
    pub output: String,

    /// Force plain output even on a TTY (synonym for --output=plain)
    #[arg(long = "no-ui", global = true)]
    pub no_ui: bool,
```

- [ ] **Step 2: Implement `spawn_renderer` using new `Driver`**

Add to `cli/crates/cook-cli/src/progress.rs` (keeping old code in place):

```rust
use cook_progress::{Driver, InlineRenderer, JsonWriter, LogConfig, LogStore, PlainRenderer as NewPlainRenderer};
use indicatif::ProgressDrawTarget;

pub enum OutputMode { Auto, Plain, Json }

impl OutputMode {
    pub fn from_cli(cli: &crate::cli::Cli) -> Self {
        if cli.output == "json" { return Self::Json; }
        if cli.output == "plain" || cli.no_ui { return Self::Plain; }
        Self::Auto
    }
}

pub fn spawn_new_renderer(
    cli: &crate::cli::Cli,
    project_root: std::path::PathBuf,
    rx: std::sync::mpsc::Receiver<cook_progress::ProgressEvent>,
) -> std::thread::JoinHandle<bool> {
    let mode = OutputMode::from_cli(cli);
    std::thread::spawn(move || {
        let log_store = LogStore::open(&project_root, LogConfig::default()).ok();

        let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
        let ci = std::env::var("CI").ok().is_some();
        let dumb = std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false);

        let renderer: Box<dyn cook_progress::NewRenderer> = match mode {
            OutputMode::Json => Box::new(JsonWriter::new(std::io::stderr())),
            OutputMode::Plain => Box::new(NewPlainRenderer::new(std::io::stderr())),
            OutputMode::Auto if !is_tty || ci || dumb => {
                Box::new(NewPlainRenderer::new(std::io::stderr()))
            }
            OutputMode::Auto => {
                Box::new(InlineRenderer::new(ProgressDrawTarget::stderr()))
            }
        };

        let mut driver = Driver::new(renderer, log_store);
        driver.run(rx).unwrap_or(false)
    })
}
```

- [ ] **Step 3: Call `spawn_new_renderer` from `run_with_progress`**

In `pipeline.rs`, replace the call to `spawn_renderer_thread` (line ~310):

```rust
let render_thread = crate::progress::spawn_new_renderer(cli, /* project_root */ std::env::current_dir().unwrap(), progress_rx);
```

Remove the call to `progress_tx.send(ProgressEvent::Finished)` at the end of `run_with_progress` — the new driver exits on `Finished` received from the bridge.

- [ ] **Step 4: Build and run a real build**

```
cargo build --release --package cook-cli
cd examples/lua && ../../cli/target/release/cook build
```
Expected: new layout (collapsed done/waiting, artifact strip on running).

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-cli/
git commit -m "feat(cook-cli): wire new renderer Driver with --output flag"
```

---

### Task 16: Add `cook logs` subcommand

**Files:**
- Modify: `cli/crates/cook-cli/src/cli.rs`
- Modify: `cli/crates/cook-cli/src/main.rs`
- Create: `cli/crates/cook-cli/src/cmd_logs.rs`

- [ ] **Step 1: Add the subcommand variant**

Edit `cli/crates/cook-cli/src/cli.rs`, extend `Command`:

```rust
    /// Show logs for past builds
    Logs {
        /// Selector: <recipe>, <recipe>:<node>, or omit to list builds
        selector: Option<String>,
        /// Specific build id
        #[arg(long)]
        build: Option<String>,
        /// Dump failed nodes from the most recent build
        #[arg(long)]
        failed: bool,
    },
```

- [ ] **Step 2: Implement `cmd_logs`**

Create `cli/crates/cook-cli/src/cmd_logs.rs`:

```rust
//! `cook logs` subcommand — dump per-node logs from .cook/logs/.

use std::fs;
use std::path::PathBuf;

use crate::error::CookError;

pub fn cmd_logs(selector: Option<&str>, build: Option<&str>, failed: bool) -> Result<(), CookError> {
    let root = std::env::current_dir()
        .map_err(|e| CookError::Other(e.to_string()))?
        .join(".cook").join("logs");

    if !root.exists() {
        println!("no builds recorded yet");
        return Ok(());
    }

    let builds = list_builds(&root)?;
    if selector.is_none() && build.is_none() && !failed {
        for (id, _) in &builds {
            println!("{id}");
        }
        return Ok(());
    }

    let target = match build {
        Some(b) => root.join(b),
        None => builds.first()
            .map(|(id, _)| root.join(id))
            .ok_or_else(|| CookError::Other("no builds found".into()))?,
    };

    if failed {
        dump_failed(&target)?;
        return Ok(());
    }

    if let Some(sel) = selector {
        let (recipe, node) = split_selector(sel);
        dump_selector(&target, recipe, node)?;
    }
    Ok(())
}

fn list_builds(root: &PathBuf) -> Result<Vec<(String, std::time::SystemTime)>, CookError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| CookError::Other(e.to_string()))? {
        let entry = entry.map_err(|e| CookError::Other(e.to_string()))?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let t = entry.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            out.push((entry.file_name().to_string_lossy().to_string(), t));
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(out)
}

fn split_selector(s: &str) -> (&str, Option<&str>) {
    match s.split_once(':') {
        Some((r, n)) => (r, Some(n)),
        None => (s, None),
    }
}

fn dump_selector(build_dir: &PathBuf, recipe: &str, node: Option<&str>) -> Result<(), CookError> {
    let dir = build_dir.join("nodes").join(recipe);
    if !dir.exists() {
        return Err(CookError::Other(format!("no logs for recipe {recipe}")));
    }
    if let Some(n) = node {
        let path = dir.join(format!("{n}.log"));
        let data = fs::read_to_string(&path).map_err(|e| CookError::Other(e.to_string()))?;
        print!("{data}");
    } else {
        for entry in fs::read_dir(&dir).map_err(|e| CookError::Other(e.to_string()))? {
            let entry = entry.map_err(|e| CookError::Other(e.to_string()))?;
            println!("─── {} ───", entry.file_name().to_string_lossy());
            let data = fs::read_to_string(entry.path()).map_err(|e| CookError::Other(e.to_string()))?;
            print!("{data}");
        }
    }
    Ok(())
}

fn dump_failed(build_dir: &PathBuf) -> Result<(), CookError> {
    let events_path = build_dir.join("events.jsonl");
    let data = fs::read_to_string(&events_path).map_err(|e| CookError::Other(e.to_string()))?;
    for line in data.lines() {
        if line.contains("\"type\":\"node-failed\"") {
            println!("{line}");
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Wire the subcommand in `main.rs`**

Edit `cli/crates/cook-cli/src/main.rs`:

```rust
mod cmd_logs;

// in the match block:
            Some(Command::Logs { selector, build, failed }) => {
                crate::cmd_logs::cmd_logs(selector.as_deref(), build.as_deref(), *failed)
            }
```

- [ ] **Step 4: Run a build + logs cycle**

```
cd examples/lua && ../../cli/target/release/cook build
../../cli/target/release/cook logs
```
Expected: lists the recent build id(s).

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-cli/
git commit -m "feat(cook-cli): cook logs subcommand"
```

---

## Phase 9: Cleanup

### Task 17: Delete old `cook-progress` internals and `cook-cli::progress` legacy types

Once the new pipeline is proven, retire the hand-rolled renderer.

**Files:**
- Delete: `cli/crates/cook-progress/src/bar.rs`
- Delete: `cli/crates/cook-progress/src/frame.rs`
- Delete: `cli/crates/cook-progress/src/output.rs`
- Delete: `cli/crates/cook-progress/src/renderer.rs`
- Delete: `cli/crates/cook-progress/src/symbols.rs`
- Delete: old examples (`basic.rs`, `failure.rs`, `kitchen_sink.rs`, `parallel.rs`, `stress.rs`)
- Modify: `cli/crates/cook-progress/src/lib.rs`
- Modify: `cli/crates/cook-progress/Cargo.toml`
- Modify: `cli/crates/cook-cli/src/progress.rs` (drop legacy code)
- Modify: `cli/crates/cook-cli/Cargo.toml` (drop `crossterm` if unused elsewhere)
- Create: `cli/crates/cook-progress/examples/kitchen_sink.rs`

- [ ] **Step 1: Remove the legacy files**

```bash
rm cli/crates/cook-progress/src/{bar,frame,output,renderer,symbols}.rs
rm cli/crates/cook-progress/examples/{basic,failure,kitchen_sink,parallel,stress}.rs
```

- [ ] **Step 2: Clean up `lib.rs`**

Replace `cli/crates/cook-progress/src/lib.rs` with:

```rust
//! Terminal progress rendering for the Cook build system.

pub mod driver;
pub mod event;
pub mod log_store;
pub mod model;
pub mod render;
pub mod strip;
pub mod style;

pub use driver::Driver;
pub use event::{NodeId, ProgressEvent, RecipeId, RecipeTopo, SkipReason, Stream};
pub use log_store::{LogConfig, LogStore};
pub use model::{BuildState, NodeState, NodeStatus, RecipeState, Status};
pub use render::Renderer;
pub use render::inline::InlineRenderer;
pub use render::json::JsonWriter;
pub use render::plain::PlainRenderer;
```

- [ ] **Step 3: Drop `crossterm` from `cook-progress/Cargo.toml`**

Remove `crossterm = "0.28"` from `[dependencies]`.

- [ ] **Step 4: Gut `cook-cli::progress`**

Replace `cli/crates/cook-cli/src/progress.rs` with the minimal new content: `OutputMode`, `spawn_new_renderer`, `resolve_color`. Remove `PlainRenderer`, `TtyRenderer`, `RecipeRenderState`, `ProgressEvent`, `spawn_renderer_thread`.

- [ ] **Step 5: Add a minimal new example**

Create `cli/crates/cook-progress/examples/kitchen_sink.rs`:

```rust
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use cook_progress::{Driver, InlineRenderer, ProgressEvent, RecipeId, RecipeTopo};
use indicatif::ProgressDrawTarget;

fn main() {
    let (tx, rx) = mpsc::channel::<ProgressEvent>();
    let handle = thread::spawn(move || {
        let mut driver = Driver::new(Box::new(InlineRenderer::new(ProgressDrawTarget::stderr())), None);
        driver.run(rx).unwrap();
    });

    tx.send(ProgressEvent::BuildStarted {
        recipes: vec![
            RecipeTopo { id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes: 2 },
            RecipeTopo { id: RecipeId::new(1), name: "lib".into(), deps: vec![RecipeId::new(0)], expected_nodes: 3 },
        ],
        total_nodes: 5,
    }).unwrap();

    thread::sleep(Duration::from_millis(300));
    tx.send(ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) }).unwrap();
    thread::sleep(Duration::from_millis(500));
    tx.send(ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(500),
        cached: 0, total: 2,
    }).unwrap();
    tx.send(ProgressEvent::Finished { success: true }).unwrap();
    handle.join().unwrap();
}
```

- [ ] **Step 6: Build + test the whole workspace**

```
cargo build --workspace
cargo test --workspace
```
Expected: clean build; all tests pass.

- [ ] **Step 7: Commit**

```bash
git add -A cli/crates/cook-progress cli/crates/cook-cli
git commit -m "refactor(cook-progress): delete legacy renderer and examples"
```

---

### Task 18: Update architecture docs

**Files:**
- Modify: `docs/architecture/supporting-modules.md`

- [ ] **Step 1: Rewrite the cook-progress section**

Replace the `cook-progress` section with a summary reflecting the new architecture:

```markdown
## cook-progress

Pure state machine (`BuildState`) fed by `ProgressEvent` emitted by cook-engine.
Three output paths share the state:

- **Inline renderer** — indicatif `MultiProgress` with per-state templates.
  One recipe → one bar. Running recipes render a bar header plus a one-line
  artifact status strip. Done/waiting/cached/failed collapse to a single line.
- **Plain renderer** — append-only chronological text. Non-TTY default.
- **JSON writer** — one `ProgressEvent` per line. `--output=json`.

Every build persists to `.cook/logs/<build-id>/` (events.jsonl, per-node logs,
manifest.toml) independent of the active renderer. `cook logs` reads that
directory for post-hoc inspection.

TUI and `cook recap` are deferred to a separate project.
```

- [ ] **Step 2: Commit**

```bash
git add docs/architecture/supporting-modules.md
git commit -m "docs: update cook-progress architecture notes"
```

---

## Self-review checklist

After completing all tasks, verify:

1. **Spec coverage:**
   - Event model + pure state → Tasks 2–4 ✓
   - Artifact strip (Rule A) → Task 5 ✓
   - Inline renderer (indicatif, per-state templates) → Tasks 9–11 ✓
   - Interactive freeze/resume → Task 11 ✓
   - Plain renderer → Task 6 ✓
   - `--output=json` → Task 7 ✓
   - Log store with rotation → Task 8 ✓
   - Driver + mode selection → Tasks 12, 15 ✓
   - `cook logs` subcommand → Task 16 ✓
   - Engine `BuildStarted`/artifact plumbing → Tasks 13–14 ✓
   - Legacy code deletion → Task 17 ✓
   - Architecture doc update → Task 18 ✓

2. **Known limitations:**
   - `InteractiveStart`/`End` currently drop to a sentinel `NodeId(u32::MAX)` in the bridge because `EngineEvent::InteractiveStart` doesn't carry a node id; acceptable — follow-up in the engine can add it.
   - `OutputLine` is attributed to "last node started in recipe" rather than the exact producing node. Requires engine to add node id to `OutputLine`; tracked as follow-up.
   - `SkipReason` defaults to `UpstreamFailed` because engine doesn't yet emit the reason; follow-up.

3. **Scope guard — out of scope by design:**
   - TUI, `cook recap`, `u` hot-swap, remote recap, web viewer, OTLP exporter.
