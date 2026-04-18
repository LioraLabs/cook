# Cook Output Experience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite `cook-progress` from scratch with three renderers (inline, TUI, plain) sharing a pure state machine, DAG-aware recipe ordering, width-safe redraws, per-build log persistence at `.cook/logs/<build-id>/`, and a new `cook recap` subcommand for replaying past builds.

**Architecture:** `cook-progress` becomes model-first. A pure `BuildState` state machine consumes `ProgressEvent`s; three renderers (`inline`, `tui`, `plain`) read from a shared `Arc<Mutex<BuildState>>`. Log persistence is an always-on sink that writes `events.jsonl`, per-node `.log` files, and a `manifest.toml` to `.cook/logs/<build-id>/`. `cook recap` replays `events.jsonl` into the same TUI code path as live runs. `cook-engine::EngineEvent` is extended with `BuildStarted { recipes }` and node-scoped `OutputLine`.

**Tech Stack:** Rust 2024 edition, `crossterm 0.28` (existing), `ratatui 0.29` (new TUI dep), `serde_json` (JSON output + events.jsonl), `serde` (events), `unicode-width` (correct truncation), `insta` (snapshot tests), `vt100` (cursor-drift integration tests).

**Sequencing.** Ten phases. Each phase ends in a checkpoint commit where `cargo build && cargo test` passes. Old code stays live until Phase 10; Phases 1–8 coexist as `cook-progress::v2` modules while the legacy `Renderer` keeps running.

- Phase 1 — Scaffolding + dependencies
- Phase 2 — Event API + pure model
- Phase 3 — Inline renderer
- Phase 4 — Plain + JSON renderers
- Phase 5 — Log persistence
- Phase 6 — TUI renderer
- Phase 7 — `cook logs` subcommand
- Phase 8 — `cook recap` subcommand
- Phase 9 — Wire cook-engine + swap over cook-cli
- Phase 10 — Delete old code

**Reference docs:**
- Design spec: `docs/superpowers/specs/2026-04-18-cook-output-experience-design.md`
- Existing legacy code: `cli/crates/cook-progress/src/{bar,frame,output,renderer,symbols}.rs`, `cli/crates/cook-cli/src/progress.rs`
- Engine events: `cli/crates/cook-engine/src/lib.rs:60-133`, emission sites in `cli/crates/cook-engine/src/executor.rs`
- Integration seam: `cli/crates/cook-cli/src/pipeline.rs:75-175` (`bridge_engine_events`, `run_with_progress`)

---

## Phase 1 — Scaffolding + dependencies

Lay the new module directory alongside the legacy crate contents. Nothing is wired yet; legacy `Renderer` keeps working.

### Task 1: Add dependencies to `cook-progress/Cargo.toml`

**Files:**
- Modify: `cli/crates/cook-progress/Cargo.toml`

- [ ] **Step 1: Replace the `[dependencies]` section and add `[dev-dependencies]`**

Open `cli/crates/cook-progress/Cargo.toml`. Replace the file with:

```toml
[package]
name = "cook-progress"
version = "0.1.0"
edition = "2024"
description = "Terminal progress rendering with composable primitives"

[dependencies]
crossterm = "0.28"
ratatui = "0.29"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
unicode-width = "0.2"
time = { version = "0.3", features = ["formatting", "macros", "serde"] }

[dev-dependencies]
insta = { version = "1", features = ["yaml"] }
vt100 = "0.15"
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /home/alex/dev/cook/cli && cargo build -p cook-progress 2>&1 | tail -10`
Expected: compiles cleanly (warnings OK).

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/Cargo.toml cli/Cargo.lock
git commit -m "chore(cook-progress): add ratatui, serde, insta, vt100 deps for rewrite"
```

---

### Task 2: Create `v2` module skeleton (empty files)

**Files:**
- Create: `cli/crates/cook-progress/src/v2/mod.rs`
- Create: `cli/crates/cook-progress/src/v2/event.rs`
- Create: `cli/crates/cook-progress/src/v2/model/mod.rs`
- Create: `cli/crates/cook-progress/src/v2/model/build.rs`
- Create: `cli/crates/cook-progress/src/v2/model/recipe.rs`
- Create: `cli/crates/cook-progress/src/v2/model/node.rs`
- Create: `cli/crates/cook-progress/src/v2/render/mod.rs`
- Create: `cli/crates/cook-progress/src/v2/render/inline.rs`
- Create: `cli/crates/cook-progress/src/v2/render/tui.rs`
- Create: `cli/crates/cook-progress/src/v2/render/plain.rs`
- Create: `cli/crates/cook-progress/src/v2/layout.rs`
- Create: `cli/crates/cook-progress/src/v2/style.rs`
- Create: `cli/crates/cook-progress/src/v2/log_store.rs`
- Create: `cli/crates/cook-progress/src/v2/driver.rs`
- Modify: `cli/crates/cook-progress/src/lib.rs`

The legacy modules (`bar`, `frame`, `output`, `renderer`, `symbols`) stay as-is. Phase 10 will delete them.

- [ ] **Step 1: Create empty placeholder files**

Each of the new `src/v2/**/*.rs` files gets a single line stating its purpose. Create them with exactly this content:

`cli/crates/cook-progress/src/v2/mod.rs`:
```rust
//! cook-progress v2 — model-first rewrite (see docs/superpowers/specs/2026-04-18-cook-output-experience-design.md).

pub mod event;
pub mod layout;
pub mod log_store;
pub mod model;
pub mod render;
pub mod style;

pub use event::{NodeId, ProgressEvent, RecipeId, RecipeTopo, SkipReason, Stream};
pub use model::build::BuildState;
pub use model::recipe::{RecipeState, Status};
pub use model::node::NodeState;
```

`cli/crates/cook-progress/src/v2/event.rs`:
```rust
//! Public event API for cook-progress v2.
```

`cli/crates/cook-progress/src/v2/layout.rs`:
```rust
//! Width-aware layout helpers (hard-truncation, unicode-width).
```

`cli/crates/cook-progress/src/v2/log_store.rs`:
```rust
//! Persistent build logs under .cook/logs/<build-id>/.
```

`cli/crates/cook-progress/src/v2/style.rs`:
```rust
//! Symbol + color configuration.
```

`cli/crates/cook-progress/src/v2/model/mod.rs`:
```rust
//! Pure state machine driven by ProgressEvent. No I/O.

pub mod build;
pub mod node;
pub mod recipe;
```

`cli/crates/cook-progress/src/v2/model/build.rs`:
```rust
//! BuildState — top-level derived state for a single build.
```

`cli/crates/cook-progress/src/v2/model/recipe.rs`:
```rust
//! RecipeState — per-recipe status + output ring.
```

`cli/crates/cook-progress/src/v2/model/node.rs`:
```rust
//! NodeState — per-node status (active work units).
```

`cli/crates/cook-progress/src/v2/render/mod.rs`:
```rust
//! Renderers — consume BuildState, produce bytes. Never mutate state.

pub mod inline;
pub mod plain;
pub mod tui;
```

`cli/crates/cook-progress/src/v2/render/inline.rs`:
```rust
//! Inline scroll-buffer renderer (default).
```

`cli/crates/cook-progress/src/v2/render/tui.rs`:
```rust
//! ratatui-based alt-screen renderer.
```

`cli/crates/cook-progress/src/v2/render/plain.rs`:
```rust
//! Plain text + JSON renderers for non-TTY contexts.
```

`cli/crates/cook-progress/src/v2/driver.rs` — don't create yet (will be added in Phase 6).

- [ ] **Step 2: Wire `v2` into the crate**

Edit `cli/crates/cook-progress/src/lib.rs` — append (keep the legacy re-exports intact):

```rust
pub mod bar;
pub mod frame;
pub mod output;
pub mod renderer;
pub mod symbols;

pub use frame::{ActiveItem, CacheInfo, Footer, Frame, ItemStatus, Section, Status};
pub use renderer::{RenderConfig, Renderer};
pub use symbols::Symbols;

// ── v2 — model-first rewrite (in progress) ────────────────────────────
pub mod v2;
```

- [ ] **Step 3: Verify build**

Run: `cd /home/alex/dev/cook/cli && cargo build -p cook-progress 2>&1 | tail -5`
Expected: compiles cleanly.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/src
git commit -m "feat(cook-progress): scaffold v2 module tree"
```

---

## Phase 2 — Event API + pure model

Implement `ProgressEvent`, typed ids, `OutputRing`, and `BuildState::apply`. Exhaustive `insta` snapshot tests. No renderers yet.

### Task 3: Define typed ids and the `Stream`/`SkipReason` enums

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/event.rs`

- [ ] **Step 1: Write the initial failing test**

Replace `event.rs` with the header plus a failing test:

```rust
//! Public event API for cook-progress v2.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_id_roundtrips_through_json() {
        let id = RecipeId(7);
        let s = serde_json::to_string(&id).unwrap();
        let back: RecipeId = serde_json::from_str(&s).unwrap();
        assert_eq!(id, back);
        assert_eq!(s, "7");
    }

    #[test]
    fn node_id_roundtrips_through_json() {
        let id = NodeId(42);
        let s = serde_json::to_string(&id).unwrap();
        let back: NodeId = serde_json::from_str(&s).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn stream_serializes_as_lowercase() {
        assert_eq!(serde_json::to_string(&Stream::Stdout).unwrap(), "\"stdout\"");
        assert_eq!(serde_json::to_string(&Stream::Stderr).unwrap(), "\"stderr\"");
    }

    #[test]
    fn skip_reason_serializes_as_kebab_case() {
        assert_eq!(
            serde_json::to_string(&SkipReason::UpstreamFailed).unwrap(),
            "\"upstream-failed\""
        );
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::event 2>&1 | tail -10`
Expected: compile errors ("cannot find type `RecipeId`") — that's the expected failing state.

- [ ] **Step 3: Add the types above the `#[cfg(test)]` block**

Append to `event.rs` just above `#[cfg(test)] mod tests`:

```rust
/// Opaque recipe identifier. Issued by the engine at BuildStarted time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecipeId(pub u32);

/// Opaque node identifier. Unique within a single RecipeId.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u32);

/// Which stdio stream an output line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Why a node was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipReason {
    UpstreamFailed,
    ConditionFalse,
    Disabled,
}
```

- [ ] **Step 4: Run tests to confirm they pass**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::event 2>&1 | tail -10`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-progress/src/v2/event.rs
git commit -m "feat(cook-progress): add v2 event ids and enums"
```

---

### Task 4: Define `RecipeTopo` and `ProgressEvent`

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/event.rs`

- [ ] **Step 1: Write the failing tests**

Insert the following tests inside the `#[cfg(test)] mod tests` block in `event.rs` (append after the existing tests):

```rust
    #[test]
    fn build_started_event_serializes_with_tag() {
        let ev = ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId(0),
                name: "deps".into(),
                deps: vec![],
                expected_nodes: 3,
            }],
            total_nodes: 3,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "build-started");
        assert_eq!(json["total_nodes"], 3);
        assert_eq!(json["recipes"][0]["name"], "deps");
    }

    #[test]
    fn node_output_event_includes_stream() {
        let ev = ProgressEvent::NodeOutput {
            recipe: RecipeId(1),
            node: NodeId(2),
            line: "compiling foo.c".into(),
            stream: Stream::Stdout,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "node-output");
        assert_eq!(json["stream"], "stdout");
        assert_eq!(json["line"], "compiling foo.c");
    }

    #[test]
    fn finished_event_includes_success() {
        let ev = ProgressEvent::Finished { success: true };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "finished");
        assert_eq!(json["success"], true);
    }
```

- [ ] **Step 2: Run tests to confirm they fail**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::event 2>&1 | tail -10`
Expected: compile errors on `ProgressEvent` / `RecipeTopo`.

- [ ] **Step 3: Implement `RecipeTopo` and `ProgressEvent`**

Append to `event.rs` just above the `#[cfg(test)]` block:

```rust
use std::time::Duration;

/// Topology entry for one recipe, sent in `BuildStarted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeTopo {
    pub id: RecipeId,
    pub name: String,
    pub deps: Vec<RecipeId>,
    pub expected_nodes: usize,
}

/// Single event emitted during a build. Public contract — versioned via `v=1` on serialized JSON.
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
        #[serde(with = "duration_ms")]
        elapsed: Duration,
        cached: usize,
        total: usize,
    },
    RecipeFailed {
        recipe: RecipeId,
        #[serde(with = "duration_ms")]
        elapsed: Duration,
        completed: usize,
        total: usize,
    },

    NodeStarted {
        recipe: RecipeId,
        node: NodeId,
        label: String,
    },
    NodeCompleted {
        recipe: RecipeId,
        node: NodeId,
        #[serde(with = "duration_ms")]
        elapsed: Duration,
    },
    NodeFailed {
        recipe: RecipeId,
        node: NodeId,
        #[serde(with = "duration_ms")]
        elapsed: Duration,
        error: String,
    },
    NodeCacheHit {
        recipe: RecipeId,
        node: NodeId,
    },
    NodeSkipped {
        recipe: RecipeId,
        node: NodeId,
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
        #[serde(with = "duration_ms")]
        elapsed: Duration,
        success: bool,
    },

    Finished {
        success: bool,
    },
}

/// Serialize a Duration as integer milliseconds. Keeps JSON compact and tooling-friendly.
mod duration_ms {
    use super::Duration;
    use serde::{Deserializer, Serializer};
    use serde::de::Deserialize;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_millis() as u64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let ms = u64::deserialize(d)?;
        Ok(Duration::from_millis(ms))
    }
}
```

- [ ] **Step 4: Run tests to confirm they pass**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::event 2>&1 | tail -10`
Expected: `test result: ok. 7 passed`.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-progress/src/v2/event.rs
git commit -m "feat(cook-progress): add ProgressEvent and RecipeTopo"
```

---

### Task 5: Implement `OutputRing`

**Files:**
- Create: `cli/crates/cook-progress/src/v2/model/output_ring.rs`
- Modify: `cli/crates/cook-progress/src/v2/model/mod.rs`

- [ ] **Step 1: Register the new module**

Edit `cli/crates/cook-progress/src/v2/model/mod.rs`:

```rust
//! Pure state machine driven by ProgressEvent. No I/O.

pub mod build;
pub mod node;
pub mod output_ring;
pub mod recipe;

pub use output_ring::OutputRing;
```

- [ ] **Step 2: Write the failing test**

Create `cli/crates/cook-progress/src/v2/model/output_ring.rs`:

```rust
//! Fixed-capacity ring buffer for the inline renderer's per-recipe output tail.

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct OutputRing {
    capacity: usize,
    lines: VecDeque<String>,
    overflowed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ring_is_empty() {
        let r = OutputRing::new(3);
        assert_eq!(r.capacity(), 3);
        assert!(r.lines().is_empty());
        assert!(!r.overflowed());
    }

    #[test]
    fn push_within_capacity_keeps_everything() {
        let mut r = OutputRing::new(3);
        r.push("a");
        r.push("b");
        assert_eq!(r.lines(), &["a", "b"]);
        assert!(!r.overflowed());
    }

    #[test]
    fn push_past_capacity_drops_oldest_and_flags_overflow() {
        let mut r = OutputRing::new(3);
        for line in ["a", "b", "c", "d", "e"] {
            r.push(line);
        }
        assert_eq!(r.lines(), &["c", "d", "e"]);
        assert!(r.overflowed());
    }

    #[test]
    fn padded_returns_capacity_lines_with_dot_padding() {
        let mut r = OutputRing::new(3);
        r.push("only one");
        let padded = r.padded("·");
        assert_eq!(padded, vec!["·", "·", "only one"]);
    }

    #[test]
    fn padded_when_empty_returns_all_padding() {
        let r = OutputRing::new(3);
        assert_eq!(r.padded("·"), vec!["·", "·", "·"]);
    }

    #[test]
    fn padded_when_full_returns_lines_unchanged() {
        let mut r = OutputRing::new(3);
        for line in ["x", "y", "z"] {
            r.push(line);
        }
        assert_eq!(r.padded("·"), vec!["x", "y", "z"]);
    }
}
```

Run `cargo test -p cook-progress --lib v2::model::output_ring` to confirm it fails to compile (missing methods).

- [ ] **Step 3: Add the impl**

Append to `output_ring.rs` (above the `#[cfg(test)]`):

```rust
impl OutputRing {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            lines: VecDeque::with_capacity(capacity),
            overflowed: false,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn lines(&self) -> Vec<&str> {
        self.lines.iter().map(String::as_str).collect()
    }

    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    pub fn push(&mut self, line: impl Into<String>) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
            self.overflowed = true;
        }
        self.lines.push_back(line.into());
    }

    /// Return exactly `capacity` lines, left-padded with `pad` so older slots
    /// come first. Used by the inline renderer to keep every recipe a fixed height.
    pub fn padded(&self, pad: &str) -> Vec<String> {
        let mut out = Vec::with_capacity(self.capacity);
        let missing = self.capacity.saturating_sub(self.lines.len());
        for _ in 0..missing {
            out.push(pad.to_string());
        }
        for line in &self.lines {
            out.push(line.clone());
        }
        out
    }
}
```

