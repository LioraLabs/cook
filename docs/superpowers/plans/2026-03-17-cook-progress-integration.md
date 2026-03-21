# Cook-Progress Integration Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Cook's indicatif-based TtyRenderer with the cook-progress crate, removing the indicatif dependency entirely.

**Architecture:** Keep the existing ProgressEvent/ProgressRenderer trait/event channel architecture unchanged. Rewrite TtyRenderer to build `cook_progress::Frame` from `RecipeRenderState` and call `cook_progress::Renderer` for rendering. PlainRenderer stays as-is.

**Tech Stack:** Rust, `cook-progress` (workspace crate), `crossterm`

**Spec:** `docs/superpowers/specs/2026-03-17-cook-progress-crate-design.md`

---

## File Structure

### Modified files:
- `Cargo.toml` (root) — replace `indicatif` with `cook-progress` dependency
- `src/scheduler/progress.rs` — rewrite `TtyRenderer` to use `cook-progress`, keep everything else
- `src/engine/pipeline.rs` — minor update to renderer thread (drain semantics change)

### Files NOT changed:
- `src/scheduler/color.rs` — kept as-is, cook-progress has its own color handling
- `src/scheduler/executor.rs` — no changes, events stay the same
- `src/scheduler/pool.rs` — no changes
- PlainRenderer — no changes
- ProgressEvent enum — no changes
- ProgressRenderer trait — no changes

---

## Chunk 1: Replace TtyRenderer

### Task 1: Update dependencies

**Files:**
- Modify: `Cargo.toml` (root)

- [ ] **Step 1: Replace indicatif with cook-progress**

In `Cargo.toml`, remove `indicatif = "0.17"` and add:

```toml
cook-progress = { path = "crates/cook-progress" }
```

- [ ] **Step 2: Verify workspace compiles**

Run: `cargo check`
Expected: Compilation errors in `progress.rs` (indicatif imports gone) — this is expected, we fix it in Task 2.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: replace indicatif dependency with cook-progress"
```

---

### Task 2: Rewrite TtyRenderer

**Files:**
- Modify: `src/scheduler/progress.rs`

This is the core task. Replace the indicatif-based TtyRenderer with one using cook-progress. Keep:
- `ProgressEvent` enum (unchanged)
- `ProgressRenderer` trait (unchanged)
- `PlainRenderer` (unchanged)
- `RecipeRenderState` (simplify — remove `last_output_lines` since cook-progress handles output buffering)

- [ ] **Step 1: Update imports**

Replace the indicatif import at the top of `src/scheduler/progress.rs`:

```rust
// REMOVE: use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
```

Add cook-progress imports:

```rust
use cook_progress::{Frame, Section, Status, ItemStatus, RenderConfig, Renderer};
```

- [ ] **Step 2: Simplify RecipeRenderState**

Remove `last_output_lines` field (cook-progress owns output buffering). Keep everything else since we still need per-recipe state tracking for building Frames.

Remove from the struct:
```rust
// REMOVE: pub last_output_lines: Vec<String>,
```

Remove from `new()`:
```rust
// REMOVE: last_output_lines: Vec::new(),
```

- [ ] **Step 3: Rewrite TtyRenderer struct**

Replace the TtyRenderer struct definition with:

```rust
pub struct TtyRenderer {
    renderer: Renderer,
    color: ColorConfig,
    pub recipes: BTreeMap<String, RecipeRenderState>,
    recipe_order: Vec<String>,
    total_recipes: usize,
    finished_recipes: usize,
    running_recipes: usize,
    waiting_recipes: usize,
    total_cached: usize,
    total_nodes: usize,
}
```

Fields removed: `multi`, `recipe_bars`, `footer_bar`, `bars_created`.
Fields added: `renderer` (cook_progress::Renderer), `out` (output buffer for drain).

- [ ] **Step 4: Rewrite TtyRenderer::new()**

```rust
impl TtyRenderer {
    pub fn new(color: ColorConfig) -> Self {
        let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
        let config = RenderConfig {
            width: cols,
            max_output_lines: 3,
            colors: color.is_enabled(),
            ..Default::default()
        };
        TtyRenderer {
            renderer: Renderer::new(config),
            color,
            recipes: BTreeMap::new(),
            recipe_order: Vec::new(),
            total_recipes: 0,
            finished_recipes: 0,
            running_recipes: 0,
            waiting_recipes: 0,
            total_cached: 0,
            total_nodes: 0,
        }
    }