- [ ] **Step 4: Run tests to confirm they pass**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::model::output_ring 2>&1 | tail -10`
Expected: `test result: ok. 6 passed`.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-progress/src/v2/model
git commit -m "feat(cook-progress): add bounded OutputRing with padded() helper"
```

---

### Task 6: Implement `NodeState` and `RecipeState`

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/model/node.rs`
- Modify: `cli/crates/cook-progress/src/v2/model/recipe.rs`

- [ ] **Step 1: Write the failing tests for `NodeState`**

Replace `node.rs` with:

```rust
//! NodeState — per-node status (active work units).

use crate::v2::event::NodeId;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Running,
    Completed,
    Failed,
    Cached,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct NodeState {
    pub id: NodeId,
    pub label: String,
    pub status: NodeStatus,
    pub started_at: Instant,
    pub elapsed: Option<Duration>,
}

impl NodeState {
    pub fn new(id: NodeId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            status: NodeStatus::Running,
            started_at: Instant::now(),
            elapsed: None,
        }
    }

    pub fn complete(&mut self, elapsed: Duration) {
        self.status = NodeStatus::Completed;
        self.elapsed = Some(elapsed);
    }

    pub fn fail(&mut self, elapsed: Duration) {
        self.status = NodeStatus::Failed;
        self.elapsed = Some(elapsed);
    }

    pub fn cache_hit(&mut self) {
        self.status = NodeStatus::Cached;
    }

    pub fn skip(&mut self) {
        self.status = NodeStatus::Skipped;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_node_starts_running() {
        let n = NodeState::new(NodeId(1), "compile foo.c");
        assert_eq!(n.status, NodeStatus::Running);
        assert_eq!(n.label, "compile foo.c");
        assert!(n.elapsed.is_none());
    }

    #[test]
    fn complete_sets_status_and_elapsed() {
        let mut n = NodeState::new(NodeId(1), "x");
        n.complete(Duration::from_millis(42));
        assert_eq!(n.status, NodeStatus::Completed);
        assert_eq!(n.elapsed, Some(Duration::from_millis(42)));
    }

    #[test]
    fn fail_sets_failed_status() {
        let mut n = NodeState::new(NodeId(1), "x");
        n.fail(Duration::from_millis(10));
        assert_eq!(n.status, NodeStatus::Failed);
    }
}
```

Run `cargo test -p cook-progress --lib v2::model::node` — should pass (the impl is already written above the tests).

- [ ] **Step 2: Replace `recipe.rs` with status enum, struct, and test stub**

```rust
//! RecipeState — per-recipe status + output ring.

use crate::v2::event::{NodeId, RecipeId, SkipReason};
use crate::v2::model::node::NodeState;
use crate::v2::model::output_ring::OutputRing;
use std::time::{Duration, Instant};

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
    pub progress: (usize, usize), // (done, total)
    pub started_at: Option<Instant>,
    pub elapsed: Option<Duration>,
    pub active_nodes: Vec<NodeState>,
    pub output_tail: OutputRing,
    pub error_log: Vec<String>,
    pub cache_hits: usize,
    pub skipped: Vec<(NodeId, SkipReason)>,
}

impl RecipeState {
    pub fn new(id: RecipeId, name: impl Into<String>, deps: Vec<RecipeId>, total: usize, tail: usize) -> Self {
        Self {
            id,
            name: name.into(),
            deps,
            status: Status::Waiting,
            progress: (0, total),
            started_at: None,
            elapsed: None,
            active_nodes: Vec::new(),
            output_tail: OutputRing::new(tail),
            error_log: Vec::new(),
            cache_hits: 0,
            skipped: Vec::new(),
        }
    }

    pub fn done(&self) -> usize {
        self.progress.0
    }

    pub fn total(&self) -> usize {
        self.progress.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_recipe_is_waiting_with_zero_progress() {
        let r = RecipeState::new(RecipeId(1), "lib", vec![], 5, 3);
        assert_eq!(r.status, Status::Waiting);
        assert_eq!(r.progress, (0, 5));
        assert_eq!(r.output_tail.capacity(), 3);
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::model 2>&1 | tail -10`
Expected: 4 tests pass (3 node + 1 recipe).

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/src/v2/model/node.rs cli/crates/cook-progress/src/v2/model/recipe.rs
git commit -m "feat(cook-progress): add NodeState and RecipeState types"
```

---

### Task 7: Implement `BuildState` and `apply()`

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/model/build.rs`

- [ ] **Step 1: Write the initial structure with failing apply tests**

Replace `build.rs` with:

```rust
//! BuildState — top-level derived state for a single build.

use crate::v2::event::{NodeId, ProgressEvent, RecipeId, SkipReason};
use crate::v2::model::node::{NodeState, NodeStatus};
use crate::v2::model::recipe::{RecipeState, Status};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct Counters {
    pub total_recipes: usize,
    pub done_recipes: usize,
    pub running_recipes: usize,
    pub waiting_recipes: usize,
    pub failed_recipes: usize,
    pub total_nodes: usize,
    pub done_nodes: usize,
    pub cached_nodes: usize,
}

#[derive(Debug, Clone)]
pub struct BuildState {
    pub order: Vec<RecipeId>,
    pub recipes: BTreeMap<RecipeId, RecipeState>,
    pub started_at: Option<Instant>,
    pub finished: bool,
    pub success: bool,
    pub totals: Counters,
    tail_lines: usize,
}

impl BuildState {
    pub fn new(tail_lines: usize) -> Self {
        Self {
            order: Vec::new(),
            recipes: BTreeMap::new(),
            started_at: None,
            finished: false,
            success: false,
            totals: Counters::default(),
            tail_lines,
        }
    }

    pub fn elapsed(&self) -> Duration {
        match self.started_at {
            Some(t) => t.elapsed(),
            None => Duration::ZERO,
        }
    }

    /// Apply a single event to the state. This is the ONLY mutation path.
    pub fn apply(&mut self, event: ProgressEvent) {
        match event {
            ProgressEvent::BuildStarted { recipes, total_nodes } => {
                self.started_at = Some(Instant::now());
                self.totals.total_recipes = recipes.len();
                self.totals.waiting_recipes = recipes.len();
                self.totals.total_nodes = total_nodes;
                for topo in recipes {
                    self.order.push(topo.id);
                    self.recipes.insert(
                        topo.id,
                        RecipeState::new(topo.id, topo.name, topo.deps, topo.expected_nodes, self.tail_lines),
                    );
                }
            }

            ProgressEvent::RecipeStarted { recipe } => {
                if let Some(r) = self.recipes.get_mut(&recipe) {
                    if r.status == Status::Waiting {
                        self.totals.waiting_recipes = self.totals.waiting_recipes.saturating_sub(1);
                    }
                    r.status = Status::Running;
                    r.started_at = Some(Instant::now());
                    self.totals.running_recipes += 1;
                }
            }

            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total } => {
                if let Some(r) = self.recipes.get_mut(&recipe) {
                    r.status = if cached == total && total > 0 { Status::Cached } else { Status::Completed };
                    r.elapsed = Some(elapsed);
                    r.progress = (total, total);
                    r.cache_hits = cached;
                    r.active_nodes.clear();
                }
                self.totals.running_recipes = self.totals.running_recipes.saturating_sub(1);
                self.totals.done_recipes += 1;
            }

            ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => {
                if let Some(r) = self.recipes.get_mut(&recipe) {
                    r.status = Status::Failed;
                    r.elapsed = Some(elapsed);
                    r.progress = (completed, total);
                    r.active_nodes.clear();
                }
                self.totals.running_recipes = self.totals.running_recipes.saturating_sub(1);
                self.totals.failed_recipes += 1;
            }

            ProgressEvent::NodeStarted { recipe, node, label } => {
                if let Some(r) = self.recipes.get_mut(&recipe) {
                    r.active_nodes.push(NodeState::new(node, label));
                }
            }

            ProgressEvent::NodeCompleted { recipe, node, elapsed } => {
                if let Some(r) = self.recipes.get_mut(&recipe) {
                    remove_active(&mut r.active_nodes, node, |n| n.complete(elapsed));
                    r.progress.0 += 1;
                }
                self.totals.done_nodes += 1;
            }

            ProgressEvent::NodeFailed { recipe, node, elapsed, error } => {
                if let Some(r) = self.recipes.get_mut(&recipe) {
                    remove_active(&mut r.active_nodes, node, |n| n.fail(elapsed));
                    r.progress.0 += 1;
                    for line in error.lines() {
                        r.error_log.push(line.to_string());
                        r.output_tail.push(line);
                    }
                }
                self.totals.done_nodes += 1;
            }

            ProgressEvent::NodeCacheHit { recipe, node } => {
                if let Some(r) = self.recipes.get_mut(&recipe) {
                    remove_active(&mut r.active_nodes, node, |n| n.cache_hit());
                    r.progress.0 += 1;
                    r.cache_hits += 1;
                }
                self.totals.cached_nodes += 1;
                self.totals.done_nodes += 1;
            }

            ProgressEvent::NodeSkipped { recipe, node, reason } => {
                if let Some(r) = self.recipes.get_mut(&recipe) {
                    remove_active(&mut r.active_nodes, node, |n| n.skip());
                    r.progress.0 += 1;
                    r.skipped.push((node, reason));
                }
                self.totals.done_nodes += 1;
            }

            ProgressEvent::NodeOutput { recipe, line, .. } => {
                if let Some(r) = self.recipes.get_mut(&recipe) {
                    r.output_tail.push(line);
                }
            }

            ProgressEvent::InteractiveStart { .. } | ProgressEvent::InteractiveEnd { .. } => {
                // Driver-level concerns; state unchanged.
            }

            ProgressEvent::Finished { success } => {
                self.finished = true;
                self.success = success;
            }
        }
    }
}

fn remove_active(active: &mut Vec<NodeState>, id: NodeId, finalize: impl FnOnce(&mut NodeState)) {
    if let Some(pos) = active.iter().position(|n| n.id == id) {
        let mut node = active.remove(pos);
        finalize(&mut node);
        // Finalized nodes are dropped from `active_nodes` — the persisted log
        // and RecipeState counters retain their contribution.
        let _ = node;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::event::{RecipeTopo, Stream};

    fn seed(tail: usize) -> BuildState {
        let mut s = BuildState::new(tail);
        s.apply(ProgressEvent::BuildStarted {
            recipes: vec![
                RecipeTopo { id: RecipeId(0), name: "deps".into(), deps: vec![], expected_nodes: 1 },
                RecipeTopo { id: RecipeId(1), name: "lib".into(), deps: vec![RecipeId(0)], expected_nodes: 2 },
            ],
            total_nodes: 3,
        });
        s
    }

    #[test]
    fn build_started_populates_order_and_recipes() {
        let s = seed(3);
        assert_eq!(s.order, vec![RecipeId(0), RecipeId(1)]);
        assert_eq!(s.recipes.len(), 2);
        assert_eq!(s.totals.total_recipes, 2);
        assert_eq!(s.totals.waiting_recipes, 2);
        assert_eq!(s.totals.total_nodes, 3);
    }

    #[test]
    fn recipe_started_transitions_to_running() {
        let mut s = seed(3);
        s.apply(ProgressEvent::RecipeStarted { recipe: RecipeId(0) });
        assert_eq!(s.recipes[&RecipeId(0)].status, Status::Running);
        assert_eq!(s.totals.running_recipes, 1);
        assert_eq!(s.totals.waiting_recipes, 1);
    }

    #[test]
    fn node_output_lands_in_output_tail() {
        let mut s = seed(3);
        s.apply(ProgressEvent::RecipeStarted { recipe: RecipeId(1) });
        s.apply(ProgressEvent::NodeOutput {
            recipe: RecipeId(1),
            node: NodeId(100),
            line: "compiling foo.c".into(),
            stream: Stream::Stdout,
        });
        assert_eq!(s.recipes[&RecipeId(1)].output_tail.lines(), vec!["compiling foo.c"]);
    }

    #[test]
    fn recipe_completed_with_all_cached_flips_to_cached_status() {
        let mut s = seed(3);
        s.apply(ProgressEvent::RecipeStarted { recipe: RecipeId(0) });
        s.apply(ProgressEvent::RecipeCompleted {
            recipe: RecipeId(0),
            elapsed: Duration::from_millis(100),
            cached: 1,
            total: 1,
        });
        assert_eq!(s.recipes[&RecipeId(0)].status, Status::Cached);
    }

    #[test]
    fn recipe_completed_with_mixed_flips_to_completed() {
        let mut s = seed(3);
        s.apply(ProgressEvent::RecipeStarted { recipe: RecipeId(1) });
        s.apply(ProgressEvent::RecipeCompleted {
            recipe: RecipeId(1),
            elapsed: Duration::from_millis(100),
            cached: 1,
            total: 2,
        });
        assert_eq!(s.recipes[&RecipeId(1)].status, Status::Completed);
    }

    #[test]
    fn node_failed_records_error_log_and_output_tail() {
        let mut s = seed(3);
        s.apply(ProgressEvent::RecipeStarted { recipe: RecipeId(1) });
        s.apply(ProgressEvent::NodeStarted {
            recipe: RecipeId(1), node: NodeId(1), label: "compile a.c".into(),
        });
        s.apply(ProgressEvent::NodeFailed {
            recipe: RecipeId(1), node: NodeId(1), elapsed: Duration::from_millis(5),
            error: "error: undeclared\nsecond line".into(),
        });
        let r = &s.recipes[&RecipeId(1)];
        assert_eq!(r.error_log, vec!["error: undeclared", "second line"]);
        assert_eq!(r.output_tail.lines(), vec!["error: undeclared", "second line"]);
        assert_eq!(r.progress.0, 1);
    }

    #[test]
    fn node_skipped_tracks_reason() {
        let mut s = seed(3);
        s.apply(ProgressEvent::NodeSkipped {
            recipe: RecipeId(1), node: NodeId(9), reason: SkipReason::UpstreamFailed,
        });
        let r = &s.recipes[&RecipeId(1)];
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].1, SkipReason::UpstreamFailed);
    }

    #[test]
    fn finished_flips_flag() {
        let mut s = seed(3);
        s.apply(ProgressEvent::Finished { success: true });
        assert!(s.finished);
        assert!(s.success);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::model::build 2>&1 | tail -15`
Expected: `test result: ok. 8 passed`.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/src/v2/model/build.rs
git commit -m "feat(cook-progress): add BuildState::apply state machine"
```

---

### Task 8: End-to-end snapshot test of an entire scripted build

**Files:**
- Create: `cli/crates/cook-progress/tests/scripted_build.rs`
- Create: `cli/crates/cook-progress/tests/snapshots/` (directory auto-created by insta)

- [ ] **Step 1: Write the integration test**

Create `cli/crates/cook-progress/tests/scripted_build.rs`:

```rust
//! End-to-end snapshot test: replay a canned event stream through BuildState
//! and snapshot the derived state. Regression guard for the state machine.

use cook_progress::v2::event::{NodeId, ProgressEvent, RecipeId, RecipeTopo, SkipReason, Stream};
use cook_progress::v2::model::build::BuildState;
use std::time::Duration;

fn canned_build() -> Vec<ProgressEvent> {
    vec![
        ProgressEvent::BuildStarted {
            recipes: vec![
                RecipeTopo { id: RecipeId(0), name: "deps".into(), deps: vec![], expected_nodes: 2 },
                RecipeTopo { id: RecipeId(1), name: "lib".into(), deps: vec![RecipeId(0)], expected_nodes: 2 },
                RecipeTopo { id: RecipeId(2), name: "bench".into(), deps: vec![RecipeId(1)], expected_nodes: 1 },
            ],
            total_nodes: 5,
        },
        ProgressEvent::RecipeStarted { recipe: RecipeId(0) },
        ProgressEvent::NodeCacheHit { recipe: RecipeId(0), node: NodeId(0) },
        ProgressEvent::NodeStarted { recipe: RecipeId(0), node: NodeId(1), label: "fetch libcurl".into() },
        ProgressEvent::NodeOutput { recipe: RecipeId(0), node: NodeId(1), line: "resolving libcurl@8.5.0".into(), stream: Stream::Stdout },
        ProgressEvent::NodeCompleted { recipe: RecipeId(0), node: NodeId(1), elapsed: Duration::from_millis(210) },
        ProgressEvent::RecipeCompleted { recipe: RecipeId(0), elapsed: Duration::from_millis(400), cached: 1, total: 2 },
        ProgressEvent::RecipeStarted { recipe: RecipeId(1) },
        ProgressEvent::NodeStarted { recipe: RecipeId(1), node: NodeId(10), label: "compile a.c".into() },
        ProgressEvent::NodeStarted { recipe: RecipeId(1), node: NodeId(11), label: "compile b.c".into() },
        ProgressEvent::NodeFailed {
            recipe: RecipeId(1), node: NodeId(10), elapsed: Duration::from_millis(55),
            error: "a.c:1:1: error: oops".into(),
        },
        ProgressEvent::NodeSkipped { recipe: RecipeId(1), node: NodeId(11), reason: SkipReason::UpstreamFailed },
        ProgressEvent::RecipeFailed { recipe: RecipeId(1), elapsed: Duration::from_millis(80), completed: 2, total: 2 },
        ProgressEvent::NodeSkipped { recipe: RecipeId(2), node: NodeId(20), reason: SkipReason::UpstreamFailed },
        ProgressEvent::Finished { success: false },
    ]
}

#[test]
fn canned_failing_build_matches_snapshot() {
    let mut s = BuildState::new(3);
    for ev in canned_build() {
        s.apply(ev);
    }

    // Normalize non-deterministic fields (timestamps) for snapshot stability.
    let summary = format!(
        "\
success={success}
finished={finished}
order={order:?}
total_recipes={tr} done_recipes={dr} failed={fr}
total_nodes={tn} done_nodes={dn} cached_nodes={cn}
---
{recipes}",
        success = s.success,
        finished = s.finished,
        order = s.order,
        tr = s.totals.total_recipes,
        dr = s.totals.done_recipes,
        fr = s.totals.failed_recipes,
        tn = s.totals.total_nodes,
        dn = s.totals.done_nodes,
        cn = s.totals.cached_nodes,
        recipes = s
            .order
            .iter()
            .map(|id| {
                let r = &s.recipes[id];
                format!(
                    "{name} [{status:?}] {done}/{total} cache_hits={cache} skipped={sk} errs={errs:?} tail={tail:?}",
                    name = r.name, status = r.status,
                    done = r.progress.0, total = r.progress.1,
                    cache = r.cache_hits, sk = r.skipped.len(),
                    errs = r.error_log, tail = r.output_tail.lines(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    );
    insta::assert_snapshot!(summary);
}
```

- [ ] **Step 2: Run the test to create the initial snapshot**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --test scripted_build 2>&1 | tail -15`
Expected: test fails with a "new snapshot" message. Inspect the `.snap.new` file that insta creates under `cli/crates/cook-progress/tests/snapshots/` and confirm the content is correct, then accept it:

```bash
cd /home/alex/dev/cook/cli
INSTA_UPDATE=always cargo test -p cook-progress --test scripted_build 2>&1 | tail -5
```

Re-run without the env var and the test should pass.

- [ ] **Step 3: Verify the snapshot is deterministic (run twice)**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --test scripted_build && cargo test -p cook-progress --test scripted_build 2>&1 | tail -5`
Expected: both runs pass.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/tests/
git commit -m "test(cook-progress): snapshot test for BuildState.apply across a scripted build"
```

---

### Phase 2 checkpoint

Run the full suite to confirm Phase 2 is green:

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-progress 2>&1 | tail -5
```

Expected: all v2 tests pass; legacy tests still pass.

---

## Phase 3 — Inline renderer

Width-aware layout helpers + `render::inline::Renderer`. Fixed 4-line rows per recipe, topo order, hard-truncation at `cols-1`, dep arrows, summary footer. Snapshot tests at widths 40/60/80/120/200 and a `vt100`-based integration test for cursor drift.

### Task 9: Width-aware layout helpers

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/layout.rs`

- [ ] **Step 1: Write failing tests**

Replace `layout.rs` with:

```rust
//! Width-aware layout helpers. All functions measure display columns with
//! `unicode-width`, ignoring ANSI escape bytes. The rest of v2 never calls
//! `str::len()` or `.chars().count()` for width — always goes through here.

use unicode_width::UnicodeWidthStr;

/// Display width of `s` in columns, counting zero for ANSI CSI sequences (ESC `[` ... `m`).
pub fn display_width(s: &str) -> usize {
    let mut out = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // Skip until a byte in 0x40..=0x7E (final byte of CSI)
            i += 2;
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if (0x40..=0x7E).contains(&b) {
                    break;
                }
            }
        } else {
            // Advance one UTF-8 char
            let start = i;
            let ch_len = utf8_char_len(bytes[i]);
            i += ch_len;
            let piece = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
            out += UnicodeWidthStr::width(piece);
        }
    }
    out
}

fn utf8_char_len(b: u8) -> usize {
    if b & 0b1000_0000 == 0 { 1 }
    else if b & 0b1110_0000 == 0b1100_0000 { 2 }
    else if b & 0b1111_0000 == 0b1110_0000 { 3 }
    else { 4 }
}

/// Truncate `s` so its display width is at most `max_cols`. If truncation
/// happens, append `…`. ANSI escapes are always preserved intact; the final
/// string may include a trailing reset.
pub fn truncate_to_width(s: &str, max_cols: usize) -> String {
    if display_width(s) <= max_cols {
        return s.to_string();
    }
    let ellipsis_width = 1;
    let target = max_cols.saturating_sub(ellipsis_width);
    let mut out = String::new();
    let mut width = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let start = i;
            i += 2;
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if (0x40..=0x7E).contains(&b) {
                    break;
                }
            }
            out.push_str(std::str::from_utf8(&bytes[start..i]).unwrap_or(""));
        } else {
            let start = i;
            let ch_len = utf8_char_len(bytes[i]);
            i += ch_len;
            let piece = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
            let w = UnicodeWidthStr::width(piece);
            if width + w > target {
                out.push('…');
                return out;
            }
            out.push_str(piece);
            width += w;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ascii_width() {
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn ansi_csi_is_zero_width() {
        assert_eq!(display_width("\x1b[31mhi\x1b[0m"), 2);
    }

    #[test]
    fn cjk_is_double_width() {
        assert_eq!(display_width("日本語"), 6);
    }

    #[test]
    fn truncate_leaves_short_strings_alone() {
        assert_eq!(truncate_to_width("hi", 10), "hi");
    }

    #[test]
    fn truncate_appends_ellipsis() {
        assert_eq!(truncate_to_width("hello world", 7), "hello …");
    }

    #[test]
    fn truncate_preserves_ansi() {
        let s = "\x1b[31merror: something bad\x1b[0m";
        let t = truncate_to_width(s, 10);
        assert!(t.starts_with("\x1b[31m"));
        assert!(t.ends_with('…'));
        assert!(display_width(&t) <= 10);
    }

    #[test]
    fn truncate_respects_cjk_widths() {
        let t = truncate_to_width("日本語あいう", 5);
        // 日=2, 本=2 → "日本…" = 5 cols
        assert_eq!(t, "日本…");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::layout 2>&1 | tail -10`
Expected: all 7 pass.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/src/v2/layout.rs
git commit -m "feat(cook-progress): width-aware layout helpers (ANSI + unicode-width)"
```

---

### Task 10: Style config (symbols + colors)

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/style.rs`

- [ ] **Step 1: Implement and test**

Replace `style.rs` with:

```rust
//! Symbol + color configuration.

#[derive(Debug, Clone)]
pub struct Style {
    pub colors: bool,
    pub sym_waiting: &'static str,
    pub sym_running: &'static str,
    pub sym_completed: &'static str,
    pub sym_failed: &'static str,
    pub sym_cached: &'static str,
    pub sym_skipped: &'static str,
    pub pad: &'static str,
    pub bar_filled: char,
    pub bar_empty: char,
}

impl Style {
    pub fn unicode(colors: bool) -> Self {
        Self {
            colors,
            sym_waiting: "◇",
            sym_running: "◆",
            sym_completed: "✓",
            sym_failed: "✗",
            sym_cached: "≋",
            sym_skipped: "⊘",
            pad: "·",
            bar_filled: '━',
            bar_empty: '━',
        }
    }

    pub fn ascii(colors: bool) -> Self {
        Self {
            colors,
            sym_waiting: "-",
            sym_running: ">",
            sym_completed: "+",
            sym_failed: "x",
            sym_cached: "=",
            sym_skipped: "/",
            pad: ".",
            bar_filled: '#',
            bar_empty: '-',
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::unicode(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_has_only_ascii_symbols() {
        let s = Style::ascii(false);
        for sym in [s.sym_waiting, s.sym_running, s.sym_completed, s.sym_failed, s.sym_cached, s.sym_skipped, s.pad] {
            assert!(sym.is_ascii(), "non-ASCII symbol: {sym:?}");
        }
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::style 2>&1 | tail -5`
Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/src/v2/style.rs
git commit -m "feat(cook-progress): add Style (unicode + ascii symbol sets)"
```

---

### Task 11: Inline renderer — header rendering

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/render/inline.rs`

- [ ] **Step 1: Initial module + failing tests for `render_header`**

Replace `render/inline.rs` with:

```rust
//! Inline scroll-buffer renderer (default).
//!
//! Layout invariants:
//!   - 4 lines per recipe (header + 3 output tail), always.
//!   - Frame height is deterministic: 4 * recipes + 3 (footer).
//!   - Every rendered line is truncated to width-1 columns before write.

use crate::v2::event::RecipeId;
use crate::v2::layout::{display_width, truncate_to_width};
use crate::v2::model::build::BuildState;
use crate::v2::model::recipe::{RecipeState, Status};
use crate::v2::style::Style;
use std::time::Duration;

pub struct InlineConfig {
    pub width: u16,
    pub tail_lines: usize,
    pub style: Style,
}

impl Default for InlineConfig {
    fn default() -> Self {
        Self { width: 80, tail_lines: 3, style: Style::unicode(false) }
    }
}

pub struct InlineRenderer {
    cfg: InlineConfig,
}

impl InlineRenderer {
    pub fn new(cfg: InlineConfig) -> Self {
        Self { cfg }
    }

    pub fn set_width(&mut self, cols: u16) {
        self.cfg.width = cols;
    }

    /// Content width: width - 1 (guard column to prevent phantom soft-wrap).
    fn cols(&self) -> usize {
        (self.cfg.width as usize).saturating_sub(1).max(10)
    }

    /// Render a recipe's header row, truncated to fit.
    pub fn render_header(&self, r: &RecipeState, dep_names: &[&str]) -> String {
        let s = &self.cfg.style;
        let sym = match r.status {
            Status::Waiting => s.sym_waiting,
            Status::Running => s.sym_running,
            Status::Completed => s.sym_completed,
            Status::Failed => s.sym_failed,
            Status::Cached => s.sym_cached,
        };
        let counter = format!("{}/{}", r.progress.0, r.progress.1);
        let elapsed = r.elapsed.map(format_duration).unwrap_or_default();

        let (done, total) = r.progress;
        let bar = render_bar(self.bar_width(r, &counter, &elapsed, dep_names), done, total, s);

        let mut line = format!("{sym} {name}  {bar} {counter}", name = r.name);
        if !elapsed.is_empty() {
            line.push_str(&format!(" · {elapsed}"));
        }
        if r.status == Status::Waiting {
            line = format!("{sym} {name}  waiting", name = r.name);
        }
        if !dep_names.is_empty() {
            let tail = format_deps(dep_names);
            line.push_str(&format!("  ← {tail}"));
        }
        truncate_to_width(&line, self.cols())
    }

    fn bar_width(&self, r: &RecipeState, counter: &str, elapsed: &str, dep_names: &[&str]) -> usize {
        // header: "{sym} {name}  {bar} {counter} · {elapsed}  ← deps"
        // fixed parts (non-bar):
        let sym_w = 1;
        let spc = 2 + 2; // spaces around name + before counter
        let name_w = display_width(&r.name);
        let counter_w = counter.len();
        let elapsed_w = if elapsed.is_empty() { 0 } else { 3 + elapsed.len() };
        let dep_w = if dep_names.is_empty() { 0 } else { 4 + display_width(&format_deps(dep_names)) };
        let fixed = sym_w + 1 + name_w + 2 + 1 + counter_w + elapsed_w + dep_w + 1;
        self.cols().saturating_sub(fixed).max(4)
    }
}

fn render_bar(width: usize, done: usize, total: usize, s: &Style) -> String {
    if total == 0 {
        return s.bar_empty.to_string().repeat(width.min(10));
    }
    let filled = (width * done) / total;
    let empty = width.saturating_sub(filled);
    let mut out = String::with_capacity(width);
    for _ in 0..filled { out.push(s.bar_filled); }
    for _ in 0..empty { out.push(s.bar_empty); }
    out
}

fn format_deps(names: &[&str]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].to_string(),
        2 => format!("{}, {}", names[0], names[1]),
        n => format!("{}, {}, +{}", names[0], names[1], n - 2),
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}.{}s", secs, d.subsec_millis() / 100)
    } else {
        format!("{}m{}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::event::{ProgressEvent, RecipeId, RecipeTopo};
    use std::time::Duration;

    fn running_recipe() -> RecipeState {
        let mut r = RecipeState::new(RecipeId(1), "lib", vec![RecipeId(0)], 10, 3);
        r.status = Status::Running;
        r.progress = (4, 10);
        r.elapsed = None;
        r
    }

    #[test]
    fn waiting_header_contains_name_and_waiting_word() {
        let r = RecipeState::new(RecipeId(0), "deps", vec![], 0, 3);
        let renderer = InlineRenderer::new(InlineConfig::default());
        let line = renderer.render_header(&r, &[]);
        assert!(line.contains("deps"), "got: {line:?}");
        assert!(line.contains("waiting"), "got: {line:?}");
    }

    #[test]
    fn running_header_has_bar_and_counter() {
        let r = running_recipe();
        let renderer = InlineRenderer::new(InlineConfig::default());
        let line = renderer.render_header(&r, &["deps"]);
        assert!(line.contains("lib"));
        assert!(line.contains("4/10"));
        assert!(line.contains("← deps"));
    }

    #[test]
    fn header_is_truncated_to_width_minus_one() {
        let r = running_recipe();
        let renderer = InlineRenderer::new(InlineConfig { width: 40, ..Default::default() });
        let line = renderer.render_header(&r, &["deps", "other", "third"]);
        assert!(display_width(&line) <= 39, "width={} line={:?}", display_width(&line), line);
    }

    #[test]
    fn deps_of_3_or_more_shows_plus_n() {
        let deps = format_deps(&["a", "b", "c", "d"]);
        assert_eq!(deps, "a, b, +2");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::render::inline 2>&1 | tail -10`
Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/src/v2/render/inline.rs
git commit -m "feat(cook-progress): inline renderer header with width-safe truncation"
```

---

### Task 12: Inline renderer — full frame rendering

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/render/inline.rs`

- [ ] **Step 1: Add frame rendering and footer**

Append to `render/inline.rs`, above the `#[cfg(test)]` block:

```rust
impl InlineRenderer {
    /// Render the complete frame for a given BuildState. Returns a `Vec<String>`
    /// where each string is one rendered (and truncated) line — no '\n' inside.
    /// The caller is responsible for emitting `\n` after each line. Total frame
    /// height == returned Vec len == 4 * recipes + 3 (footer separator, text, hint).
    pub fn render_frame(&self, state: &BuildState) -> Vec<String> {
        let mut out = Vec::with_capacity(4 * state.order.len() + 3);
        for id in &state.order {
            let Some(r) = state.recipes.get(id) else { continue; };
            let dep_names: Vec<&str> = r
                .deps
                .iter()
                .filter_map(|d| state.recipes.get(d).map(|x| x.name.as_str()))
                .collect();
            out.push(self.render_header(r, &dep_names));
            let tail = r.output_tail.padded(self.cfg.style.pad);
            for line in tail {
                let indent = format!("    {line}");
                out.push(truncate_to_width(&indent, self.cols()));
            }
        }
        // Footer: separator, status line, hint.
        let sep = "─".repeat(self.cols().min(60));
        out.push(sep);
        out.push(truncate_to_width(&self.render_footer_status(state), self.cols()));
        out.push(truncate_to_width(&self.render_footer_hint(state), self.cols()));
        out
    }

    fn render_footer_status(&self, s: &BuildState) -> String {
        let t = &s.totals;
        let mut parts = Vec::new();
        if t.running_recipes > 0 { parts.push(format!("{} running", t.running_recipes)); }
        if t.done_recipes > 0 { parts.push(format!("{} done", t.done_recipes)); }
        if t.failed_recipes > 0 { parts.push(format!("{} failed", t.failed_recipes)); }
        if t.waiting_recipes > 0 { parts.push(format!("{} waiting", t.waiting_recipes)); }
        if t.cached_nodes > 0 { parts.push(format!("{}/{} cached", t.cached_nodes, t.total_nodes)); }
        parts.push(format_duration(s.elapsed()));
        parts.join(" · ")
    }

    fn render_footer_hint(&self, s: &BuildState) -> String {
        if s.finished {
            if s.success {
                format!("{} build succeeded", self.cfg.style.sym_completed)
            } else {
                format!("{} build failed", self.cfg.style.sym_failed)
            }
        } else {
            "press 'u' for live UI · ctrl-c to cancel".to_string()
        }
    }
}
```

- [ ] **Step 2: Add a test that asserts frame height**

Append inside the `#[cfg(test)]` module in `render/inline.rs`:

```rust
    #[test]
    fn frame_height_is_exactly_four_per_recipe_plus_three_footer_lines() {
        let mut s = BuildState::new(3);
        s.apply(ProgressEvent::BuildStarted {
            recipes: vec![
                RecipeTopo { id: RecipeId(0), name: "deps".into(), deps: vec![], expected_nodes: 2 },
                RecipeTopo { id: RecipeId(1), name: "lib".into(), deps: vec![RecipeId(0)], expected_nodes: 3 },
            ],
            total_nodes: 5,
        });
        let renderer = InlineRenderer::new(InlineConfig::default());
        let lines = renderer.render_frame(&s);
        assert_eq!(lines.len(), 4 * 2 + 3, "got: {}", lines.len());
    }

    #[test]
    fn every_frame_line_fits_in_cols_minus_one() {
        let mut s = BuildState::new(3);
        s.apply(ProgressEvent::BuildStarted {
            recipes: vec![
                RecipeTopo { id: RecipeId(0), name: "this-is-a-super-long-recipe-name-that-will-not-fit".into(), deps: vec![], expected_nodes: 2 },
            ],
            total_nodes: 2,
        });
        let renderer = InlineRenderer::new(InlineConfig { width: 40, ..Default::default() });
        for line in renderer.render_frame(&s) {
            assert!(display_width(&line) <= 39, "width {}: {line:?}", display_width(&line));
        }
    }
```

- [ ] **Step 3: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::render::inline 2>&1 | tail -10`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/src/v2/render/inline.rs
git commit -m "feat(cook-progress): inline renderer full frame + footer"
```

---

### Task 13: Inline renderer — clear/draw to a writer

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/render/inline.rs`

- [ ] **Step 1: Add the write-to-sink implementation**

Append to `render/inline.rs`, still above `#[cfg(test)]`:

```rust
use std::io::{self, Write};

impl InlineRenderer {
    /// Write the frame to `out` and return the number of logical lines emitted.
    /// Each line is followed by '\n'. ANSI cursor-up + erase-line is used only
    /// by `clear_last_frame`, not by this method.
    pub fn write_frame<W: Write>(&self, state: &BuildState, out: &mut W) -> io::Result<usize> {
        let frame = self.render_frame(state);
        for line in &frame {
            writeln!(out, "{line}")?;
        }
        Ok(frame.len())
    }

    /// Emit `lines` copies of ESC[1A ESC[2K to erase the previously rendered frame.
    /// Caller must pass the value returned by the last `write_frame` call.
    pub fn clear_last_frame<W: Write>(&self, lines: usize, out: &mut W) -> io::Result<()> {
        for _ in 0..lines {
            write!(out, "\x1b[1A\x1b[2K")?;
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Add a test that drives write+clear through a Vec**

Append inside the `#[cfg(test)]` module:

```rust
    #[test]
    fn write_frame_then_clear_emits_expected_bytes() {
        let mut s = BuildState::new(3);
        s.apply(ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo { id: RecipeId(0), name: "a".into(), deps: vec![], expected_nodes: 1 }],
            total_nodes: 1,
        });
        let renderer = InlineRenderer::new(InlineConfig::default());
        let mut buf = Vec::new();
        let n = renderer.write_frame(&s, &mut buf).unwrap();
        assert_eq!(n, 4 * 1 + 3);
        let frame_bytes = buf.len();
        renderer.clear_last_frame(n, &mut buf).unwrap();
        // Each clear is 8 bytes: ESC[1A ESC[2K
        assert_eq!(buf.len() - frame_bytes, n * "\x1b[1A\x1b[2K".len());
    }
```

- [ ] **Step 3: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::render::inline 2>&1 | tail -10`
Expected: 7 tests pass.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/src/v2/render/inline.rs
git commit -m "feat(cook-progress): inline write_frame + clear_last_frame"
```

---

### Task 14: Snapshot tests at 5 widths

**Files:**
- Create: `cli/crates/cook-progress/tests/inline_snapshots.rs`

- [ ] **Step 1: Add the multi-width snapshot test**

Create `cli/crates/cook-progress/tests/inline_snapshots.rs`:

```rust
//! Snapshot tests for the inline renderer at representative widths.

use cook_progress::v2::event::{NodeId, ProgressEvent, RecipeId, RecipeTopo, Stream};
use cook_progress::v2::model::build::BuildState;
use cook_progress::v2::render::inline::{InlineConfig, InlineRenderer};
use cook_progress::v2::style::Style;

fn canned_state() -> BuildState {
    let mut s = BuildState::new(3);
    s.apply(ProgressEvent::BuildStarted {
        recipes: vec![
            RecipeTopo { id: RecipeId(0), name: "deps".into(), deps: vec![], expected_nodes: 3 },
            RecipeTopo { id: RecipeId(1), name: "lib".into(), deps: vec![RecipeId(0)], expected_nodes: 5 },
            RecipeTopo { id: RecipeId(2), name: "test".into(), deps: vec![RecipeId(1)], expected_nodes: 4 },
        ],
        total_nodes: 12,
    });
    s.apply(ProgressEvent::RecipeStarted { recipe: RecipeId(0) });
    s.apply(ProgressEvent::NodeCacheHit { recipe: RecipeId(0), node: NodeId(1) });
    s.apply(ProgressEvent::NodeCacheHit { recipe: RecipeId(0), node: NodeId(2) });
    s.apply(ProgressEvent::NodeCompleted {
        recipe: RecipeId(0), node: NodeId(3), elapsed: std::time::Duration::from_millis(40),
    });
    s.apply(ProgressEvent::RecipeCompleted {
        recipe: RecipeId(0), elapsed: std::time::Duration::from_millis(400),
        cached: 2, total: 3,
    });
    s.apply(ProgressEvent::RecipeStarted { recipe: RecipeId(1) });
    s.apply(ProgressEvent::NodeStarted {
        recipe: RecipeId(1), node: NodeId(10), label: "compile renderer.cpp".into(),
    });
    s.apply(ProgressEvent::NodeOutput {
        recipe: RecipeId(1), node: NodeId(10),
        line: "renderer.cpp:42:9: warning: unused variable 'foo'".into(),
        stream: Stream::Stderr,
    });
    s.apply(ProgressEvent::NodeOutput {
        recipe: RecipeId(1), node: NodeId(10),
        line: "[g++] -O2 -c renderer.cpp -o build/renderer.o".into(),
        stream: Stream::Stdout,
    });
    s
}

fn render_at(width: u16) -> String {
    let renderer = InlineRenderer::new(InlineConfig {
        width,
        tail_lines: 3,
        style: Style::unicode(false),
    });
    renderer.render_frame(&canned_state()).join("\n")
}

#[test] fn snap_width_40()  { insta::assert_snapshot!("width_40",  render_at(40));  }
#[test] fn snap_width_60()  { insta::assert_snapshot!("width_60",  render_at(60));  }
#[test] fn snap_width_80()  { insta::assert_snapshot!("width_80",  render_at(80));  }
#[test] fn snap_width_120() { insta::assert_snapshot!("width_120", render_at(120)); }
#[test] fn snap_width_200() { insta::assert_snapshot!("width_200", render_at(200)); }
```

- [ ] **Step 2: Generate initial snapshots**

Run: `cd /home/alex/dev/cook/cli && INSTA_UPDATE=always cargo test -p cook-progress --test inline_snapshots 2>&1 | tail -10`

Expected: creates 5 snapshot files under `tests/snapshots/inline_snapshots__*.snap`.

- [ ] **Step 3: Manually inspect each snapshot**

Inspect the 5 new `.snap` files. For each, confirm:
- No line exceeds `width-1` columns
- Headers show symbol + name + bar + counter
- Output tail lines are indented 4 spaces
- Deps show `← deps` where applicable

- [ ] **Step 4: Re-run tests to confirm they pass deterministically**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --test inline_snapshots 2>&1 | tail -5`
Expected: `test result: ok. 5 passed`.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-progress/tests/
git commit -m "test(cook-progress): inline renderer width snapshots 40/60/80/120/200"
```

---

### Task 15: `vt100` integration test for narrow-terminal cursor drift

**Files:**
- Create: `cli/crates/cook-progress/tests/vt100_cursor_drift.rs`

- [ ] **Step 1: Write the regression test**

Create `cli/crates/cook-progress/tests/vt100_cursor_drift.rs`:

```rust
//! Regression guard for the narrow-terminal cursor-drift bug.
//!
//! Drives the inline renderer through many frames at a narrow width, feeds the
//! produced bytes (frame + clear cycle) into a vt100 parser, and asserts that
//! at the end of each cycle the terminal contains ONLY the latest frame.

use cook_progress::v2::event::{NodeId, ProgressEvent, RecipeId, RecipeTopo, Stream};
use cook_progress::v2::layout::display_width;
use cook_progress::v2::model::build::BuildState;
use cook_progress::v2::render::inline::{InlineConfig, InlineRenderer};
use cook_progress::v2::style::Style;
use std::time::Duration;

#[test]
fn redraw_at_narrow_width_never_leaves_stale_lines() {
    // 80-row parser, 40-col terminal (intentionally narrow).
    let mut parser = vt100::Parser::new(80, 40, 0);
    let renderer = InlineRenderer::new(InlineConfig {
        width: 40,
        tail_lines: 3,
        style: Style::unicode(false),
    });

    let mut state = BuildState::new(3);
    state.apply(ProgressEvent::BuildStarted {
        recipes: vec![
            RecipeTopo { id: RecipeId(0), name: "deps".into(), deps: vec![], expected_nodes: 1 },
            RecipeTopo { id: RecipeId(1), name: "lib".into(), deps: vec![RecipeId(0)], expected_nodes: 3 },
        ],
        total_nodes: 4,
    });

    let mut last_height: usize = 0;
    let output_lines = [
        "compiling this-is-an-artificially-long-line-that-would-soft-wrap-on-40-cols",
        "linking",
        "error: undeclared identifier in very long path /a/b/c/d/e/f/g/h",
    ];

    // 20 ticks — lots of redraw cycles.
    for (i, line) in output_lines.iter().cycle().take(20).enumerate() {
        // Tick event: emit an output line.
        state.apply(ProgressEvent::NodeOutput {
            recipe: RecipeId(1), node: NodeId(i as u32),
            line: (*line).to_string(), stream: Stream::Stdout,
        });

        let mut buf = Vec::new();
        renderer.clear_last_frame(last_height, &mut buf).unwrap();
        last_height = renderer.write_frame(&state, &mut buf).unwrap();
        parser.process(&buf);

        // Assert: no visible line on-screen exceeds 39 columns of non-whitespace content.
        let screen = parser.screen();
        for row in 0..parser.screen().size().0 {
            let contents = screen.row_wrapped(row, 0);
            assert!(
                display_width(&contents) <= 40,
                "row {row} at tick {i} exceeds 40 cols: {contents:?}"
            );
        }
    }

    // Final frame: compute what we expect on screen vs what vt100 shows.
    let expected = renderer.render_frame(&state);
    let screen = parser.screen();
    for (idx, expected_line) in expected.iter().enumerate() {
        let row = (screen.size().0 as usize).saturating_sub(expected.len()) + idx;
        let got = screen.row_wrapped(row as u16, 0);
        let got_trimmed = got.trim_end();
        assert_eq!(
            got_trimmed, expected_line.trim_end(),
            "row {row}: expected {expected_line:?}, got {got_trimmed:?}"
        );
    }
    let _ = Duration::ZERO; // silence unused import warning if compiler nags
}
```

Note: `vt100::Screen::row_wrapped` may not exist under that exact name. If the test fails to compile, use `screen.contents()` (split by '\n') or `screen.cell(row, col)` to assemble row content. Adjust the helper inline; the semantic assertion (each visible row is ≤ 40 cols of drawn content) is what matters.

- [ ] **Step 2: Run the test**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --test vt100_cursor_drift 2>&1 | tail -10`
Expected: passes. If `row_wrapped` doesn't compile, consult `cargo doc -p vt100 --open` and replace with whichever method iterates row contents.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/tests/vt100_cursor_drift.rs
git commit -m "test(cook-progress): vt100 regression guard for narrow-terminal cursor drift"
```

---

### Phase 3 checkpoint

Run the full suite:

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-progress 2>&1 | tail -5
```

Expected: all v2 tests pass.

---

## Phase 4 — Plain + JSON renderers

Chronological text format for pipes / CI + JSON-lines emitter. Both are pure writers driven by the event stream (not by `BuildState`).

### Task 16: Plain text renderer

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/render/plain.rs`

- [ ] **Step 1: Write the renderer**

Replace `render/plain.rs` with:

```rust
//! Plain text + JSON renderers for non-TTY contexts.

use crate::v2::event::{NodeId, ProgressEvent, RecipeId, Stream};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::Duration;

pub struct PlainRenderer {
    names: BTreeMap<RecipeId, String>,
    node_starts: BTreeMap<(RecipeId, NodeId), std::time::Instant>,
}

impl PlainRenderer {
    pub fn new() -> Self {
        Self { names: BTreeMap::new(), node_starts: BTreeMap::new() }
    }

    fn name(&self, id: &RecipeId) -> &str {
        self.names.get(id).map(String::as_str).unwrap_or("?")
    }

    pub fn handle<W: Write>(&mut self, event: &ProgressEvent, out: &mut W) -> io::Result<()> {
        match event {
            ProgressEvent::BuildStarted { recipes, .. } => {
                for topo in recipes {
                    self.names.insert(topo.id, topo.name.clone());
                }
            }
            ProgressEvent::RecipeStarted { .. } => {}
            ProgressEvent::NodeStarted { recipe, node, .. } => {
                self.node_starts.insert((*recipe, *node), std::time::Instant::now());
            }
            ProgressEvent::NodeOutput { recipe, node, line, stream } => {
                let prefix = match stream {
                    Stream::Stdout => "",
                    Stream::Stderr => "err! ",
                };
                writeln!(out, "  [{}/{}] {}{}", self.name(recipe), node.0, prefix, line)?;
            }
            ProgressEvent::NodeCompleted { recipe, node, elapsed } => {
                writeln!(out, "  {}/{}  {}  done", self.name(recipe), node.0, fmt_secs(*elapsed))?;
            }
            ProgressEvent::NodeCacheHit { recipe, node } => {
                writeln!(out, "  {}/{}  cached", self.name(recipe), node.0)?;
            }
            ProgressEvent::NodeSkipped { recipe, node, reason } => {
                writeln!(out, "  {}/{}  skipped ({:?})", self.name(recipe), node.0, reason)?;
            }
            ProgressEvent::NodeFailed { recipe, node, elapsed, error } => {
                writeln!(out, "  {}/{}  {}  FAILED", self.name(recipe), node.0, fmt_secs(*elapsed))?;
                for line in error.lines() {
                    writeln!(out, "  [{}/{}] {}", self.name(recipe), node.0, line)?;
                }
            }
            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total } => {
                let cache_part = if *cached == *total && *total > 0 {
                    ", cached".to_string()
                } else if *cached > 0 {
                    format!(", {cached}/{total} cached")
                } else {
                    String::new()
                };
                writeln!(out, "{}  done  ({}/{}{})  {}",
                    self.name(recipe), total, total, cache_part, fmt_secs(*elapsed))?;
            }
            ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => {
                writeln!(out, "{}  FAILED  ({}/{} nodes)  {}",
                    self.name(recipe), completed, total, fmt_secs(*elapsed))?;
            }
            ProgressEvent::InteractiveStart { recipe, node } => {
                writeln!(out, "── interactive: {}/{} ──", self.name(recipe), node.0)?;
            }
            ProgressEvent::InteractiveEnd { .. } => {
                writeln!(out, "── back to build ──")?;
            }
            ProgressEvent::Finished { success } => {
                writeln!(out, "build {}", if *success { "succeeded" } else { "failed" })?;
            }
        }
        Ok(())
    }
}

fn fmt_secs(d: Duration) -> String {
    format!("{:.2}s", d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::event::{RecipeTopo, Stream};
    use std::time::Duration;

    fn lines(events: &[ProgressEvent]) -> String {
        let mut buf = Vec::new();
        let mut r = PlainRenderer::new();
        for ev in events {
            r.handle(ev, &mut buf).unwrap();
        }
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn plain_emits_node_output_with_recipe_prefix() {
        let out = lines(&[
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId(0), name: "lib".into(), deps: vec![], expected_nodes: 1 }],
                total_nodes: 1,
            },
            ProgressEvent::NodeOutput {
                recipe: RecipeId(0), node: NodeId(1),
                line: "gcc -c foo.c".into(), stream: Stream::Stdout,
            },
        ]);
        assert!(out.contains("[lib/1] gcc -c foo.c"));
    }

    #[test]
    fn plain_emits_recipe_done_with_cache() {
        let out = lines(&[
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId(0), name: "deps".into(), deps: vec![], expected_nodes: 3 }],
                total_nodes: 3,
            },
            ProgressEvent::RecipeCompleted {
                recipe: RecipeId(0), elapsed: Duration::from_millis(400), cached: 2, total: 3,
            },
        ]);
        assert!(out.contains("deps  done"));
        assert!(out.contains("2/3 cached"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::render::plain 2>&1 | tail -10`
Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/src/v2/render/plain.rs
git commit -m "feat(cook-progress): chronological plain-text renderer"
```

---

### Task 17: JSON-lines renderer

**Files:**
- Create: `cli/crates/cook-progress/src/v2/render/json.rs`
- Modify: `cli/crates/cook-progress/src/v2/render/mod.rs`

- [ ] **Step 1: Register module**

Edit `cli/crates/cook-progress/src/v2/render/mod.rs`:

```rust
//! Renderers — consume BuildState, produce bytes. Never mutate state.

pub mod inline;
pub mod json;
pub mod plain;
pub mod tui;
```

- [ ] **Step 2: Create the JSON writer**

Create `cli/crates/cook-progress/src/v2/render/json.rs`:

```rust
//! JSON-lines renderer. One object per event. Schema version = 1.

use crate::v2::event::ProgressEvent;
use serde::Serialize;
use std::io::{self, Write};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Serialize)]
struct Envelope<'a> {
    ts: String,
    v: u32,
    #[serde(flatten)]
    event: &'a ProgressEvent,
}

pub struct JsonRenderer;

impl JsonRenderer {
    pub fn new() -> Self { Self }

    pub fn handle<W: Write>(&self, event: &ProgressEvent, out: &mut W) -> io::Result<()> {
        let env = Envelope {
            ts: OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default(),
            v: 1,
            event,
        };
        serde_json::to_writer(&mut *out, &env)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        out.write_all(b"\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::event::{RecipeId, RecipeTopo};

    #[test]
    fn json_renderer_emits_envelope_with_type_and_version() {
        let ev = ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo { id: RecipeId(0), name: "deps".into(), deps: vec![], expected_nodes: 1 }],
            total_nodes: 1,
        };
        let mut buf = Vec::new();
        JsonRenderer::new().handle(&ev, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["type"], "build-started");
        assert!(v["ts"].as_str().unwrap().contains('T'));
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::render::json 2>&1 | tail -10`
Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/src/v2/render/
git commit -m "feat(cook-progress): JSON-lines renderer with RFC3339 timestamps + v=1"
```

---

### Phase 4 checkpoint

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-progress 2>&1 | tail -5
```

Expected: all v2 tests pass.

---

## Phase 5 — Log persistence

Always-on writer to `.cook/logs/<build-id>/`: `events.jsonl`, per-node logs, manifest. Rotation bounded by both per-node size and total-dir size.

### Task 18: `log_store` — build id + directory layout

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/log_store.rs`

- [ ] **Step 1: Implement build-id generator + paths**

Replace `log_store.rs` with:

```rust
//! Persistent build logs under .cook/logs/<build-id>/.

use crate::v2::event::{NodeId, ProgressEvent, RecipeId};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Computed paths for a single build-id within a project's `.cook/logs/`.
pub struct BuildPaths {
    pub root: PathBuf,       // .cook/logs/<build-id>
    pub events: PathBuf,     // root/events.jsonl
    pub manifest: PathBuf,   // root/manifest.toml
    pub nodes_dir: PathBuf,  // root/nodes
}

impl BuildPaths {
    pub fn new(base: &Path, build_id: &str) -> Self {
        let root = base.join(".cook").join("logs").join(build_id);
        Self {
            events: root.join("events.jsonl"),
            manifest: root.join("manifest.toml"),
            nodes_dir: root.join("nodes"),
            root,
        }
    }
}

/// Generate a short, sortable build id: YYYY-MM-DD-<6hex>.
pub fn new_build_id() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let (y, mo, d) = ymd_from_unix(secs);
    let rand = format!("{:06x}", (now.as_nanos() as u64) & 0x00ff_ffff);
    format!("{y:04}-{mo:02}-{d:02}-{rand}")
}

// Minimal Gregorian date arithmetic sufficient for build-id labels.
fn ymd_from_unix(secs: i64) -> (i64, u32, u32) {
    let days_since_epoch = secs.div_euclid(86_400);
    // Epoch = 1970-01-01 (Thursday)
    let (y, m, d) = civil_from_days(days_since_epoch);
    (y, m as u32, d as u32)
}

// H. Hinnant's algorithm for days_from_civil inverse.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_matches_date_format() {
        let id = new_build_id();
        // YYYY-MM-DD-<6hex>
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 4, "id={id}");
        assert_eq!(parts[0].len(), 4);
        assert_eq!(parts[1].len(), 2);
        assert_eq!(parts[2].len(), 2);
        assert_eq!(parts[3].len(), 6);
    }

    #[test]
    fn build_paths_places_everything_under_cook_logs() {
        let p = BuildPaths::new(Path::new("/tmp/proj"), "2026-04-18-abcdef");
        assert!(p.root.ends_with(".cook/logs/2026-04-18-abcdef"));
        assert!(p.events.ends_with("events.jsonl"));
        assert!(p.manifest.ends_with("manifest.toml"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::log_store 2>&1 | tail -10`
Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/src/v2/log_store.rs
git commit -m "feat(cook-progress): build-id generation and per-build paths"
```

---

### Task 19: `LogStore` — event + node writers with rotation

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/log_store.rs`

- [ ] **Step 1: Add the writer**

Append to `log_store.rs`, above `#[cfg(test)]`:

```rust
#[derive(Clone, Debug)]
pub struct LogStoreConfig {
    pub max_bytes_per_node: u64,    // per log file cap; further writes dropped with "--- truncated ---" marker
    pub max_total_bytes: u64,       // directory-wide cap across all historic builds
    pub keep_builds: usize,         // keep this many newest build dirs
}

impl Default for LogStoreConfig {
    fn default() -> Self {
        Self {
            max_bytes_per_node: 2 * 1024 * 1024,
            max_total_bytes: 500 * 1024 * 1024,
            keep_builds: 20,
        }
    }
}

pub struct LogStore {
    paths: BuildPaths,
    events: BufWriter<File>,
    node_writers: BTreeMap<(RecipeId, NodeId), NodeWriter>,
    names: BTreeMap<RecipeId, String>,
    node_labels: BTreeMap<(RecipeId, NodeId), String>,
    cfg: LogStoreConfig,
}

struct NodeWriter {
    file: BufWriter<File>,
    bytes: u64,
    truncated: bool,
    cap: u64,
}

impl LogStore {
    pub fn create(base: &Path, build_id: &str, cfg: LogStoreConfig) -> io::Result<Self> {
        let paths = BuildPaths::new(base, build_id);
        fs::create_dir_all(&paths.root)?;
        fs::create_dir_all(&paths.nodes_dir)?;
        let events = BufWriter::new(
            OpenOptions::new().create(true).append(true).open(&paths.events)?,
        );
        Ok(Self {
            paths,
            events,
            node_writers: BTreeMap::new(),
            names: BTreeMap::new(),
            node_labels: BTreeMap::new(),
            cfg,
        })
    }

    pub fn paths(&self) -> &BuildPaths {
        &self.paths
    }

    /// Write one event (already-serialized JSON line) to events.jsonl.
    pub fn write_event_json(&mut self, line_without_newline: &str) -> io::Result<()> {
        self.events.write_all(line_without_newline.as_bytes())?;
        self.events.write_all(b"\n")
    }

    /// Track topology + per-node labels so we can open files by name later.
    pub fn observe(&mut self, event: &ProgressEvent) {
        match event {
            ProgressEvent::BuildStarted { recipes, .. } => {
                for topo in recipes {
                    self.names.insert(topo.id, topo.name.clone());
                }
            }
            ProgressEvent::NodeStarted { recipe, node, label } => {
                self.node_labels.insert((*recipe, *node), label.clone());
            }
            _ => {}
        }
    }

    /// Append one output line to the per-node log file. Creates on first use.
    pub fn write_node_line(&mut self, recipe: RecipeId, node: NodeId, line: &str) -> io::Result<()> {
        let cap = self.cfg.max_bytes_per_node;
        let dir = &self.paths.nodes_dir;
        let recipe_name = self.names.get(&recipe).cloned().unwrap_or_else(|| format!("r{}", recipe.0));
        let node_label = self.node_labels.get(&(recipe, node)).cloned().unwrap_or_else(|| format!("n{}", node.0));
        let entry = self.node_writers.entry((recipe, node));
        let w = match entry {
            std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::btree_map::Entry::Vacant(v) => {
                let path = node_log_path(dir, &recipe_name, &node_label);
                if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
                let file = BufWriter::new(OpenOptions::new().create(true).append(true).open(&path)?);
                v.insert(NodeWriter { file, bytes: 0, truncated: false, cap })
            }
        };
        if w.truncated { return Ok(()); }
        let needed = line.len() as u64 + 1;
        if w.bytes + needed > w.cap {
            w.file.write_all(b"--- truncated ---\n")?;
            w.truncated = true;
            return Ok(());
        }
        w.file.write_all(line.as_bytes())?;
        w.file.write_all(b"\n")?;
        w.bytes += needed;
        Ok(())
    }

    /// Finalize writers and write manifest.
    pub fn finalize(mut self, command: &str, exit_code: i32) -> io::Result<()> {
        self.events.flush()?;
        for (_, mut w) in self.node_writers { let _ = w.file.flush(); }
        let manifest = Manifest {
            schema: 1,
            command: command.to_string(),
            exit_code,
            finished: true,
        };
        let toml_text = toml::to_string(&manifest)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        fs::write(&self.paths.manifest, toml_text)?;
        Ok(())
    }
}

fn node_log_path(nodes_dir: &Path, recipe: &str, node_label: &str) -> PathBuf {
    let safe = sanitize_for_fs(node_label);
    nodes_dir.join(recipe).join(format!("{safe}.log"))
}

fn sanitize_for_fs(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

#[derive(Serialize)]
struct Manifest {
    schema: u32,
    command: String,
    exit_code: i32,
    finished: bool,
}

/// Prune `.cook/logs/` so it satisfies both `keep_builds` and `max_total_bytes`.
pub fn prune_logs(base: &Path, cfg: &LogStoreConfig) -> io::Result<()> {
    let logs_dir = base.join(".cook").join("logs");
    if !logs_dir.exists() { return Ok(()); }
    // List child directories sorted descending (most recent first by name).
    let mut entries: Vec<_> = fs::read_dir(&logs_dir)?
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.file_name()));

    // Rule 1: keep at most `keep_builds`.
    for (idx, entry) in entries.iter().enumerate() {
        if idx >= cfg.keep_builds {
            let _ = fs::remove_dir_all(entry.path());
        }
    }

    // Rule 2: total bytes cap. Remove oldest-first until within bound.
    loop {
        let total: u64 = fs::read_dir(&logs_dir)?
            .filter_map(|r| r.ok())
            .map(|e| dir_size(&e.path()).unwrap_or(0))
            .sum();
        if total <= cfg.max_total_bytes { break; }
        let mut surviving: Vec<_> = fs::read_dir(&logs_dir)?
            .filter_map(|r| r.ok())
            .collect();
        surviving.sort_by_key(|e| e.file_name());
        let oldest = match surviving.first() {
            Some(e) => e.path(),
            None => break,
        };
        if fs::remove_dir_all(&oldest).is_err() { break; }
    }
    Ok(())
}

fn dir_size(path: &Path) -> io::Result<u64> {
    if path.is_file() {
        return Ok(fs::metadata(path)?.len());
    }
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let p = entry?.path();
        total += dir_size(&p)?;
    }
    Ok(total)
}
```

- [ ] **Step 2: Append tests**

Append to `log_store.rs` inside the `#[cfg(test)]` module:

```rust
    #[test]
    fn log_store_writes_events_and_node_logs_to_correct_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = LogStore::create(tmp.path(), "2026-04-18-test01", LogStoreConfig::default()).unwrap();
        store.observe(&ProgressEvent::BuildStarted {
            recipes: vec![crate::v2::event::RecipeTopo { id: RecipeId(0), name: "lib".into(), deps: vec![], expected_nodes: 1 }],
            total_nodes: 1,
        });
        store.observe(&ProgressEvent::NodeStarted {
            recipe: RecipeId(0), node: NodeId(1), label: "foo.c".into(),
        });
        store.write_event_json(r#"{"type":"build-started"}"#).unwrap();
        store.write_node_line(RecipeId(0), NodeId(1), "hello line").unwrap();
        store.finalize("cook build", 0).unwrap();

        let events = fs::read_to_string(tmp.path().join(".cook/logs/2026-04-18-test01/events.jsonl")).unwrap();
        assert!(events.contains("build-started"));
        let log = fs::read_to_string(tmp.path().join(".cook/logs/2026-04-18-test01/nodes/lib/foo.c.log")).unwrap();
        assert_eq!(log, "hello line\n");
        let manifest = fs::read_to_string(tmp.path().join(".cook/logs/2026-04-18-test01/manifest.toml")).unwrap();
        assert!(manifest.contains("schema = 1"));
    }

    #[test]
    fn log_store_emits_truncation_marker_when_node_log_exceeds_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = LogStoreConfig { max_bytes_per_node: 32, ..LogStoreConfig::default() };
        let mut store = LogStore::create(tmp.path(), "2026-04-18-test02", cfg).unwrap();
        store.observe(&ProgressEvent::BuildStarted {
            recipes: vec![crate::v2::event::RecipeTopo { id: RecipeId(0), name: "r".into(), deps: vec![], expected_nodes: 1 }],
            total_nodes: 1,
        });
        for _ in 0..20 {
            store.write_node_line(RecipeId(0), NodeId(0), "01234567890123456789").unwrap();
        }
        store.finalize("cook build", 0).unwrap();
        let log = fs::read_to_string(tmp.path().join(".cook/logs/2026-04-18-test02/nodes/r/n0.log")).unwrap();
        assert!(log.contains("--- truncated ---"));
        assert!(log.len() <= 200);
    }
```

- [ ] **Step 3: Add `tempfile` dev-dep**

Edit `cli/crates/cook-progress/Cargo.toml` → `[dev-dependencies]`, add:

```toml
tempfile = "3"
```

- [ ] **Step 4: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::log_store 2>&1 | tail -10`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-progress/src/v2/log_store.rs cli/crates/cook-progress/Cargo.toml cli/Cargo.lock
git commit -m "feat(cook-progress): LogStore with events.jsonl + per-node logs + manifest + rotation"
```

---

### Phase 5 checkpoint

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-progress 2>&1 | tail -5
```

Expected: all v2 tests pass.

---

## Phase 6 — TUI renderer (ratatui)

Three tasks: the tree widget, the log-pane widget, the event loop + key bindings. TUI tests use ratatui's `TestBackend` to assert cell-grid content.

### Task 20: TUI tree widget

**Files:**
- Create: `cli/crates/cook-progress/src/v2/render/tui_tree.rs`
- Modify: `cli/crates/cook-progress/src/v2/render/mod.rs`

- [ ] **Step 1: Register module**

Edit `render/mod.rs`:

```rust
pub mod inline;
pub mod json;
pub mod plain;
pub mod tui;
pub mod tui_tree;
pub mod tui_log;
```

- [ ] **Step 2: Write the tree widget**

Create `render/tui_tree.rs`:

```rust
//! Tree widget for the TUI left pane.

use crate::v2::event::RecipeId;
use crate::v2::model::build::BuildState;
use crate::v2::model::recipe::Status;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::collections::BTreeSet;

pub struct TreeState {
    pub selected: Option<RecipeId>,
    pub expanded: BTreeSet<RecipeId>,
}

impl TreeState {
    pub fn new() -> Self { Self { selected: None, expanded: BTreeSet::new() } }
}

pub struct TreeWidget<'a> {
    pub state: &'a BuildState,
    pub tree: &'a TreeState,
}

impl<'a> Widget for TreeWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default().title(" Recipes ").borders(Borders::RIGHT);
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines: Vec<Line> = Vec::new();
        for id in &self.state.order {
            let Some(r) = self.state.recipes.get(id) else { continue; };
            let is_sel = self.tree.selected == Some(*id);
            let sym = match r.status {
                Status::Waiting => "◇",
                Status::Running => "◆",
                Status::Completed => "✓",
                Status::Failed => "✗",
                Status::Cached => "≋",
            };
            let color = match r.status {
                Status::Running => Color::Cyan,
                Status::Completed | Status::Cached => Color::Green,
                Status::Failed => Color::Red,
                _ => Color::Gray,
            };
            let mut st = Style::default().fg(color);
            if is_sel { st = st.bg(Color::DarkGray).add_modifier(Modifier::BOLD); }
            lines.push(Line::from(vec![
                Span::styled(format!(" {sym} "), st),
                Span::styled(r.name.clone(), st),
                Span::raw(format!("  {}/{}", r.progress.0, r.progress.1)),
            ]));
            if self.tree.expanded.contains(id) {
                for n in &r.active_nodes {
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::raw(n.label.clone()),
                    ]));
                }
            }
        }
        Paragraph::new(lines).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::event::{ProgressEvent, RecipeTopo};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn tree_renders_every_recipe_from_order() {
        let mut s = BuildState::new(3);
        s.apply(ProgressEvent::BuildStarted {
            recipes: vec![
                RecipeTopo { id: RecipeId(0), name: "deps".into(), deps: vec![], expected_nodes: 1 },
                RecipeTopo { id: RecipeId(1), name: "lib".into(), deps: vec![RecipeId(0)], expected_nodes: 2 },
            ],
            total_nodes: 3,
        });
        let tree = TreeState::new();
        let backend = TestBackend::new(30, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| f.render_widget(TreeWidget { state: &s, tree: &tree }, f.size())).unwrap();
        let text = term.backend().buffer().content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(text.contains("deps"));
        assert!(text.contains("lib"));
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::render::tui_tree 2>&1 | tail -10`
Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/src/v2/render/
git commit -m "feat(cook-progress): TUI tree widget with TestBackend snapshot"
```

---

### Task 21: TUI log-pane widget

**Files:**
- Create: `cli/crates/cook-progress/src/v2/render/tui_log.rs`

- [ ] **Step 1: Write the widget**

Create `render/tui_log.rs`:

```rust
//! Log pane widget (right side). Reads the selected node's log file into a
//! scrollable paragraph. Follow-tail toggles whether we pin to the end.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

pub struct LogPaneState {
    pub lines: Vec<String>,
    pub scroll: u16,      // topmost row
    pub follow_tail: bool,
}

impl LogPaneState {
    pub fn new() -> Self { Self { lines: Vec::new(), scroll: 0, follow_tail: true } }

    pub fn set_lines(&mut self, lines: Vec<String>) {
        self.lines = lines;
        if self.follow_tail {
            self.scroll_to_bottom();
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll = self.lines.len().saturating_sub(1) as u16;
    }
}

pub struct LogPaneWidget<'a> {
    pub title: &'a str,
    pub state: &'a LogPaneState,
}

impl<'a> Widget for LogPaneWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default().title(self.title).borders(Borders::NONE);
        let inner = block.inner(area);
        block.render(area, buf);
        let lines: Vec<Line> = self.state.lines.iter().map(|l| Line::from(Span::raw(l.clone()))).collect();
        let mut p = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((self.state.scroll, 0));
        if !self.state.follow_tail {
            p = p.style(Style::default().add_modifier(Modifier::DIM));
        }
        p.render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn log_pane_renders_follow_tail_by_default() {
        let mut state = LogPaneState::new();
        state.set_lines(vec!["line 1".into(), "line 2".into()]);
        assert!(state.follow_tail);
        assert_eq!(state.scroll, 1);
        let backend = TestBackend::new(20, 4);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| f.render_widget(LogPaneWidget { title: " log ", state: &state }, f.size())).unwrap();
        let text = term.backend().buffer().content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(text.contains("line"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::render::tui_log 2>&1 | tail -5`
Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/src/v2/render/tui_log.rs
git commit -m "feat(cook-progress): TUI log-pane widget with follow-tail"
```

---

### Task 22: TUI app event loop

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/render/tui.rs`

- [ ] **Step 1: Implement the `TuiApp` struct + `run()` method**

Replace `render/tui.rs` with:

```rust
//! ratatui-based alt-screen renderer.

use crate::v2::event::RecipeId;
use crate::v2::log_store::BuildPaths;
use crate::v2::model::build::BuildState;
use crate::v2::render::tui_log::{LogPaneState, LogPaneWidget};
use crate::v2::render::tui_tree::{TreeState, TreeWidget};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use crossterm::{event, execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Terminal;
use std::fs;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct TuiApp {
    tree: TreeState,
    log: LogPaneState,
    paths: Option<BuildPaths>,
}

impl TuiApp {
    pub fn new(paths: Option<BuildPaths>) -> Self {
        Self { tree: TreeState::new(), log: LogPaneState::new(), paths }
    }

    /// Run the TUI event loop against a shared BuildState. Returns when the
    /// user quits (q/Esc) or the build finishes and the user hits q.
    pub fn run(&mut self, state: Arc<Mutex<BuildState>>) -> io::Result<()> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, terminal::EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut term = Terminal::new(backend)?;

        let tick = Duration::from_millis(100);
        loop {
            {
                let st = state.lock().unwrap();
                if self.tree.selected.is_none() {
                    self.tree.selected = st.order.first().copied();
                }
                if let Some(recipe_id) = self.tree.selected {
                    self.refresh_log(&st, recipe_id);
                }
                term.draw(|f| {
                    let chunks = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Length(30), Constraint::Min(10)])
                        .split(f.size());
                    f.render_widget(TreeWidget { state: &st, tree: &self.tree }, chunks[0]);
                    f.render_widget(LogPaneWidget { title: " log ", state: &self.log }, chunks[1]);
                })?;
            }

            if event::poll(tick)? {
                if let Event::Key(k) = event::read()? {
                    let handled = self.handle_key(k.code, k.modifiers, Arc::clone(&state));
                    if let KeyOutcome::Quit = handled { break; }
                }
            }
        }

        terminal::disable_raw_mode()?;
        execute!(term.backend_mut(), terminal::LeaveAlternateScreen)?;
        Ok(())
    }

    pub(crate) fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers, state: Arc<Mutex<BuildState>>) -> KeyOutcome {
        let st = state.lock().unwrap();
        match (code, mods) {
            (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => return KeyOutcome::Quit,
            (KeyCode::Char('j'), _) | (KeyCode::Down, _) => { self.move_select(&st, 1); }
            (KeyCode::Char('k'), _) | (KeyCode::Up, _) => { self.move_select(&st, -1); }
            (KeyCode::Char('f'), _) => {
                self.log.follow_tail = !self.log.follow_tail;
                if self.log.follow_tail { self.log.scroll_to_bottom(); }
            }
            (KeyCode::Char(' '), _) => {
                if let Some(id) = self.tree.selected {
                    if !self.tree.expanded.insert(id) { self.tree.expanded.remove(&id); }
                }
            }
            _ => {}
        }
        KeyOutcome::Continue
    }

    fn move_select(&mut self, st: &BuildState, delta: i32) {
        if st.order.is_empty() { return; }
        let cur = self.tree.selected.and_then(|s| st.order.iter().position(|x| *x == s)).unwrap_or(0) as i32;
        let n = st.order.len() as i32;
        let next = ((cur + delta).rem_euclid(n)) as usize;
        self.tree.selected = Some(st.order[next]);
    }

    fn refresh_log(&mut self, st: &BuildState, recipe_id: RecipeId) {
        let Some(r) = st.recipes.get(&recipe_id) else { return; };
        // Prefer on-disk full log; fall back to the tail ring if no paths.
        if let Some(paths) = &self.paths {
            let dir = paths.nodes_dir.join(&r.name);
            let content = collect_logs(&dir).unwrap_or_default();
            self.log.set_lines(content);
        } else {
            self.log.set_lines(r.output_tail.lines().into_iter().map(String::from).collect());
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum KeyOutcome { Continue, Quit }

fn collect_logs(dir: &std::path::Path) -> io::Result<Vec<String>> {
    if !dir.exists() { return Ok(Vec::new()); }
    let mut lines = Vec::new();
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_file() {
            let text = fs::read_to_string(&p)?;
            for line in text.lines() {
                lines.push(line.to_string());
            }
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::event::{ProgressEvent, RecipeTopo};
    use crossterm::event::KeyModifiers;

    #[test]
    fn j_advances_selection() {
        let mut s = BuildState::new(3);
        s.apply(ProgressEvent::BuildStarted {
            recipes: vec![
                RecipeTopo { id: RecipeId(0), name: "a".into(), deps: vec![], expected_nodes: 1 },
                RecipeTopo { id: RecipeId(1), name: "b".into(), deps: vec![], expected_nodes: 1 },
            ],
            total_nodes: 2,
        });
        let state = Arc::new(Mutex::new(s));
        let mut app = TuiApp::new(None);
        app.tree.selected = Some(RecipeId(0));
        let out = app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE, Arc::clone(&state));
        assert_eq!(out, KeyOutcome::Continue);
        assert_eq!(app.tree.selected, Some(RecipeId(1)));
    }

    #[test]
    fn q_quits() {
        let s = BuildState::new(3);
        let state = Arc::new(Mutex::new(s));
        let mut app = TuiApp::new(None);
        assert_eq!(app.handle_key(KeyCode::Char('q'), KeyModifiers::NONE, state), KeyOutcome::Quit);
    }

    #[test]
    fn space_toggles_expansion() {
        let mut s = BuildState::new(3);
        s.apply(ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo { id: RecipeId(0), name: "a".into(), deps: vec![], expected_nodes: 1 }],
            total_nodes: 1,
        });
        let state = Arc::new(Mutex::new(s));
        let mut app = TuiApp::new(None);
        app.tree.selected = Some(RecipeId(0));
        app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE, Arc::clone(&state));
        assert!(app.tree.expanded.contains(&RecipeId(0)));
        app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE, Arc::clone(&state));
        assert!(!app.tree.expanded.contains(&RecipeId(0)));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::render::tui 2>&1 | tail -10`
Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add cli/crates/cook-progress/src/v2/render/tui.rs
git commit -m "feat(cook-progress): TUI app event loop with j/k/space/q/f keys"
```

---

### Phase 6 checkpoint

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-progress 2>&1 | tail -5
```

Expected: all v2 tests pass.

---

## Phase 7 — `cook logs` subcommand

Adds a CLI for browsing historical build logs as plain text. No TUI.

### Task 23: `list` + `show` helpers in cook-progress

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/log_store.rs`

- [ ] **Step 1: Append helpers**

Append to `log_store.rs`, above `#[cfg(test)]`:

```rust
/// List all `.cook/logs/*` build ids, newest first (by dir name).
pub fn list_builds(base: &Path) -> io::Result<Vec<String>> {
    let logs_dir = base.join(".cook").join("logs");
    if !logs_dir.exists() { return Ok(Vec::new()); }
    let mut v: Vec<String> = fs::read_dir(&logs_dir)?
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    v.sort();
    v.reverse();
    Ok(v)
}

/// Read a single node's log as a String. `spec` = `"recipe:node"`.
pub fn read_node_log(base: &Path, build_id: &str, spec: &str) -> io::Result<String> {
    let (recipe, node) = spec.split_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "expected recipe:node"))?;
    let safe = sanitize_for_fs(node);
    let path = BuildPaths::new(base, build_id).nodes_dir.join(recipe).join(format!("{safe}.log"));
    fs::read_to_string(path)
}

/// Collect all failed nodes from events.jsonl of a given build.
pub fn failed_nodes(base: &Path, build_id: &str) -> io::Result<Vec<(String, String)>> {
    let events = fs::read_to_string(BuildPaths::new(base, build_id).events)?;
    let mut name_by_id: BTreeMap<u32, String> = BTreeMap::new();
    let mut out = Vec::new();
    for line in events.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) { Ok(x) => x, Err(_) => continue };
        match v["type"].as_str() {
            Some("build-started") => {
                if let Some(recipes) = v["recipes"].as_array() {
                    for r in recipes {
                        if let (Some(id), Some(name)) = (r["id"].as_u64(), r["name"].as_str()) {
                            name_by_id.insert(id as u32, name.to_string());
                        }
                    }
                }
            }
            Some("node-failed") => {
                let rid = v["recipe"].as_u64().unwrap_or(0) as u32;
                let nid = v["node"].as_u64().unwrap_or(0);
                let recipe = name_by_id.get(&rid).cloned().unwrap_or_else(|| format!("r{rid}"));
                out.push((recipe, format!("n{nid}")));
            }
            _ => {}
        }
    }
    Ok(out)
}
```

- [ ] **Step 2: Tests**

Append inside the existing `#[cfg(test)]` module:

```rust
    #[test]
    fn list_builds_returns_descending_order() {
        let tmp = tempfile::tempdir().unwrap();
        for id in ["2026-04-17-aaa", "2026-04-18-bbb", "2026-04-18-ccc"] {
            LogStore::create(tmp.path(), id, LogStoreConfig::default())
                .unwrap()
                .finalize("cook", 0)
                .unwrap();
        }
        let ids = list_builds(tmp.path()).unwrap();
        assert_eq!(ids, vec!["2026-04-18-ccc", "2026-04-18-bbb", "2026-04-17-aaa"]);
    }
```

- [ ] **Step 3: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::log_store 2>&1 | tail -5`
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/src/v2/log_store.rs
git commit -m "feat(cook-progress): list_builds / read_node_log / failed_nodes helpers"
```

---

### Task 24: `cook logs` CLI subcommand

**Files:**
- Modify: `cli/crates/cook-cli/src/cli.rs`
- Create: `cli/crates/cook-cli/src/cmd_logs.rs`
- Modify: `cli/crates/cook-cli/src/main.rs`

- [ ] **Step 1: Add the subcommand enum variant**

Find the subcommand enum in `cli/crates/cook-cli/src/cli.rs` (search for `enum Command` or similar). Add a new variant:

```rust
    /// Show logs from previous builds.
    Logs {
        /// `recipe:node` to show, or omit to list builds.
        spec: Option<String>,
        /// Build id (default: latest).
        #[arg(long)]
        build: Option<String>,
        /// Dump all failed nodes from the latest build.
        #[arg(long)]
        failed: bool,
    },
```

- [ ] **Step 2: Implement the command handler**

Create `cli/crates/cook-cli/src/cmd_logs.rs`:

```rust
//! cook logs — inspect persistent build logs.

use crate::error::CookError;
use cook_progress::v2::log_store;
use std::path::Path;

pub fn cmd_logs(
    project_dir: &Path,
    spec: Option<String>,
    build: Option<String>,
    failed: bool,
) -> Result<(), CookError> {
    let build_id = match build {
        Some(id) => id,
        None => log_store::list_builds(project_dir)
            .map_err(|e| CookError::Other(format!("list_builds: {e}")))?
            .into_iter()
            .next()
            .ok_or_else(|| CookError::Other("no previous builds found".into()))?,
    };

    if failed {
        let failures = log_store::failed_nodes(project_dir, &build_id)
            .map_err(|e| CookError::Other(format!("failed_nodes: {e}")))?;
        for (recipe, node) in failures {
            let s = log_store::read_node_log(project_dir, &build_id, &format!("{recipe}:{node}"))
                .unwrap_or_default();
            println!("── {recipe}:{node} ──\n{s}");
        }
        return Ok(());
    }

    match spec {
        None => {
            for id in log_store::list_builds(project_dir)
                .map_err(|e| CookError::Other(format!("list_builds: {e}")))? {
                println!("{id}");
            }
        }
        Some(s) => {
            let content = log_store::read_node_log(project_dir, &build_id, &s)
                .map_err(|e| CookError::Other(format!("read_node_log: {e}")))?;
            print!("{content}");
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Wire the handler in `main.rs`**

Edit `cli/crates/cook-cli/src/main.rs`. Add `mod cmd_logs;` near the other `mod` declarations, and add a match arm for the `Logs` variant. For example:

```rust
        Command::Logs { spec, build, failed } => {
            let project_dir = std::env::current_dir().map_err(|e| CookError::Other(e.to_string()))?;
            cmd_logs::cmd_logs(&project_dir, spec, build, failed)?;
        }
```

Also add `cook-progress` to `cook-cli/Cargo.toml` `[dependencies]` if not already present (it is — confirm).

- [ ] **Step 4: Build and smoke-test**

```bash
cd /home/alex/dev/cook/cli
cargo build -p cook-cli 2>&1 | tail -5
# Dry smoke test: no .cook/logs yet → should say "no previous builds found"
cargo run -p cook-cli -- logs 2>&1 | tail -5
```

Expected: builds cleanly; `cook logs` errors with "no previous builds found" (exit code 1), since there are no logs persisted yet.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-cli/src/cli.rs cli/crates/cook-cli/src/main.rs cli/crates/cook-cli/src/cmd_logs.rs
git commit -m "feat(cook-cli): cook logs subcommand"
```

---

### Phase 7 checkpoint

```bash
cd /home/alex/dev/cook/cli && cargo test 2>&1 | tail -5
```

Expected: all tests pass across the workspace.

---

## Phase 8 — `cook recap` subcommand

Replay `events.jsonl` through `BuildState`, then launch the same TUI.

### Task 25: `replay_events` helper

**Files:**
- Modify: `cli/crates/cook-progress/src/v2/log_store.rs`

- [ ] **Step 1: Add the replayer**

Append to `log_store.rs`, above `#[cfg(test)]`:

```rust
use crate::v2::model::build::BuildState;

/// Read events.jsonl and apply every event to a fresh BuildState.
/// Ignores lines that don't parse (forward compat with future schema tweaks).
pub fn replay_events(base: &Path, build_id: &str, tail_lines: usize) -> io::Result<BuildState> {
    let events = fs::read_to_string(BuildPaths::new(base, build_id).events)?;
    let mut state = BuildState::new(tail_lines);
    for line in events.lines() {
        let stripped = strip_envelope(line);
        if let Ok(ev) = serde_json::from_str::<ProgressEvent>(&stripped) {
            state.apply(ev);
        }
    }
    Ok(state)
}

/// The envelope adds `ts` and `v` — strip them so the inner event roundtrips.
fn strip_envelope(line: &str) -> String {
    let mut v: serde_json::Value = match serde_json::from_str(line) {
        Ok(x) => x,
        Err(_) => return String::new(),
    };
    if let Some(obj) = v.as_object_mut() {
        obj.remove("ts");
        obj.remove("v");
    }
    v.to_string()
}
```

- [ ] **Step 2: Add a roundtrip test**

Append inside the `#[cfg(test)]` module:

```rust
    #[test]
    fn replay_events_reconstructs_terminal_state() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = LogStore::create(tmp.path(), "2026-04-18-rt", LogStoreConfig::default()).unwrap();

        use crate::v2::render::json::JsonRenderer;
        let jr = JsonRenderer::new();
        let canned = [
            ProgressEvent::BuildStarted {
                recipes: vec![crate::v2::event::RecipeTopo { id: RecipeId(0), name: "lib".into(), deps: vec![], expected_nodes: 1 }],
                total_nodes: 1,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId(0) },
            ProgressEvent::NodeStarted { recipe: RecipeId(0), node: NodeId(1), label: "a".into() },
            ProgressEvent::NodeCompleted { recipe: RecipeId(0), node: NodeId(1), elapsed: std::time::Duration::from_millis(5) },
            ProgressEvent::RecipeCompleted { recipe: RecipeId(0), elapsed: std::time::Duration::from_millis(10), cached: 0, total: 1 },
            ProgressEvent::Finished { success: true },
        ];
        for ev in &canned {
            let mut buf = Vec::new();
            jr.handle(ev, &mut buf).unwrap();
            let s = String::from_utf8(buf).unwrap();
            store.write_event_json(s.trim_end()).unwrap();
        }
        store.finalize("cook build", 0).unwrap();

        let state = replay_events(tmp.path(), "2026-04-18-rt", 3).unwrap();
        assert_eq!(state.order, vec![RecipeId(0)]);
        assert!(state.success);
        assert_eq!(state.recipes[&RecipeId(0)].status, crate::v2::model::recipe::Status::Completed);
    }
```

- [ ] **Step 3: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-progress --lib v2::log_store 2>&1 | tail -5`
Expected: all pass including the new replay test.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress/src/v2/log_store.rs
git commit -m "feat(cook-progress): replay_events reconstructs BuildState from events.jsonl"
```

---

### Task 26: `cook recap` CLI subcommand

**Files:**
- Modify: `cli/crates/cook-cli/src/cli.rs`
- Create: `cli/crates/cook-cli/src/cmd_recap.rs`
- Modify: `cli/crates/cook-cli/src/main.rs`

- [ ] **Step 1: Add the subcommand variant**

In `cli.rs`, add a new variant:

```rust
    /// Open the TUI against a past build.
    Recap {
        /// Specific build id.
        build: Option<String>,
        /// List all builds and exit.
        #[arg(long)]
        list: bool,
        /// Jump to the last failed build.
        #[arg(long = "last-failed")]
        last_failed: bool,
    },
```

- [ ] **Step 2: Create the handler**

Create `cli/crates/cook-cli/src/cmd_recap.rs`:

```rust
//! cook recap — replay past builds through the TUI.

use crate::error::CookError;
use cook_progress::v2::log_store::{self, BuildPaths};
use cook_progress::v2::render::tui::TuiApp;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub fn cmd_recap(
    project_dir: &Path,
    build: Option<String>,
    list: bool,
    last_failed: bool,
) -> Result<(), CookError> {
    if list {
        for id in log_store::list_builds(project_dir)
            .map_err(|e| CookError::Other(format!("list_builds: {e}")))? {
            println!("{id}");
        }
        return Ok(());
    }

    let build_id = match (build, last_failed) {
        (Some(id), _) => id,
        (None, true) => pick_last_failed(project_dir)?,
        (None, false) => log_store::list_builds(project_dir)
            .map_err(|e| CookError::Other(format!("list_builds: {e}")))?
            .into_iter()
            .next()
            .ok_or_else(|| CookError::Other("no previous builds".into()))?,
    };

    let state = log_store::replay_events(project_dir, &build_id, 3)
        .map_err(|e| CookError::Other(format!("replay_events: {e}")))?;
    let paths = BuildPaths::new(project_dir, &build_id);

    let mut app = TuiApp::new(Some(paths));
    app.run(Arc::new(Mutex::new(state)))
        .map_err(|e| CookError::Other(format!("tui: {e}")))?;
    Ok(())
}

fn pick_last_failed(base: &Path) -> Result<String, CookError> {
    for id in log_store::list_builds(base)
        .map_err(|e| CookError::Other(format!("list_builds: {e}")))?
    {
        let fails = log_store::failed_nodes(base, &id)
            .map_err(|e| CookError::Other(format!("failed_nodes: {e}")))?;
        if !fails.is_empty() { return Ok(id); }
    }
    Err(CookError::Other("no failed builds found".into()))
}
```

- [ ] **Step 3: Wire the handler**

Edit `cli/crates/cook-cli/src/main.rs`. Add `mod cmd_recap;` and the match arm:

```rust
        Command::Recap { build, list, last_failed } => {
            let project_dir = std::env::current_dir().map_err(|e| CookError::Other(e.to_string()))?;
            cmd_recap::cmd_recap(&project_dir, build, list, last_failed)?;
        }
```

- [ ] **Step 4: Smoke test**

```bash
cd /home/alex/dev/cook/cli
cargo build -p cook-cli 2>&1 | tail -5
cargo run -p cook-cli -- recap --list 2>&1 | tail -5
```

Expected: builds cleanly. `recap --list` prints nothing (no builds yet) or exits 0.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-cli
git commit -m "feat(cook-cli): cook recap subcommand"
```

---

### Phase 8 checkpoint

```bash
cd /home/alex/dev/cook/cli && cargo test 2>&1 | tail -5
```

Expected: all tests pass.

---

## Phase 9 — Wire cook-engine + swap cook-cli over

Evolve `EngineEvent` (add `BuildStarted`, change `OutputLine` to carry node name, add `SkipReason`), and swap `cook-cli::progress` over to the new `cook-progress::v2` renderers. Legacy `cook-progress::Renderer` stays valid (kept compiling) but is no longer called.

### Task 27: Evolve `EngineEvent`

**Files:**
- Modify: `cli/crates/cook-engine/src/lib.rs`
- Modify: `cli/crates/cook-engine/src/executor.rs`
- Modify: `cli/crates/cook-engine/src/run.rs`

- [ ] **Step 1: Add `BuildStarted` + `SkipReason` + node-scoped `OutputLine`**

Edit `cook-engine/src/lib.rs` around lines 60–133. Add the imports and update the enum:

```rust
use cook_contracts::{CacheMeta, WorkPayload};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    UpstreamFailed,
    ConditionFalse,
    Disabled,
}

#[derive(Debug, Clone)]
pub struct RecipeTopo {
    pub name: String,
    pub deps: Vec<String>,
    pub expected_nodes: usize,
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    BuildStarted {
        recipes: Vec<RecipeTopo>,
        total_nodes: usize,
    },
    RecipeQueued { name: String },
    RecipeStarted { name: String, total_nodes: usize },
    RecipeCompleted { name: String, elapsed: Duration, cached_nodes: usize, total_nodes: usize },
    RecipeFailed { name: String, elapsed: Duration, completed_nodes: usize, total_nodes: usize },
    NodeStarted { recipe: String, node_name: String },
    NodeCompleted { recipe: String, node_name: String, elapsed: Duration },
    NodeFailed { recipe: String, node_name: String, elapsed: Duration, error: String },
    NodeCacheHit { recipe: String, node_name: String },
    NodeSkipped { recipe: String, node_name: String, reason: SkipReason },
    InteractiveStart { recipe: String, node_name: String },
    InteractiveEnd { recipe: String, node_name: String, elapsed: Duration, success: bool },
    OutputLine { recipe: String, node_name: String, line: String, is_stderr: bool },
    Finished { elapsed: Duration, success: bool },
}
```

- [ ] **Step 2: Emit `BuildStarted` in `run.rs`**

Find the top of `cook_engine::run::run` (in `run.rs`) where the executor is launched. Before the main executor loop starts, construct a `Vec<RecipeTopo>` from the recipe infos + inferred deps and emit `EngineEvent::BuildStarted`.

Locate the function `run(...)` in `cli/crates/cook-engine/src/run.rs` (grep for `pub fn run`). Just after the recipe order / dag is computed, add:

```rust
    let topo: Vec<RecipeTopo> = recipe_order
        .iter()
        .map(|name| RecipeTopo {
            name: name.clone(),
            deps: dag.deps_of(name).cloned().unwrap_or_default(),
            expected_nodes: work_units_per_recipe.get(name).copied().unwrap_or(0),
        })
        .collect();
    let total_nodes: usize = topo.iter().map(|t| t.expected_nodes).sum();
    emit(&event_tx, EngineEvent::BuildStarted { recipes: topo, total_nodes });
```

Replace the variable names (`recipe_order`, `dag`, `work_units_per_recipe`) with whatever the actual `run.rs` uses. Inspect the function first; it already has an `emit` helper and a recipe traversal.

- [ ] **Step 3: Change `OutputLine` emission sites to include `node_name`**

In `cli/crates/cook-engine/src/executor.rs` at lines 522–531 and 574–584, replace:

```rust
                for line in &result.output_lines {
                    emit(
                        &event_tx,
                        EngineEvent::OutputLine {
                            recipe: recipe_name.clone(),
                            line: line.clone(),
                            is_stderr: false,
                        },
                    );
                }
```

with:

```rust
                for line in &result.output_lines {
                    emit(
                        &event_tx,
                        EngineEvent::OutputLine {
                            recipe: recipe_name.clone(),
                            node_name: result.node_name.clone(),
                            line: line.clone(),
                            is_stderr: false,
                        },
                    );
                }
```

(Both occurrences — success path around line 522 and failure path around line 574.)

- [ ] **Step 4: Update `NodeSkipped` emission sites to include a reason**

Search for `EngineEvent::NodeSkipped {` in `executor.rs` and `run.rs`. Every call site must pass `reason: SkipReason::UpstreamFailed` (since today's only skip path is via `cancel_subtree`). Example:

```rust
EngineEvent::NodeSkipped {
    recipe: rn.clone(),
    node_name: nn.clone(),
    reason: SkipReason::UpstreamFailed,
}
```

- [ ] **Step 5: Verify the engine builds**

Run: `cd /home/alex/dev/cook/cli && cargo build -p cook-engine 2>&1 | tail -10`
Expected: compiles. If fields are missing on callers, add them.

- [ ] **Step 6: Verify downstream compiles (expect breakage in pipeline.rs)**

Run: `cd /home/alex/dev/cook/cli && cargo build -p cook-cli 2>&1 | tail -20`
Expected: `bridge_engine_events` in `pipeline.rs` won't compile (missing new fields). That's fine — Task 28 rewrites it.

- [ ] **Step 7: Commit**

```bash
git add cli/crates/cook-engine
git commit -m "feat(cook-engine): evolve EngineEvent (BuildStarted, node-scoped OutputLine, SkipReason)"
```

---

### Task 28: Rewrite `cook-cli::progress` using v2

**Files:**
- Modify: `cli/crates/cook-cli/src/progress.rs`
- Modify: `cli/crates/cook-cli/src/pipeline.rs`

- [ ] **Step 1: Replace `progress.rs` with a thin v2 adapter**

Replace `cli/crates/cook-cli/src/progress.rs` with:

```rust
//! Progress renderer adapter. Translates EngineEvent → cook-progress v2 ProgressEvent
//! and drives the v2 renderers based on CLI mode selection.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::sync::mpsc;

use crate::cli::Cli;
use crate::color::{ColorConfig, ColorMode};
use cook_progress::v2::event::{NodeId, ProgressEvent, RecipeId, RecipeTopo, SkipReason, Stream};
use cook_progress::v2::log_store::{self, LogStore, LogStoreConfig};
use cook_progress::v2::model::build::BuildState;
use cook_progress::v2::render::inline::{InlineConfig, InlineRenderer};
use cook_progress::v2::render::json::JsonRenderer;
use cook_progress::v2::render::plain::PlainRenderer;
use cook_progress::v2::style::Style;

pub enum OutputMode { Inline, Plain, Json, Tui }

pub struct ProgressSession {
    pub state: Arc<Mutex<BuildState>>,
    pub ids: Arc<Mutex<IdMap>>,
    pub store: Arc<Mutex<Option<LogStore>>>,
    pub mode: OutputMode,
}

#[derive(Default)]
pub struct IdMap {
    recipes: BTreeMap<String, RecipeId>,
    nodes: BTreeMap<(String, String), NodeId>,
    next_recipe: u32,
    next_node: u32,
}

impl IdMap {
    pub fn recipe_id(&mut self, name: &str) -> RecipeId {
        if let Some(id) = self.recipes.get(name) { return *id; }
        let id = RecipeId(self.next_recipe);
        self.next_recipe += 1;
        self.recipes.insert(name.to_string(), id);
        id
    }
    pub fn node_id(&mut self, recipe: &str, node: &str) -> NodeId {
        let key = (recipe.to_string(), node.to_string());
        if let Some(id) = self.nodes.get(&key) { return *id; }
        let id = NodeId(self.next_node);
        self.next_node += 1;
        self.nodes.insert(key, id);
        id
    }
}

pub fn resolve_output_mode(cli: &Cli) -> OutputMode {
    // Priority: --output=json > --output=plain > --ui > --no-ui > auto
    if cli.output.as_deref() == Some("json") { return OutputMode::Json; }
    if cli.output.as_deref() == Some("plain") { return OutputMode::Plain; }
    if cli.ui { return OutputMode::Tui; }
    if cli.no_ui { return OutputMode::Inline; }
    if !std::io::stderr().is_terminal() { return OutputMode::Plain; }
    if std::env::var("CI").is_ok() { return OutputMode::Plain; }
    if std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false) { return OutputMode::Plain; }
    OutputMode::Inline
}

pub fn resolve_color(cli: &Cli) -> ColorConfig {
    let mode: ColorMode = cli.color.parse().unwrap_or(ColorMode::Auto);
    ColorConfig::resolve(mode, std::io::stderr().is_terminal(), std::env::var("NO_COLOR").is_ok())
}

/// Spawn the renderer thread. It owns a BuildState + LogStore, consumes
/// EngineEvents, converts them to v2 ProgressEvents, applies them, and draws.
pub fn spawn_renderer_thread(
    engine_rx: mpsc::Receiver<cook_engine::EngineEvent>,
    color: ColorConfig,
    mode: OutputMode,
    project_dir: &Path,
    command: String,
) -> std::thread::JoinHandle<()> {
    let project_dir = project_dir.to_path_buf();
    std::thread::spawn(move || {
        let ids = Arc::new(Mutex::new(IdMap::default()));
        let state = Arc::new(Mutex::new(BuildState::new(3)));

        let build_id = log_store::new_build_id();
        let store = LogStore::create(&project_dir, &build_id, LogStoreConfig::default()).ok();
        let store = Arc::new(Mutex::new(store));

        let inline = InlineRenderer::new(InlineConfig {
            width: crossterm::terminal::size().map(|(c,_)| c).unwrap_or(80),
            tail_lines: 3,
            style: Style::unicode(color.enabled),
        });
        let plain = Arc::new(Mutex::new(PlainRenderer::new()));
        let jr = JsonRenderer::new();

        let mut last_height: usize = 0;
        let mut exit_code: i32 = 0;

        while let Ok(engine_ev) = engine_rx.recv() {
            let is_finished = matches!(engine_ev, cook_engine::EngineEvent::Finished { .. });

            // Translate and dispatch.
            let v2_events = translate_event(&ids, engine_ev);
            for ev in v2_events {
                if let ProgressEvent::Finished { success } = &ev {
                    if !success { exit_code = 1; }
                }
                {
                    let mut st = state.lock().unwrap();
                    st.apply(ev.clone());
                }
                if let Some(s) = store.lock().unwrap().as_mut() {
                    s.observe(&ev);
                    let mut buf = Vec::new();
                    if jr.handle(&ev, &mut buf).is_ok() {
                        let line = String::from_utf8_lossy(&buf);
                        let _ = s.write_event_json(line.trim_end());
                    }
                    if let ProgressEvent::NodeOutput { recipe, node, line, .. } = &ev {
                        let _ = s.write_node_line(*recipe, *node, line);
                    }
                }
                match mode {
                    OutputMode::Inline => {
                        let st = state.lock().unwrap();
                        let mut sink = std::io::stderr();
                        let _ = inline.clear_last_frame(last_height, &mut sink);
                        last_height = inline.write_frame(&st, &mut sink).unwrap_or(0);
                    }
                    OutputMode::Plain => {
                        let mut pr = plain.lock().unwrap();
                        let _ = pr.handle(&ev, &mut std::io::stderr());
                    }
                    OutputMode::Json => {
                        let _ = jr.handle(&ev, &mut std::io::stdout());
                    }
                    OutputMode::Tui => { /* TUI runs on main thread; see cmd_run */ }
                }
            }

            if is_finished { break; }
        }

        if let Some(s) = store.lock().unwrap().take() {
            let _ = s.finalize(&command, exit_code);
        }
        let _ = log_store::prune_logs(&project_dir, &LogStoreConfig::default());
        let _ = std::io::stderr().flush();
    })
}