    pub fn new_for_test(color: ColorConfig, width: u16) -> Self {
        let config = RenderConfig {
            width,
            max_output_lines: 3,
            colors: color.is_enabled(),
            ..Default::default()
        };
        TtyRenderer {
            renderer: Renderer::new(config),
            color,
            recipes: BTreeMap::new(),
            recipe_order: Vec::new(),
            total_recipes: 0,
            finished_recipes: 0,
            running_recipes: 0,
            waiting_recipes: 0,
            total_cached: 0,
            total_nodes: 0,
        }
    }

    pub fn drain(&self) -> Vec<u8> {
        Vec::new()
    }
}
```

Note: `ColorConfig` needs an `is_enabled()` method. Add to `src/scheduler/color.rs`:
```rust
pub fn is_enabled(&self) -> bool {
    self.enabled
}
```

- [ ] **Step 5: Rewrite update_state()**

Keep `update_state()` almost identical to current, but:
- Remove the `OutputLine` branch that manages `last_output_lines` (cook-progress handles this)
- Remove `last_output_lines.clear()` from `RecipeCompleted` branch

The OutputLine event will still be handled in `handle()` (see Step 7) — it just calls `self.renderer.push_output()` instead of updating a Vec.

- [ ] **Step 6: Add build_frame() method**

This is the key new method — builds a `cook_progress::Frame` from current state:

```rust
fn build_frame(&self) -> Frame {
    let mut frame = Frame::new();

    for name in &self.recipe_order {
        let state = match self.recipes.get(name) {
            Some(s) => s,
            None => continue,
        };

        let mut section = Section::new(name, name);

        if state.finished && !state.failed {
            // Completed
            let all_cached = state.total_nodes > 0
                && state.cached_nodes == state.total_nodes;
            if all_cached {
                section = section
                    .status(Status::Cached)
                    .progress(state.total_nodes, state.total_nodes);
            } else {
                section = section
                    .status(Status::Completed)
                    .progress(state.total_nodes, state.total_nodes)
                    .elapsed(state.start.elapsed());
                if state.cached_nodes > 0 {
                    section = section.cache(state.cached_nodes, state.total_nodes);
                }
            }
        } else if state.finished && state.failed {
            // Failed
            section = section
                .status(Status::Failed)
                .progress(state.completed_nodes, state.total_nodes)
                .elapsed(state.start.elapsed());
        } else if state.active_nodes.is_empty() && state.completed_nodes == 0 {
            // Waiting
            section = section.status(Status::Waiting);
        } else {
            // Running
            section = section
                .status(Status::Running)
                .progress(state.completed_nodes, state.total_nodes)
                .elapsed(state.start.elapsed());

            for node in &state.active_nodes {
                section = section.active_item(node, ItemStatus::Running);
            }
        }

        frame = frame.section(section);
    }

    // Footer
    let mut parts: Vec<String> = Vec::new();
    if self.finished_recipes > 0 {
        parts.push(format!("{} done", self.finished_recipes));
    }
    if self.running_recipes > 0 {
        parts.push(format!("{} running", self.running_recipes));
    }
    if self.waiting_recipes > 0 {
        parts.push(format!("{} waiting", self.waiting_recipes));
    }
    if self.total_cached > 0 {
        parts.push(format!("{}/{} cached", self.total_cached, self.total_nodes));
    }
    if !parts.is_empty() {
        frame = frame.footer(parts.join(" · "));
    }

    frame
}
```

- [ ] **Step 7: Rewrite handle()**

Remove all indicatif-specific code (create_all_bars, ensure_bar, refresh_recipe, refresh_footer, multi.println, multi.clear). Replace with cook-progress clear/render cycle:

```rust
impl ProgressRenderer for TtyRenderer {
    fn handle(&mut self, event: ProgressEvent) {
        let is_queue = matches!(event, ProgressEvent::RecipeQueued { .. });
        let is_interactive_start = matches!(event, ProgressEvent::InteractiveStart { .. });
        let is_interactive_end = matches!(event, ProgressEvent::InteractiveEnd { .. });
        let is_finished = matches!(event, ProgressEvent::Finished);

        // Push output lines to cook-progress renderer (before update_state consumes event)
        if let ProgressEvent::OutputLine { ref recipe, ref line, .. } = event {
            self.renderer.push_output(recipe, line);
        }

        // Push error output to cook-progress renderer
        if let ProgressEvent::NodeFailed { ref recipe, ref error, .. } = event {
            for err_line in error.lines() {
                self.renderer.push_output(recipe, err_line);
            }
            self.renderer.set_error(recipe);
        }

        self.update_state(&event);

        // Don't render until we have bars to show
        if is_queue {
            return;
        }

        // Interactive handoff — clear display and reset renderer
        if is_interactive_start {
            let _ = self.renderer.clear_last_frame(&mut std::io::stderr());
            self.renderer.reset();
            return;
        }

        // Build frame from current state and render
        let frame = self.build_frame();
        let _ = self.renderer.clear_last_frame(&mut std::io::stderr());
        let _ = self.renderer.render_frame(&frame, &mut std::io::stderr());

        // On finished, add a newline after the final frame
        if is_finished {
            let _ = writeln!(std::io::stderr());
        }
    }