fn translate_event(ids: &Arc<Mutex<IdMap>>, ev: cook_engine::EngineEvent) -> Vec<ProgressEvent> {
    use cook_engine::EngineEvent as E;
    let mut out = Vec::new();
    let mut m = ids.lock().unwrap();
    match ev {
        E::BuildStarted { recipes, total_nodes } => {
            let v2_recipes: Vec<RecipeTopo> = recipes
                .into_iter()
                .map(|r| RecipeTopo {
                    id: m.recipe_id(&r.name),
                    name: r.name.clone(),
                    deps: r.deps.iter().map(|d| m.recipe_id(d)).collect(),
                    expected_nodes: r.expected_nodes,
                })
                .collect();
            out.push(ProgressEvent::BuildStarted { recipes: v2_recipes, total_nodes });
        }
        E::RecipeQueued { .. } => {}
        E::RecipeStarted { name, .. } => {
            out.push(ProgressEvent::RecipeStarted { recipe: m.recipe_id(&name) });
        }
        E::RecipeCompleted { name, elapsed, cached_nodes, total_nodes } => {
            out.push(ProgressEvent::RecipeCompleted {
                recipe: m.recipe_id(&name), elapsed, cached: cached_nodes, total: total_nodes,
            });
        }
        E::RecipeFailed { name, elapsed, completed_nodes, total_nodes } => {
            out.push(ProgressEvent::RecipeFailed {
                recipe: m.recipe_id(&name), elapsed, completed: completed_nodes, total: total_nodes,
            });
        }
        E::NodeStarted { recipe, node_name } => {
            out.push(ProgressEvent::NodeStarted {
                recipe: m.recipe_id(&recipe),
                node: m.node_id(&recipe, &node_name),
                label: node_name,
            });
        }
        E::NodeCompleted { recipe, node_name, elapsed } => {
            out.push(ProgressEvent::NodeCompleted {
                recipe: m.recipe_id(&recipe), node: m.node_id(&recipe, &node_name), elapsed,
            });
        }
        E::NodeFailed { recipe, node_name, elapsed, error } => {
            out.push(ProgressEvent::NodeFailed {
                recipe: m.recipe_id(&recipe), node: m.node_id(&recipe, &node_name), elapsed, error,
            });
        }
        E::NodeCacheHit { recipe, node_name } => {
            out.push(ProgressEvent::NodeCacheHit {
                recipe: m.recipe_id(&recipe), node: m.node_id(&recipe, &node_name),
            });
        }
        E::NodeSkipped { recipe, node_name, reason } => {
            let r = match reason {
                cook_engine::SkipReason::UpstreamFailed => SkipReason::UpstreamFailed,
                cook_engine::SkipReason::ConditionFalse => SkipReason::ConditionFalse,
                cook_engine::SkipReason::Disabled => SkipReason::Disabled,
            };
            out.push(ProgressEvent::NodeSkipped {
                recipe: m.recipe_id(&recipe), node: m.node_id(&recipe, &node_name), reason: r,
            });
        }
        E::InteractiveStart { recipe, node_name } => {
            out.push(ProgressEvent::InteractiveStart {
                recipe: m.recipe_id(&recipe), node: m.node_id(&recipe, &node_name),
            });
        }
        E::InteractiveEnd { recipe, node_name, elapsed, success } => {
            out.push(ProgressEvent::InteractiveEnd {
                recipe: m.recipe_id(&recipe), node: m.node_id(&recipe, &node_name), elapsed, success,
            });
        }
        E::OutputLine { recipe, node_name, line, is_stderr } => {
            out.push(ProgressEvent::NodeOutput {
                recipe: m.recipe_id(&recipe),
                node: m.node_id(&recipe, &node_name),
                line,
                stream: if is_stderr { Stream::Stderr } else { Stream::Stdout },
            });
        }
        E::Finished { success, .. } => {
            out.push(ProgressEvent::Finished { success });
        }
    }
    out
}
```

- [ ] **Step 2: Update `pipeline.rs::run_with_progress`**

Edit `pipeline.rs`. Delete `bridge_engine_events` entirely and rewrite `run_with_progress` to dispatch through the new thread directly:

```rust
fn run_with_progress(
    cli: &Cli,
    recipe_infos: &BTreeMap<String, cook_engine::analyzer::RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, (cook_register::Registry, String)>,
    num_jobs: usize,
    inferred_deps: &BTreeMap<String, Vec<String>>,
) -> Result<cook_engine::run::RunResult, CookError> {
    let color = crate::progress::resolve_color(cli);
    let mode = crate::progress::resolve_output_mode(cli);

    let project_dir = cli.file.parent().unwrap_or(Path::new(".")).to_path_buf();
    let command = format!("cook {}", targets.join(" "));
    let (engine_tx, engine_rx) = mpsc::channel::<cook_engine::EngineEvent>();
    let render_thread = crate::progress::spawn_renderer_thread(engine_rx, color, mode, &project_dir, command);

    let result = cook_engine::run::run(recipe_infos, targets, registries, num_jobs, inferred_deps, move |event| {
        let _ = engine_tx.send(event);
    });

    let _ = render_thread.join();
    result.map_err(engine_error_to_cook_error)
}
```

Remove the old imports (`use crate::progress::{resolve_color, spawn_renderer_thread, ProgressEvent};`) and leave the new ones in place.

- [ ] **Step 3: Add `--ui`, `--no-ui`, `--output` to `cli.rs`**

Find the `Cli` struct in `cli/crates/cook-cli/src/cli.rs` and add:

```rust
    /// Force the alt-screen TUI renderer.
    #[arg(long, global = true)]
    pub ui: bool,

    /// Disable the alt-screen TUI (use inline even if CI sees a TTY).
    #[arg(long = "no-ui", global = true)]
    pub no_ui: bool,

    /// Output mode: plain | json. Default: auto.
    #[arg(long, global = true)]
    pub output: Option<String>,
```

- [ ] **Step 4: Build and smoke test**

```bash
cd /home/alex/dev/cook/cli
cargo build 2>&1 | tail -10
# Run an example build (there's one under examples/cpp-project)
cargo run -p cook-cli -- --file ../examples/cpp-project/Cookfile run build 2>&1 | tail -20
```

Expected: builds cleanly. The cpp-project example runs with the new inline renderer; a `.cook/logs/<build-id>/` directory appears under the example.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-cli
git commit -m "feat(cook-cli): swap progress renderer to cook-progress v2"
```

---

### Phase 9 checkpoint

```bash
cd /home/alex/dev/cook/cli && cargo test 2>&1 | tail -5
```

Expected: all tests pass; a smoke run of `cook run` on an example uses the new renderer.

---

## Phase 10 — Delete old code

Everything in `cook-progress::v2::*` is authoritative. Tear out the legacy surface.

### Task 29: Remove legacy `cook-progress` modules

**Files:**
- Delete: `cli/crates/cook-progress/src/bar.rs`
- Delete: `cli/crates/cook-progress/src/frame.rs`
- Delete: `cli/crates/cook-progress/src/output.rs`
- Delete: `cli/crates/cook-progress/src/renderer.rs`
- Delete: `cli/crates/cook-progress/src/symbols.rs`
- Delete: `cli/crates/cook-progress/examples/basic.rs`
- Delete: `cli/crates/cook-progress/examples/failure.rs`
- Delete: `cli/crates/cook-progress/examples/kitchen_sink.rs`
- Delete: `cli/crates/cook-progress/examples/parallel.rs`
- Delete: `cli/crates/cook-progress/examples/stress.rs`
- Modify: `cli/crates/cook-progress/src/lib.rs`