    fn drain(&self) -> Vec<u8> {
        TtyRenderer::drain(self)
    }
}
```

- [ ] **Step 8: Remove dead code**

Remove these methods from TtyRenderer (no longer needed):
- `create_all_bars()`
- `ensure_bar()`
- `bar_str()`
- `refresh_recipe()`
- `refresh_footer()`
- `new_with_buffer()` (replace with `new_for_test()`)

- [ ] **Step 9: Verify it compiles**

Run: `cargo check`
Expected: compiles (may need to fix ColorConfig.is_enabled() or similar minor issues)

- [ ] **Step 10: Run tests**

Run: `cargo test`
Expected: PlainRenderer tests pass. TtyRenderer tests may need updates (Step 11).

- [ ] **Step 11: Commit**

```bash
git add src/scheduler/progress.rs src/scheduler/color.rs
git commit -m "feat: rewrite TtyRenderer to use cook-progress crate"
```

---

### Task 3: Update TtyRenderer tests

**Files:**
- Modify: `src/scheduler/progress.rs` (test section)

The state-tracking tests (`test_tty_renderer_state_tracking`, etc.) should mostly still pass since `update_state()` is largely unchanged. Update tests that reference removed fields or methods.

- [ ] **Step 1: Update test helper**

Replace `make_tty_renderer()`:
```rust
fn make_tty_renderer() -> TtyRenderer {
    use crate::scheduler::color::ColorMode;
    let color = ColorConfig::resolve(ColorMode::Never, false, false);
    TtyRenderer::new_for_test(color, 120)
}
```

- [ ] **Step 2: Remove/update tests for removed fields**

- Remove `test_tty_renderer_output_line_tracking` (cook-progress handles output buffering, `last_output_lines` field removed)
- Remove `test_tty_renderer_drain_returns_empty` (drain semantics unchanged but test references internal state)
- Update `test_tty_renderer_handle_updates_bars` — remove `recipe_bars` assertion, keep state assertions
- Update `test_tty_renderer_recipe_completion` — remove `last_output_lines` assertion

- [ ] **Step 3: Run all tests**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add src/scheduler/progress.rs
git commit -m "test: update TtyRenderer tests for cook-progress integration"
```

---

### Task 4: Update pipeline.rs renderer thread

**Files:**
- Modify: `src/engine/pipeline.rs`

The renderer thread currently calls `renderer.drain()` after each event and writes the result to stderr. With cook-progress, TtyRenderer writes directly to stderr during `handle()`, so `drain()` always returns empty. This is fine — no change needed for correctness. But we can simplify.

- [ ] **Step 1: Verify the renderer thread works as-is**

The existing code in `spawn_renderer_thread()` should work without changes since:
- `TtyRenderer::drain()` returns empty Vec (cook-progress writes directly to stderr)
- `PlainRenderer::drain()` returns buffered output (unchanged)

Run: `cargo test --test integration`
Expected: Integration tests pass

- [ ] **Step 2: Commit (if any changes needed)**

```bash
git add src/engine/pipeline.rs
git commit -m "chore: verify pipeline renderer thread works with cook-progress"
```

---

### Task 5: Final verification and cleanup

- [ ] **Step 1: Run all tests**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings (may need to clean up unused imports from indicatif removal)

- [ ] **Step 3: Manual verification**

Run cook on a real project to verify the progress output looks correct:

```bash
cargo run -- run <some-recipe>
```

Verify:
- Progress bars render correctly
- Completed sections collapse
- Failed sections expand with error output
- Footer shows correct counts
- No visual glitches at various terminal widths

- [ ] **Step 4: Commit any cleanup**

```bash
git add -A
git commit -m "chore: final cleanup after cook-progress integration"
```