- [ ] **Step 1: Delete legacy files**

```bash
cd /home/alex/dev/cook
rm cli/crates/cook-progress/src/{bar,frame,output,renderer,symbols}.rs
rm cli/crates/cook-progress/examples/{basic,failure,kitchen_sink,parallel,stress}.rs
```

- [ ] **Step 2: Collapse `lib.rs` to re-export v2**

Replace `cli/crates/cook-progress/src/lib.rs` with:

```rust
//! cook-progress — terminal progress rendering for Cook builds.
//!
//! Model-first architecture: a pure `BuildState` state machine consumes
//! `ProgressEvent`s; three renderers (inline / tui / plain) draw from it.
//! See `docs/superpowers/specs/2026-04-18-cook-output-experience-design.md`.

pub mod v2;

pub use v2::event::{NodeId, ProgressEvent, RecipeId, RecipeTopo, SkipReason, Stream};
pub use v2::model::build::BuildState;
pub use v2::model::recipe::{RecipeState, Status};
pub use v2::model::node::{NodeState, NodeStatus};
pub use v2::render::inline::{InlineConfig, InlineRenderer};
pub use v2::render::plain::PlainRenderer;
pub use v2::render::json::JsonRenderer;
pub use v2::render::tui::TuiApp;
pub use v2::log_store::{self, BuildPaths, LogStore, LogStoreConfig};
```

- [ ] **Step 3: Build & test**

```bash
cd /home/alex/dev/cook/cli
cargo build 2>&1 | tail -10
cargo test -p cook-progress 2>&1 | tail -10
```

Expected: compiles cleanly; all v2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-progress
git commit -m "refactor(cook-progress): delete legacy bar/frame/output/renderer/symbols modules"
```

---

### Task 30: Move `v2` contents up to top-level (remove the `v2` prefix)

Only after everything is green — this is a flat rename.

**Files:**
- Rename: `cli/crates/cook-progress/src/v2/*` → `cli/crates/cook-progress/src/*`

- [ ] **Step 1: Move files**

```bash
cd /home/alex/dev/cook/cli/crates/cook-progress/src
git mv v2/event.rs event.rs
git mv v2/layout.rs layout.rs
git mv v2/log_store.rs log_store.rs
git mv v2/style.rs style.rs
git mv v2/driver.rs driver.rs   # may not exist if still stubbed — skip if so
git mv v2/model model
git mv v2/render render
rmdir v2
rm v2/mod.rs 2>/dev/null || true
```

- [ ] **Step 2: Rewrite all `crate::v2::` imports**

Run: `cd /home/alex/dev/cook && grep -rnl 'crate::v2::' cli/crates/cook-progress/src` — edit each file and replace `crate::v2::` with `crate::`.

Also update re-exports in `lib.rs` (remove `v2::` from the paths).

In downstream crates (`cook-cli`), rewrite `cook_progress::v2::` → `cook_progress::`.

```bash
cd /home/alex/dev/cook
grep -rnl 'cook_progress::v2::' cli/crates/cook-cli/src
```

Edit each file to drop `::v2`.

- [ ] **Step 3: Build & test**

```bash
cd /home/alex/dev/cook/cli
cargo build 2>&1 | tail -10
cargo test 2>&1 | tail -10
```

Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add cli/crates
git commit -m "refactor(cook-progress): promote v2 modules to top-level"
```

---

### Task 31: Update architecture docs

**Files:**
- Modify: `docs/architecture/supporting-modules.md`

- [ ] **Step 1: Rewrite the cook-progress section**

Open `docs/architecture/supporting-modules.md`. Replace the `cook-progress` section (search for `cook-progress`) with:

```markdown
### `cook-progress`

Terminal progress rendering. Model-first architecture.

- `event.rs` — public `ProgressEvent` + typed `RecipeId` / `NodeId`
- `model/` — pure state machine (`BuildState::apply`)
- `render/inline.rs` — scroll-buffer renderer (default)
- `render/tui.rs` — ratatui alt-screen renderer
- `render/plain.rs` — pipe/CI text renderer
- `render/json.rs` — JSON-lines emitter
- `log_store.rs` — `.cook/logs/<build-id>/` writer + replayer
- `layout.rs` — width-aware truncation

Design spec: `docs/superpowers/specs/2026-04-18-cook-output-experience-design.md`
```

- [ ] **Step 2: Commit**

```bash
git add docs/architecture/supporting-modules.md
git commit -m "docs: update cook-progress architecture after v2 rewrite"
```

---

### Phase 10 checkpoint (final)

```bash
cd /home/alex/dev/cook/cli && cargo build && cargo test 2>&1 | tail -10
```

Expected: clean build, all tests pass. The rewrite is complete.

---

## Self-Review

Against the spec:

- **Goals** — Inline stability ✓ (Tasks 9–15), DAG-aware order ✓ (Task 7: `order` frozen at BuildStarted; Task 27 emits topo from engine), 3 output lines always ✓ (Task 5: OutputRing.padded; Task 12: frame fixed 4 per recipe), opt-in TUI ✓ (Tasks 20–22 + 28), plain/json ✓ (Tasks 16–17), log persistence ✓ (Tasks 18–19), recap ✓ (Tasks 25–26), pure model ✓ (Task 8).
- **Non-goals** — none introduced.
- **Interactive takeover** — Partially covered: engine already emits `InteractiveStart` / `InteractiveEnd`; v2 has the events. Known-gap: the inline renderer's leave-raw-mode/enter-raw-mode dance for interactive takeover is not implemented as its own task — lives inside `spawn_renderer_thread` where `InteractiveStart` currently does nothing visible. Flag this if it matters at review time; a follow-up task can add `renderer.pause()` / `renderer.resume()` hooks.
- **Hot-swap `u` key** — Task 22 implements the TUI app and accepts the state Arc; the `u`-key trigger that launches TuiApp from inline mode is not wired (it requires a non-blocking stdin reader in `spawn_renderer_thread`). Ship-gap acceptable; can be added post-Phase 10 as a small feature task that spins `TuiApp::run` on a keypress.
- **Replay timing (`cook recap --replay`)** — `replay_events` replays instantaneously (applies every event at once). Animated replay with original timing is NOT implemented; `--replay` flag not wired. Mark as a follow-up. Not in Phase 8 tasks.
- **Mode selection matrix** — Task 28 `resolve_output_mode` implements the precedence described in spec.
- **Per-node full-log file for TUI drill-in** — Task 22 `collect_logs` reads the node files; present.
- **Failure scrollback block** — The spec's "fenced error block" for inline mode's end-of-run summary is not rendered by the inline renderer. The renderer stops at the footer. Add a follow-up task to emit a trailing error block after `Finished { success: false }` in `spawn_renderer_thread`.

**Known follow-ups to flag to the user** (not in this plan, to keep scope honest):

1. Inline renderer pause/resume for interactive-command takeover.
2. `u`-key mid-build hot-swap from inline to TUI.
3. `cook recap --replay` animated timing.
4. End-of-run failure block (fenced error with node tail + path).

The remaining feature surface from the spec is covered.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-18-cook-output-experience.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints for review.

Which approach?
