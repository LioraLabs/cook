# cook-progress Crate Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a standalone terminal progress rendering crate with composable primitives — progress bars, collapsible sections, streaming output, status footers — that Cook (and other tools) can use to render animated CLI progress displays.

**Architecture:** Two-layer model — stateless `Frame` descriptions (rebuilt each tick by the consumer) and a stateful `Renderer` (owns output buffers, tracks frame height for clearing). Consumer-driven rendering: the consumer calls `clear_last_frame()` then `render_frame()` on their own schedule. The crate writes to `&mut impl Write`.

**Tech Stack:** Rust, `crossterm` (ANSI styling + terminal width detection)

**Spec:** `docs/superpowers/specs/2026-03-17-cook-progress-crate-design.md`

---

## File Structure

### New files (all under `crates/cook-progress/`):
- `Cargo.toml` — crate manifest, `crossterm` dependency
- `src/lib.rs` — public re-exports
- `src/symbols.rs` — `Symbols` struct with default symbol set
- `src/frame.rs` — `Frame`, `Section`, `Footer`, `Status`, `ActiveItem`, `ItemStatus`, `CacheInfo`
- `src/output.rs` — `OutputBuffer`, `BufferState`
- `src/bar.rs` — fixed-width progress bar string rendering
- `src/renderer.rs` — `Renderer`, `RenderConfig`, `clear_last_frame`, `render_frame`
- `examples/basic.rs` — single section progressing and completing
- `examples/parallel.rs` — multiple concurrent sections with footer
- `examples/failure.rs` — section that fails with error expansion
- `examples/kitchen_sink.rs` — all states animated together

### Modified files:
- `Cargo.toml` (root) — add `[workspace]` section with `crates/cook-progress` member

---

## Chunk 1: Scaffolding & Core Types

### Task 1: Workspace and crate scaffolding

**Files:**
- Modify: `Cargo.toml` (root)
- Create: `crates/cook-progress/Cargo.toml`
- Create: `crates/cook-progress/src/lib.rs`

- [ ] **Step 1: Add workspace section to root Cargo.toml**

Add at the top of the root `Cargo.toml`, before `[package]`:

```toml
[workspace]
members = [".", "crates/cook-progress"]
```

- [ ] **Step 2: Create crate directory and Cargo.toml**

```bash
mkdir -p crates/cook-progress/src
```

Create `crates/cook-progress/Cargo.toml`:

```toml
[package]
name = "cook-progress"
version = "0.1.0"
edition = "2024"
description = "Terminal progress rendering with composable primitives"

[dependencies]
crossterm = "0.28"
```

- [ ] **Step 3: Create minimal lib.rs**

Create `crates/cook-progress/src/lib.rs`:

```rust
pub mod symbols;
pub mod frame;
pub mod output;
pub mod bar;
pub mod renderer;
```

- [ ] **Step 4: Create stub modules so it compiles**

Create empty files: `symbols.rs`, `frame.rs`, `output.rs`, `bar.rs`, `renderer.rs` in `crates/cook-progress/src/`.

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p cook-progress`
Expected: compiles with no errors

- [ ] **Step 6: Commit**

```bash
git add crates/cook-progress/ Cargo.toml Cargo.lock
git commit -m "feat: scaffold cook-progress workspace crate"
```

---

### Task 2: Symbols module

**Files:**
- Create: `crates/cook-progress/src/symbols.rs`

- [ ] **Step 1: Write failing tests for Symbols defaults**

In `crates/cook-progress/src/symbols.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_symbols() {
        let s = Symbols::default();
        assert_eq!(s.running, "◆");
        assert_eq!(s.completed, "✓");
        assert_eq!(s.failed, "✗");
        assert_eq!(s.cached, "≋");
        assert_eq!(s.waiting, "◇");
    }

    #[test]
    fn default_item_symbols() {
        let s = Symbols::default();
        assert_eq!(s.item_running, "◇");
        assert_eq!(s.item_completed, "✓");
        assert_eq!(s.item_failed, "✗");
        assert_eq!(s.item_cached, "≋");
        assert_eq!(s.item_skipped, "○");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-progress symbols`
Expected: FAIL — `Symbols` not defined

- [ ] **Step 3: Implement Symbols**

Add above the tests in `crates/cook-progress/src/symbols.rs`:

```rust
/// Customizable symbol set for progress rendering.
#[derive(Debug, Clone)]
pub struct Symbols {
    pub running: &'static str,
    pub completed: &'static str,
    pub failed: &'static str,
    pub cached: &'static str,
    pub waiting: &'static str,
    pub item_running: &'static str,
    pub item_completed: &'static str,
    pub item_failed: &'static str,
    pub item_cached: &'static str,
    pub item_skipped: &'static str,
}

impl Default for Symbols {
    fn default() -> Self {
        Self {
            running: "◆",
            completed: "✓",
            failed: "✗",
            cached: "≋",
            waiting: "◇",
            item_running: "◇",
            item_completed: "✓",
            item_failed: "✗",
            item_cached: "≋",
            item_skipped: "○",
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cook-progress symbols`
Expected: 2 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cook-progress/src/symbols.rs
git commit -m "feat(cook-progress): add Symbols with default symbol set"
```

---

### Task 3: Frame types

**Files:**
- Create: `crates/cook-progress/src/frame.rs`

- [ ] **Step 1: Write failing tests for frame types and builders**

In `crates/cook-progress/src/frame.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn section_builder_defaults() {
        let s = Section::new("build", "Build");
        assert_eq!(s.id, "build");
        assert_eq!(s.label, "Build");
        assert_eq!(s.status, Status::Waiting);
        assert_eq!(s.progress, None);
        assert_eq!(s.elapsed, None);
        assert!(s.active_items.is_empty());
        assert!(s.cache_info.is_none());
        assert!(s.children.is_empty());
    }

    #[test]
    fn section_builder_chaining() {
        let s = Section::new("lib", "lib")
            .status(Status::Running)
            .progress(3, 5)
            .elapsed(Duration::from_millis(800))
            .active_item("compile a.c", ItemStatus::Running)
            .active_item("compile b.c", ItemStatus::Running);

        assert_eq!(s.status, Status::Running);
        assert_eq!(s.progress, Some((3, 5)));
        assert_eq!(s.elapsed, Some(Duration::from_millis(800)));
        assert_eq!(s.active_items.len(), 2);
        assert_eq!(s.active_items[0].label, "compile a.c");
    }

    #[test]
    fn section_with_cache_info() {
        let s = Section::new("lib", "lib")
            .status(Status::Completed)
            .cache(3, 5);

        let info = s.cache_info.unwrap();
        assert_eq!(info.hits, 3);
        assert_eq!(info.total, 5);
    }

    #[test]
    fn frame_builder() {
        let frame = Frame::new()
            .section(Section::new("lib", "lib").status(Status::Running))
            .section(Section::new("test", "test").status(Status::Waiting))
            .footer("2 running · 1 waiting");

        assert_eq!(frame.sections.len(), 2);
        assert!(frame.footer.is_some());
        assert_eq!(frame.footer.unwrap().text, "2 running · 1 waiting");
    }

    #[test]
    fn frame_without_footer() {
        let frame = Frame::new()
            .section(Section::new("lib", "lib"));

        assert!(frame.footer.is_none());
    }

    #[test]
    fn section_with_children() {
        let s = Section::new("build", "Build")
            .child(Section::new("lib", "lib").status(Status::Running))
            .child(Section::new("bin", "bin").status(Status::Waiting));

        assert_eq!(s.children.len(), 2);
        assert_eq!(s.children[0].id, "lib");
        assert_eq!(s.children[1].id, "bin");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-progress frame`
Expected: FAIL — types not defined

- [ ] **Step 3: Implement frame types with builders**

Add above the tests in `crates/cook-progress/src/frame.rs`:

```rust
use std::time::Duration;

/// Status of a section, determines rendering behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Running,
    Completed,
    Failed,
    Cached,
}

/// Status of an individual active item within a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Running,
    Completed,
    Failed,
    Cached,
    Skipped,
}

/// An active item displayed within a running section.
#[derive(Debug, Clone)]
pub struct ActiveItem {
    pub label: String,
    pub status: ItemStatus,
}

/// Cache hit information for a section.
#[derive(Debug, Clone, Copy)]
pub struct CacheInfo {
    pub hits: usize,
    pub total: usize,
}

/// Consumer-formatted text pinned at the bottom of the frame.
#[derive(Debug, Clone)]
pub struct Footer {
    pub text: String,
}

/// A collapsible group — the main structural unit.
#[derive(Debug, Clone)]
pub struct Section {
    pub id: String,
    pub label: String,
    pub status: Status,
    pub progress: Option<(usize, usize)>,
    pub elapsed: Option<Duration>,
    pub active_items: Vec<ActiveItem>,
    pub cache_info: Option<CacheInfo>,
    pub children: Vec<Section>,
}

impl Section {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: Status::Waiting,
            progress: None,
            elapsed: None,
            active_items: Vec::new(),
            cache_info: None,
            children: Vec::new(),
        }
    }

    pub fn status(mut self, status: Status) -> Self {
        self.status = status;
        self
    }

    pub fn progress(mut self, completed: usize, total: usize) -> Self {
        self.progress = Some((completed, total));
        self
    }

    pub fn elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = Some(elapsed);
        self
    }

    pub fn active_item(mut self, label: impl Into<String>, status: ItemStatus) -> Self {
        self.active_items.push(ActiveItem {
            label: label.into(),
            status,
        });
        self
    }

    pub fn cache(mut self, hits: usize, total: usize) -> Self {
        self.cache_info = Some(CacheInfo { hits, total });
        self
    }

    pub fn child(mut self, child: Section) -> Self {
        self.children.push(child);
        self
    }
}

/// The complete frame to render in one tick.
#[derive(Debug, Clone)]
pub struct Frame {
    pub sections: Vec<Section>,
    pub footer: Option<Footer>,
}

impl Frame {
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
            footer: None,
        }
    }

    pub fn section(mut self, section: Section) -> Self {
        self.sections.push(section);
        self
    }

    pub fn footer(mut self, text: impl Into<String>) -> Self {
        self.footer = Some(Footer { text: text.into() });
        self
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cook-progress frame`
Expected: 6 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cook-progress/src/frame.rs
git commit -m "feat(cook-progress): add Frame, Section, and builder types"
```

---

### Task 4: Output buffer

**Files:**
- Create: `crates/cook-progress/src/output.rs`

- [ ] **Step 1: Write failing tests for OutputBuffer**

In `crates/cook-progress/src/output.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_returns_no_lines() {
        let buf = OutputBuffer::new();
        assert!(buf.visible_lines(3).is_empty());
        assert!(!buf.is_error());
    }

    #[test]
    fn buffer_grows_incrementally() {
        let mut buf = OutputBuffer::new();
        buf.push("line 1");
        assert_eq!(buf.visible_lines(3), vec!["line 1"]);

        buf.push("line 2");
        assert_eq!(buf.visible_lines(3), vec!["line 1", "line 2"]);

        buf.push("line 3");
        assert_eq!(buf.visible_lines(3), vec!["line 1", "line 2", "line 3"]);
    }

    #[test]
    fn buffer_rolls_at_max() {
        let mut buf = OutputBuffer::new();
        buf.push("line 1");
        buf.push("line 2");
        buf.push("line 3");
        buf.push("line 4");

        // max_lines = 3, so oldest drops off
        assert_eq!(buf.visible_lines(3), vec!["line 2", "line 3", "line 4"]);
    }

    #[test]
    fn error_shows_all_lines() {
        let mut buf = OutputBuffer::new();
        buf.push("line 1");
        buf.push("line 2");
        buf.push("line 3");
        buf.push("line 4");
        buf.set_error();

        // Even with max_lines = 2, error shows all
        assert_eq!(
            buf.visible_lines(2),
            vec!["line 1", "line 2", "line 3", "line 4"]
        );
        assert!(buf.is_error());
    }

    #[test]
    fn clear_resets_buffer() {
        let mut buf = OutputBuffer::new();
        buf.push("line 1");
        buf.set_error();
        buf.clear();

        assert!(buf.visible_lines(3).is_empty());
        assert!(!buf.is_error());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-progress output`
Expected: FAIL — `OutputBuffer` not defined

- [ ] **Step 3: Implement OutputBuffer**

Add above the tests in `crates/cook-progress/src/output.rs`:

```rust
/// State of an output buffer — normal (rolling window) or error (show all).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferState {
    Normal,
    Error,
}

/// Internal storage for captured output lines.
#[derive(Debug, Clone)]
pub struct OutputBuffer {
    lines: Vec<String>,
    state: BufferState,
}

impl OutputBuffer {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            state: BufferState::Normal,
        }
    }

    pub fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    /// Returns the lines that should be displayed.
    /// In Normal state: last `max_lines` lines.
    /// In Error state: all lines.
    pub fn visible_lines(&self, max_lines: usize) -> Vec<&str> {
        if self.state == BufferState::Error {
            self.lines.iter().map(|s| s.as_str()).collect()
        } else if self.lines.len() <= max_lines {
            self.lines.iter().map(|s| s.as_str()).collect()
        } else {
            self.lines[self.lines.len() - max_lines..]
                .iter()
                .map(|s| s.as_str())
                .collect()
        }
    }

    pub fn set_error(&mut self) {
        self.state = BufferState::Error;
    }

    pub fn is_error(&self) -> bool {
        self.state == BufferState::Error
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.state = BufferState::Normal;
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cook-progress output`
Expected: 5 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cook-progress/src/output.rs
git commit -m "feat(cook-progress): add OutputBuffer with rolling window and error expansion"
```

---

## Chunk 2: Bar Rendering & Renderer

### Task 5: Progress bar rendering

**Files:**
- Create: `crates/cook-progress/src/bar.rs`

The bar renderer produces a fixed-width string of filled (━) and empty (━ dimmed) segments. It does NOT include the label, counter, or elapsed — just the bar itself. The renderer composes these pieces.

- [ ] **Step 1: Write failing tests for bar rendering**

In `crates/cook-progress/src/bar.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_bar() {
        let bar = render_bar(20, 5, 5, false);
        // 20 filled segments, no empty
        assert_eq!(bar, "━━━━━━━━━━━━━━━━━━━━");
    }

    #[test]
    fn empty_bar() {
        let bar = render_bar(20, 0, 5, false);
        // 0 filled, 20 empty
        assert_eq!(bar, "━━━━━━━━━━━━━━━━━━━━");
    }

    #[test]
    fn half_bar() {
        let bar = render_bar(20, 3, 6, false);
        // 10 filled, 10 empty — without colors both are same char
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn zero_total_is_empty_bar() {
        let bar = render_bar(20, 0, 0, false);
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn bar_width_respected() {
        for width in [10, 20, 40, 80] {
            let bar = render_bar(width, 3, 10, false);
            assert_eq!(bar.chars().count(), width as usize);
        }
    }

    #[test]
    fn colored_bar_contains_ansi() {
        let bar = render_bar(20, 3, 6, true);
        // Colored output contains ANSI escape sequences
        assert!(bar.contains('\x1b'));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-progress bar`
Expected: FAIL — `render_bar` not defined

- [ ] **Step 3: Implement bar rendering**

Add above the tests in `crates/cook-progress/src/bar.rs`:

```rust
use crossterm::style::{Color, ResetColor, SetForegroundColor};
use std::fmt::Write as FmtWrite;

const BAR_CHAR: &str = "━";

/// Render a fixed-width progress bar string.
///
/// Returns a string of `width` bar characters (━). With colors enabled,
/// filled segments are green and empty segments are dimmed.
///
/// - `width`: total character width of the bar
/// - `completed`: number of completed items
/// - `total`: total number of items (0 treated as empty bar)
/// - `colors`: whether to apply ANSI color codes
pub fn render_bar(width: u16, completed: usize, total: usize, colors: bool) -> String {
    let width = width as usize;
    let filled = if total == 0 {
        0
    } else {
        (completed * width) / total
    };
    let empty = width - filled;

    if colors {
        let mut s = String::new();
        let _ = write!(s, "{}{}{}{}{}",
            SetForegroundColor(Color::Green),
            BAR_CHAR.repeat(filled),
            SetForegroundColor(Color::DarkGrey),
            BAR_CHAR.repeat(empty),
            ResetColor,
        );
        s
    } else {
        BAR_CHAR.repeat(width)
    }
}

/// Render a fully filled bar for completed/cached sections.
///
/// - `width`: total character width of the bar
/// - `color`: the color for the bar (green for success, red for failure)
/// - `colors`: whether to apply ANSI color codes
pub fn render_full_bar(width: u16, color: Color, colors: bool) -> String {
    let bar = BAR_CHAR.repeat(width as usize);
    if colors {
        format!("{}{bar}{}", SetForegroundColor(color), ResetColor)
    } else {
        bar
    }
}

/// Render a partial bar for failed sections.
///
/// Same as `render_bar` but filled segments use the specified color (typically red).
pub fn render_failed_bar(width: u16, completed: usize, total: usize, colors: bool) -> String {
    let width_usize = width as usize;
    let filled = if total == 0 {
        0
    } else {
        (completed * width_usize) / total
    };
    let empty = width_usize - filled;

    if colors {
        let mut s = String::new();
        let _ = write!(s, "{}{}{}{}{}",
            SetForegroundColor(Color::Red),
            BAR_CHAR.repeat(filled),
            SetForegroundColor(Color::DarkGrey),
            BAR_CHAR.repeat(empty),
            ResetColor,
        );
        s
    } else {
        BAR_CHAR.repeat(width_usize)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cook-progress bar`
Expected: 6 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cook-progress/src/bar.rs
git commit -m "feat(cook-progress): add fixed-width progress bar rendering"
```

---

### Task 6: Renderer — core structure and output management

**Files:**
- Create: `crates/cook-progress/src/renderer.rs`

- [ ] **Step 1: Write failing tests for Renderer construction and output management**

In `crates/cook-progress/src/renderer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RenderConfig {
        RenderConfig {
            width: 80,
            max_output_lines: 3,
            symbols: Symbols::default(),
            colors: false,
        }
    }

    #[test]
    fn new_renderer_has_no_state() {
        let r = Renderer::new(test_config());
        assert_eq!(r.last_frame_height(), 0);
    }

    #[test]
    fn push_output_creates_buffer() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "compiling foo.c");

        let lines = r.visible_lines("lib");
        assert_eq!(lines, vec!["compiling foo.c"]);
    }

    #[test]
    fn push_output_appends_to_existing_buffer() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "line 1");
        r.push_output("lib", "line 2");

        assert_eq!(r.visible_lines("lib"), vec!["line 1", "line 2"]);
    }

    #[test]
    fn set_error_shows_all_lines() {
        let mut r = Renderer::new(test_config());
        for i in 0..10 {
            r.push_output("lib", &format!("line {i}"));
        }
        r.set_error("lib");

        // All 10 lines visible despite max_output_lines = 3
        assert_eq!(r.visible_lines("lib").len(), 10);
    }

    #[test]
    fn set_width_updates_config() {
        let mut r = Renderer::new(test_config());
        r.set_width(120);
        // Width change is reflected internally — tested via render output in later tests
    }

    #[test]
    fn reset_clears_all_state() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "line 1");
        r.set_error("lib");
        r.reset();

        assert!(r.visible_lines("lib").is_empty());
        assert_eq!(r.last_frame_height(), 0);
    }

    #[test]
    fn missing_section_returns_empty_lines() {
        let r = Renderer::new(test_config());
        assert!(r.visible_lines("nonexistent").is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-progress renderer`
Expected: FAIL — `Renderer` not defined

- [ ] **Step 3: Implement Renderer core**

Add above the tests in `crates/cook-progress/src/renderer.rs`:

```rust
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::Duration;

use crate::bar;
use crate::frame::{Footer, Frame, ItemStatus, Section, Status};
use crate::output::OutputBuffer;
use crate::symbols::Symbols;

use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};

/// Configuration for the renderer.
#[derive(Debug, Clone)]
pub struct RenderConfig {
    pub width: u16,
    pub max_output_lines: usize,
    pub symbols: Symbols,
    pub colors: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            width: 80,
            max_output_lines: 3,
            symbols: Symbols::default(),
            colors: true,
        }
    }
}

/// Stateful renderer that owns output buffers and frame tracking.
pub struct Renderer {
    last_frame_height: usize,
    output_buffers: BTreeMap<String, OutputBuffer>,
    config: RenderConfig,
}

impl Renderer {
    pub fn new(config: RenderConfig) -> Self {
        Self {
            last_frame_height: 0,
            output_buffers: BTreeMap::new(),
            config,
        }
    }

    pub fn last_frame_height(&self) -> usize {
        self.last_frame_height
    }

    pub fn push_output(&mut self, section_id: &str, line: &str) {
        self.output_buffers
            .entry(section_id.to_string())
            .or_insert_with(OutputBuffer::new)
            .push(line);
    }

    pub fn set_error(&mut self, section_id: &str) {
        self.output_buffers
            .entry(section_id.to_string())
            .or_insert_with(OutputBuffer::new)
            .set_error();
    }

    pub fn set_width(&mut self, width: u16) {
        self.config.width = width;
    }

    /// Returns the visible output lines for a section.
    pub fn visible_lines(&self, section_id: &str) -> Vec<&str> {
        match self.output_buffers.get(section_id) {
            Some(buf) => buf.visible_lines(self.config.max_output_lines),
            None => Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.last_frame_height = 0;
        self.output_buffers.clear();
    }

    /// Erase the previous frame from the terminal.
    /// Emits cursor-up + line-erase for each line of the last frame.
    pub fn clear_last_frame(&self, w: &mut impl Write) -> io::Result<()> {
        for _ in 0..self.last_frame_height {
            // Move cursor up one line, then clear the entire line
            write!(w, "\x1b[1A\x1b[2K")?;
        }
        Ok(())
    }

    /// Render the current frame.
    /// The consumer must call `clear_last_frame()` before this.
    pub fn render_frame(&mut self, frame: &Frame, w: &mut impl Write) -> io::Result<()> {
        let mut line_count = 0;

        for section in &frame.sections {
            line_count += self.render_section(section, w, 0)?;
        }

        if let Some(footer) = &frame.footer {
            line_count += self.render_footer(footer, w)?;
        }

        self.last_frame_height = line_count;
        Ok(())
    }

    fn render_section(
        &self,
        section: &Section,
        w: &mut impl Write,
        _depth: usize,
    ) -> io::Result<usize> {
        let mut lines = 0;

        match section.status {
            Status::Waiting => {
                lines += self.render_waiting(section, w)?;
            }
            Status::Running => {
                lines += self.render_running(section, w)?;
            }
            Status::Completed => {
                lines += self.render_completed(section, w)?;
            }
            Status::Failed => {
                lines += self.render_failed(section, w)?;
            }
            Status::Cached => {
                lines += self.render_cached(section, w)?;
            }
        }

        for child in &section.children {
            lines += self.render_section(child, w, _depth + 1)?;
        }

        Ok(lines)
    }

    fn render_waiting(&self, section: &Section, w: &mut impl Write) -> io::Result<usize> {
        if self.config.colors {
            writeln!(
                w,
                "{}{} {}{}",
                SetForegroundColor(Color::DarkGrey),
                self.config.symbols.waiting,
                section.label,
                ResetColor,
            )?;
        } else {
            writeln!(w, "{} {}", self.config.symbols.waiting, section.label)?;
        }
        Ok(1)
    }

    fn render_running(&self, section: &Section, w: &mut impl Write) -> io::Result<usize> {
        let mut lines = 0;

        // Line 1: symbol label bar counter · elapsed
        let (completed, total) = section.progress.unwrap_or((0, 0));
        let bar_width = self.bar_width(&section.label, completed, total, &section.elapsed);
        let bar_str = bar::render_bar(bar_width, completed, total, self.config.colors);

        let counter = format!("{completed}/{total}");
        let elapsed_str = section
            .elapsed
            .map(|d| format!(" · {}", format_duration(d)))
            .unwrap_or_default();

        if self.config.colors {
            writeln!(
                w,
                "{}{}{} {}{}{} {bar_str} {counter}{elapsed_str}",
                SetForegroundColor(Color::Blue),
                self.config.symbols.running,
                ResetColor,
                SetAttribute(Attribute::Bold),
                section.label,
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            writeln!(
                w,
                "{} {} {bar_str} {counter}{elapsed_str}",
                self.config.symbols.running, section.label,
            )?;
        }
        lines += 1;

        // Line 2: active items (if any)
        if !section.active_items.is_empty() {
            if self.config.colors {
                write!(w, "  ")?;
                for (i, item) in section.active_items.iter().enumerate() {
                    if i > 0 {
                        write!(w, "  ")?;
                    }
                    let (sym, color) = self.item_symbol_and_color(&item.status);
                    write!(w, "{}{sym}{} {}", SetForegroundColor(color), ResetColor, item.label)?;
                }
                writeln!(w)?;
            } else {
                let items: Vec<String> = section
                    .active_items
                    .iter()
                    .map(|item| {
                        let sym = self.item_symbol(&item.status);
                        format!("{sym} {}", item.label)
                    })
                    .collect();
                writeln!(w, "  {}", items.join("  "))?;
            }
            lines += 1;
        }

        // Lines 3+: streaming output (last N lines, dimmed)
        let visible = self.visible_lines(&section.id);
        for line in &visible {
            if self.config.colors {
                writeln!(w, "    {}{line}{}", SetForegroundColor(Color::DarkGrey), ResetColor)?;
            } else {
                writeln!(w, "    {line}")?;
            }
            lines += 1;
        }

        Ok(lines)
    }

    fn render_completed(&self, section: &Section, w: &mut impl Write) -> io::Result<usize> {
        let (completed, total) = section.progress.unwrap_or((0, 0));
        let bar_width = self.bar_width(&section.label, completed, total, &section.elapsed);
        let bar_str = bar::render_full_bar(bar_width, Color::Green, self.config.colors);

        let counter = format!("{completed}/{total}");
        let elapsed_str = section
            .elapsed
            .map(|d| format!(" · {}", format_duration(d)))
            .unwrap_or_default();

        let cache_str = section
            .cache_info
            .map(|c| format!(" ({}/{} cached)", c.hits, c.total))
            .unwrap_or_default();

        if self.config.colors {
            writeln!(
                w,
                "{}{}{} {}{}{} {bar_str} {counter}{elapsed_str}{cache_str}",
                SetForegroundColor(Color::Green),
                self.config.symbols.completed,
                ResetColor,
                SetAttribute(Attribute::Bold),
                section.label,
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            writeln!(
                w,
                "{} {} {bar_str} {counter}{elapsed_str}{cache_str}",
                self.config.symbols.completed, section.label,
            )?;
        }
        Ok(1)
    }

    fn render_cached(&self, section: &Section, w: &mut impl Write) -> io::Result<usize> {
        let (completed, total) = section.progress.unwrap_or((0, 0));
        let bar_width = self.bar_width_cached(&section.label, completed, total);
        let bar_str = bar::render_full_bar(bar_width, Color::Green, self.config.colors);

        let counter = format!("{completed}/{total}");

        if self.config.colors {
            writeln!(
                w,
                "{}{}{} {}{}{} {bar_str} {counter} cached",
                SetForegroundColor(Color::Green),
                self.config.symbols.cached,
                ResetColor,
                SetAttribute(Attribute::Bold),
                section.label,
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            writeln!(
                w,
                "{} {} {bar_str} {counter} cached",
                self.config.symbols.cached, section.label,
            )?;
        }
        Ok(1)
    }

    fn render_failed(&self, section: &Section, w: &mut impl Write) -> io::Result<usize> {
        let mut lines = 0;

        let (completed, total) = section.progress.unwrap_or((0, 0));
        let bar_width = self.bar_width(&section.label, completed, total, &section.elapsed);
        let bar_str = bar::render_failed_bar(bar_width, completed, total, self.config.colors);

        let counter = format!("{completed}/{total}");
        let elapsed_str = section
            .elapsed
            .map(|d| format!(" · {}", format_duration(d)))
            .unwrap_or_default();

        if self.config.colors {
            writeln!(
                w,
                "{}{}{} {}{}{} {bar_str} {counter}{elapsed_str}",
                SetForegroundColor(Color::Red),
                self.config.symbols.failed,
                ResetColor,
                SetAttribute(Attribute::Bold),
                section.label,
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            writeln!(
                w,
                "{} {} {bar_str} {counter}{elapsed_str}",
                self.config.symbols.failed, section.label,
            )?;
        }
        lines += 1;

        // Error output with red left border
        let visible = self.visible_lines(&section.id);
        for line in &visible {
            if self.config.colors {
                writeln!(w, "  {}│{} {line}", SetForegroundColor(Color::Red), ResetColor)?;
            } else {
                writeln!(w, "  │ {line}")?;
            }
            lines += 1;
        }

        Ok(lines)
    }

    fn render_footer(&self, footer: &Footer, w: &mut impl Write) -> io::Result<usize> {
        let separator = "─".repeat(self.config.width as usize);
        if self.config.colors {
            writeln!(w, "{}{separator}{}", SetForegroundColor(Color::DarkGrey), ResetColor)?;
            writeln!(w, "{}{}{}", SetForegroundColor(Color::DarkGrey), footer.text, ResetColor)?;
        } else {
            writeln!(w, "{separator}")?;
            writeln!(w, "{}", footer.text)?;
        }
        Ok(2)
    }

    fn item_symbol(&self, status: &ItemStatus) -> &str {
        match status {
            ItemStatus::Running => self.config.symbols.item_running,
            ItemStatus::Completed => self.config.symbols.item_completed,
            ItemStatus::Failed => self.config.symbols.item_failed,
            ItemStatus::Cached => self.config.symbols.item_cached,
            ItemStatus::Skipped => self.config.symbols.item_skipped,
        }
    }

    fn item_symbol_and_color(&self, status: &ItemStatus) -> (&str, Color) {
        match status {
            ItemStatus::Running => (self.config.symbols.item_running, Color::DarkGrey),
            ItemStatus::Completed => (self.config.symbols.item_completed, Color::Green),
            ItemStatus::Failed => (self.config.symbols.item_failed, Color::Red),
            ItemStatus::Cached => (self.config.symbols.item_cached, Color::Green),
            ItemStatus::Skipped => (self.config.symbols.item_skipped, Color::DarkGrey),
        }
    }

    /// Calculate bar width for sections with counter and optional elapsed.
    /// Layout: "SYM LABEL BAR COUNTER · ELAPSED"
    /// The bar fills the remaining space.
    fn bar_width(
        &self,
        label: &str,
        completed: usize,
        total: usize,
        elapsed: &Option<Duration>,
    ) -> u16 {
        // "◆ " (2) + label + " " (1) + " " (1) + counter + elapsed
        let counter = format!("{completed}/{total}");
        let elapsed_len = elapsed
            .map(|d| format!(" · {}", format_duration(d)).len())
            .unwrap_or(0);
        let fixed = 2 + label.len() + 1 + 1 + counter.len() + elapsed_len;
        let remaining = (self.config.width as usize).saturating_sub(fixed);
        remaining.max(4) as u16
    }

    /// Calculate bar width for cached sections.
    fn bar_width_cached(&self, label: &str, completed: usize, total: usize) -> u16 {
        let counter = format!("{completed}/{total}");
        // "SYM " (2) + label + " " (1) + " " (1) + counter + " cached" (7)
        let fixed = 2 + label.len() + 1 + 1 + counter.len() + 7;
        let remaining = (self.config.width as usize).saturating_sub(fixed);
        remaining.max(4) as u16
    }
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let mins = secs as u64 / 60;
        let remaining = secs as u64 % 60;
        format!("{mins}m{remaining:02}s")
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cook-progress renderer`
Expected: 7 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cook-progress/src/renderer.rs
git commit -m "feat(cook-progress): add Renderer with output management and section rendering"
```

---

### Task 7: Renderer — frame rendering integration tests

**Files:**
- Modify: `crates/cook-progress/src/renderer.rs` (add tests)

These tests verify that `render_frame` produces the expected output for each status variant. Using `colors: false` for clean string assertions.

- [ ] **Step 1: Add render_frame integration tests**

Append to the `tests` module in `crates/cook-progress/src/renderer.rs` (add `use std::time::Duration;` and `use crate::frame::{Frame, Section, Status, ItemStatus};` to the test module imports if not already present):

```rust
    fn render_to_string(renderer: &mut Renderer, frame: &Frame) -> String {
        let mut buf = Vec::new();
        renderer.render_frame(frame, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn render_waiting_section() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new().section(Section::new("lib", "lib"));

        let output = render_to_string(&mut r, &frame);
        assert!(output.contains("◇ lib"));
        assert_eq!(r.last_frame_height(), 1);
    }

    #[test]
    fn render_running_section_with_bar() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "compiling foo.c");

        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Running)
                .progress(3, 5)
                .elapsed(Duration::from_millis(800))
                .active_item("compile a.c", ItemStatus::Running)
                .active_item("compile b.c", ItemStatus::Running),
        );

        let output = render_to_string(&mut r, &frame);
        let lines: Vec<&str> = output.lines().collect();

        // Line 1: symbol + label + bar + counter + elapsed
        assert!(lines[0].contains("◆ lib"));
        assert!(lines[0].contains("3/5"));
        assert!(lines[0].contains("0.8s"));
        // Line 2: active items
        assert!(lines[1].contains("compile a.c"));
        assert!(lines[1].contains("compile b.c"));
        // Line 3: streaming output
        assert!(lines[2].contains("compiling foo.c"));

        assert_eq!(r.last_frame_height(), 3);
    }

    #[test]
    fn render_completed_section() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Completed)
                .progress(5, 5)
                .elapsed(Duration::from_secs(1)),
        );

        let output = render_to_string(&mut r, &frame);
        assert!(output.contains("✓ lib"));
        assert!(output.contains("5/5"));
        assert_eq!(r.last_frame_height(), 1);
    }

    #[test]
    fn render_completed_with_cache_info() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Completed)
                .progress(5, 5)
                .elapsed(Duration::from_secs(1))
                .cache(3, 5),
        );

        let output = render_to_string(&mut r, &frame);
        assert!(output.contains("(3/5 cached)"));
    }

    #[test]
    fn render_cached_section() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Cached)
                .progress(5, 5),
        );

        let output = render_to_string(&mut r, &frame);
        assert!(output.contains("≋ lib"));
        assert!(output.contains("cached"));
        assert_eq!(r.last_frame_height(), 1);
    }

    #[test]
    fn render_failed_section_with_error_output() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "error[E0308]: mismatched types");
        r.push_output("lib", "  --> src/lib.rs:42:5");
        r.set_error("lib");

        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Failed)
                .progress(3, 5)
                .elapsed(Duration::from_millis(2100)),
        );

        let output = render_to_string(&mut r, &frame);
        let lines: Vec<&str> = output.lines().collect();

        assert!(lines[0].contains("✗ lib"));
        assert!(lines[0].contains("3/5"));
        // Error lines with red left border
        assert!(lines[1].contains("│"));
        assert!(lines[1].contains("error[E0308]"));
        assert!(lines[2].contains("│"));
        assert!(lines[2].contains("src/lib.rs:42:5"));

        assert_eq!(r.last_frame_height(), 3);
    }

    #[test]
    fn render_footer() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new()
            .section(Section::new("lib", "lib"))
            .footer("1 waiting");

        let output = render_to_string(&mut r, &frame);
        let lines: Vec<&str> = output.lines().collect();

        // Separator + footer text
        assert!(lines[1].contains("───"));
        assert!(lines[2].contains("1 waiting"));
        assert_eq!(r.last_frame_height(), 3);
    }

    #[test]
    fn clear_last_frame_emits_correct_escapes() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new()
            .section(Section::new("a", "a"))
            .section(Section::new("b", "b"));

        let mut buf = Vec::new();
        r.render_frame(&frame, &mut buf).unwrap();
        assert_eq!(r.last_frame_height(), 2);

        let mut clear_buf = Vec::new();
        r.clear_last_frame(&mut clear_buf).unwrap();
        let clear_str = String::from_utf8(clear_buf).unwrap();

        // Should have 2 cursor-up + erase sequences
        assert_eq!(clear_str.matches("\x1b[1A\x1b[2K").count(), 2);
    }

    #[test]
    fn render_composite_frame() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "compiling foo.c");

        let frame = Frame::new()
            .section(
                Section::new("deps", "deps")
                    .status(Status::Completed)
                    .progress(2, 2)
                    .elapsed(Duration::from_millis(300)),
            )
            .section(
                Section::new("lib", "lib")
                    .status(Status::Running)
                    .progress(3, 5)
                    .elapsed(Duration::from_millis(800))
                    .active_item("compile a.c", ItemStatus::Running),
            )
            .section(Section::new("test", "test"))
            .footer("1 done · 1 running · 1 waiting");

        let output = render_to_string(&mut r, &frame);
        let lines: Vec<&str> = output.lines().collect();

        // deps (completed) = 1 line
        // lib (running) = 1 bar + 1 active items + 1 output = 3 lines
        // test (waiting) = 1 line
        // footer = 2 lines (separator + text)
        assert_eq!(lines.len(), 7);
        assert_eq!(r.last_frame_height(), 7);
    }
```

- [ ] **Step 2: Run all tests**

Run: `cargo test -p cook-progress`
Expected: All tests PASS (previous unit tests + new integration tests)

- [ ] **Step 3: Commit**

```bash
git add crates/cook-progress/src/renderer.rs
git commit -m "test(cook-progress): add render_frame integration tests for all status variants"
```

---

### Task 8: Update lib.rs with public re-exports

**Files:**
- Modify: `crates/cook-progress/src/lib.rs`

- [ ] **Step 1: Add public re-exports for ergonomic imports**

Replace the contents of `crates/cook-progress/src/lib.rs`:

```rust
pub mod bar;
pub mod frame;
pub mod output;
pub mod renderer;
pub mod symbols;

pub use frame::{ActiveItem, CacheInfo, Footer, Frame, ItemStatus, Section, Status};
pub use renderer::{RenderConfig, Renderer};
pub use symbols::Symbols;
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p cook-progress`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add crates/cook-progress/src/lib.rs
git commit -m "feat(cook-progress): add public re-exports for ergonomic imports"
```

---

## Chunk 3: Animated Examples

### Task 9: Basic example

**Files:**
- Create: `crates/cook-progress/examples/basic.rs`

A single section that progresses through 5 nodes, showing streaming output, then completes.

- [ ] **Step 1: Write the basic example**

Create `crates/cook-progress/examples/basic.rs`:

```rust
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use cook_progress::{Frame, Renderer, RenderConfig, Section, Status, ItemStatus};

fn main() -> io::Result<()> {
    let mut stdout = io::stdout();
    let mut renderer = Renderer::new(RenderConfig {
        width: 80,
        max_output_lines: 3,
        colors: true,
        ..Default::default()
    });

    // Hide cursor
    write!(stdout, "\x1b[?25l")?;

    let start = Instant::now();
    let nodes = ["compile a.c", "compile b.c", "compile c.c", "compile d.c", "link lib"];
    let total = nodes.len();

    for (i, node) in nodes.iter().enumerate() {
        // Render running state
        for tick in 0..5 {
            renderer.clear_last_frame(&mut stdout)?;

            if tick == 2 {
                renderer.push_output("lib", &format!("  {node}..."));
            }

            let frame = Frame::new()
                .section(
                    Section::new("lib", "lib")
                        .status(Status::Running)
                        .progress(i, total)
                        .elapsed(start.elapsed())
                        .active_item(*node, ItemStatus::Running),
                );

            renderer.render_frame(&frame, &mut stdout)?;
            stdout.flush()?;
            thread::sleep(Duration::from_millis(80));
        }
    }

    // Final: completed
    renderer.clear_last_frame(&mut stdout)?;
    let frame = Frame::new()
        .section(
            Section::new("lib", "lib")
                .status(Status::Completed)
                .progress(total, total)
                .elapsed(start.elapsed()),
        );
    renderer.render_frame(&frame, &mut stdout)?;

    // Show cursor
    write!(stdout, "\x1b[?25h")?;
    stdout.flush()?;

    Ok(())
}
```

- [ ] **Step 2: Verify it compiles and runs**

Run: `cargo run -p cook-progress --example basic`
Expected: animated progress bar that fills up and collapses to a completed line

- [ ] **Step 3: Commit**

```bash
git add crates/cook-progress/examples/basic.rs
git commit -m "feat(cook-progress): add basic animated example"
```

---

### Task 10: Parallel example

**Files:**
- Create: `crates/cook-progress/examples/parallel.rs`

Multiple sections running concurrently with different completion times and a footer.

- [ ] **Step 1: Write the parallel example**

Create `crates/cook-progress/examples/parallel.rs`:

```rust
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use cook_progress::{Frame, Renderer, RenderConfig, Section, Status, ItemStatus};

fn main() -> io::Result<()> {
    let mut stdout = io::stdout();
    let mut renderer = Renderer::new(RenderConfig {
        width: 80,
        max_output_lines: 2,
        colors: true,
        ..Default::default()
    });

    write!(stdout, "\x1b[?25l")?;

    let start = Instant::now();
    let tick_ms: u64 = 100;
    let total_ticks: usize = 40;

    // deps finishes at tick 10, lib at tick 30, test runs from tick 10-40
    for tick in 0..total_ticks {
        renderer.clear_last_frame(&mut stdout)?;

        let elapsed = start.elapsed();

        // Simulate output
        if tick == 5 {
            renderer.push_output("deps", "fetching dep-a...");
        }
        if tick == 8 {
            renderer.push_output("deps", "fetching dep-b...");
        }
        if tick == 15 {
            renderer.push_output("lib", "compiling src/foo.c");
        }
        if tick == 20 {
            renderer.push_output("lib", "compiling src/bar.c");
        }
        if tick == 25 {
            renderer.push_output("lib", "linking libcook.a");
        }
        if tick == 32 {
            renderer.push_output("test", "running test_add...");
        }
        if tick == 35 {
            renderer.push_output("test", "running test_multiply...");
        }

        let deps = if tick < 10 {
            Section::new("deps", "deps")
                .status(Status::Running)
                .progress(tick.min(2), 2)
                .elapsed(elapsed)
                .active_item("fetch dep-a", ItemStatus::Running)
        } else {
            Section::new("deps", "deps")
                .status(Status::Completed)
                .progress(2, 2)
                .elapsed(Duration::from_millis(tick_ms * 10u64))
        };

        let lib = if tick < 10 {
            Section::new("lib", "lib").status(Status::Waiting)
        } else if tick < 30 {
            let completed = ((tick - 10) * 5) / 20;
            Section::new("lib", "lib")
                .status(Status::Running)
                .progress(completed, 5)
                .elapsed(Duration::from_millis(tick_ms * (tick - 10) as u64)
)
                .active_item("compile src/main.c", ItemStatus::Running)
        } else {
            Section::new("lib", "lib")
                .status(Status::Completed)
                .progress(5, 5)
                .elapsed(Duration::from_millis(tick_ms * 20))
                .cache(2, 5)
        };

        let test = if tick < 10 {
            Section::new("test", "test").status(Status::Waiting)
        } else if tick < total_ticks - 2 {
            let completed = ((tick - 10) * 8) / 28;
            Section::new("test", "test")
                .status(Status::Running)
                .progress(completed, 8)
                .elapsed(Duration::from_millis(tick_ms * (tick - 10) as u64)
)
                .active_item("test_suite", ItemStatus::Running)
        } else {
            Section::new("test", "test")
                .status(Status::Completed)
                .progress(8, 8)
                .elapsed(Duration::from_millis(tick_ms * 28))
        };

        let running = [&deps, &lib, &test]
            .iter()
            .filter(|s| s.status == Status::Running)
            .count();
        let done = [&deps, &lib, &test]
            .iter()
            .filter(|s| s.status == Status::Completed)
            .count();
        let waiting = 3 - running - done;

        let frame = Frame::new()
            .section(deps)
            .section(lib)
            .section(test)
            .footer(format!("{done} done · {running} running · {waiting} waiting"));

        renderer.render_frame(&frame, &mut stdout)?;
        stdout.flush()?;
        thread::sleep(Duration::from_millis(tick_ms));
    }

    write!(stdout, "\x1b[?25h")?;
    stdout.flush()?;

    Ok(())
}
```

- [ ] **Step 2: Verify it runs**

Run: `cargo run -p cook-progress --example parallel`
Expected: three sections progressing at different rates, footer updating

- [ ] **Step 3: Commit**

```bash
git add crates/cook-progress/examples/parallel.rs
git commit -m "feat(cook-progress): add parallel sections example"
```

---

### Task 11: Failure example

**Files:**
- Create: `crates/cook-progress/examples/failure.rs`

A section that progresses partway then fails, showing error expansion.

- [ ] **Step 1: Write the failure example**

Create `crates/cook-progress/examples/failure.rs`:

```rust
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use cook_progress::{Frame, Renderer, RenderConfig, Section, Status, ItemStatus};

fn main() -> io::Result<()> {
    let mut stdout = io::stdout();
    let mut renderer = Renderer::new(RenderConfig {
        width: 80,
        max_output_lines: 3,
        colors: true,
        ..Default::default()
    });

    write!(stdout, "\x1b[?25l")?;

    let start = Instant::now();
    let tick_ms = 120;

    // Progress through some nodes successfully
    for tick in 0..15 {
        renderer.clear_last_frame(&mut stdout)?;

        let completed = tick.min(3);

        if tick == 3 {
            renderer.push_output("lib", "compiling src/foo.c");
        }
        if tick == 6 {
            renderer.push_output("lib", "compiling src/bar.c");
        }
        if tick == 9 {
            renderer.push_output("lib", "compiling src/baz.c");
        }

        let frame = Frame::new()
            .section(
                Section::new("lib", "lib")
                    .status(Status::Running)
                    .progress(completed, 5)
                    .elapsed(start.elapsed())
                    .active_item("compile src/main.c", ItemStatus::Running),
            )
            .footer("1 running");

        renderer.render_frame(&frame, &mut stdout)?;
        stdout.flush()?;
        thread::sleep(Duration::from_millis(tick_ms));
    }

    // Failure! Push error output and expand
    renderer.push_output("lib", "error[E0308]: mismatched types");
    renderer.push_output("lib", "  --> src/main.c:42:5");
    renderer.push_output("lib", "   |");
    renderer.push_output("lib", "42 |     let x: i32 = \"hello\";");
    renderer.push_output("lib", "   |                  ^^^^^^^ expected `i32`, found `&str`");
    renderer.set_error("lib");

    renderer.clear_last_frame(&mut stdout)?;
    let frame = Frame::new()
        .section(
            Section::new("lib", "lib")
                .status(Status::Failed)
                .progress(3, 5)
                .elapsed(start.elapsed()),
        )
        .footer("1 failed");

    renderer.render_frame(&frame, &mut stdout)?;

    write!(stdout, "\x1b[?25h")?;
    stdout.flush()?;

    Ok(())
}
```

- [ ] **Step 2: Verify it runs**

Run: `cargo run -p cook-progress --example failure`
Expected: progress bar runs, then fails and expands to show full error with red border

- [ ] **Step 3: Commit**

```bash
git add crates/cook-progress/examples/failure.rs
git commit -m "feat(cook-progress): add failure example with error expansion"
```

---

### Task 12: Kitchen sink example

**Files:**
- Create: `crates/cook-progress/examples/kitchen_sink.rs`

All states animated together — waiting, running, completed, cached, failed.

- [ ] **Step 1: Write the kitchen sink example**

Create `crates/cook-progress/examples/kitchen_sink.rs`:

```rust
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use cook_progress::{Frame, Renderer, RenderConfig, Section, Status, ItemStatus};

fn main() -> io::Result<()> {
    let mut stdout = io::stdout();
    let mut renderer = Renderer::new(RenderConfig {
        width: 80,
        max_output_lines: 3,
        colors: true,
        ..Default::default()
    });

    write!(stdout, "\x1b[?25l")?;

    let start = Instant::now();
    let tick_ms: u64 = 100;
    let total_ticks: usize = 50;

    for tick in 0..total_ticks {
        renderer.clear_last_frame(&mut stdout)?;

        // Simulate output at various points
        if tick == 3 { renderer.push_output("deps", "resolving dep-a v1.2.3"); }
        if tick == 5 { renderer.push_output("deps", "resolving dep-b v0.8.1"); }
        if tick == 12 { renderer.push_output("lib", "compiling src/parser.c"); }
        if tick == 16 { renderer.push_output("lib", "compiling src/lexer.c"); }
        if tick == 20 { renderer.push_output("lib", "compiling src/codegen.c"); }
        if tick == 24 { renderer.push_output("lib", "linking libcook.a"); }
        if tick == 30 { renderer.push_output("test", "running test_parser..."); }
        if tick == 33 { renderer.push_output("test", "running test_lexer..."); }
        if tick == 36 {
            renderer.push_output("test", "FAILED: test_codegen");
            renderer.push_output("test", "  assertion failed: expected 42, got 0");
            renderer.push_output("test", "  at tests/codegen.rs:15:9");
            renderer.set_error("test");
        }

        // deps: runs ticks 0-8, cached result
        let deps = if tick < 8 {
            let completed = (tick * 4) / 8;
            Section::new("deps", "deps")
                .status(Status::Running)
                .progress(completed, 4)
                .elapsed(start.elapsed())
                .active_item("resolve", ItemStatus::Running)
        } else {
            Section::new("deps", "deps")
                .status(Status::Cached)
                .progress(4, 4)
        };

        // lib: waits until tick 8, runs 8-28, completes
        let lib = if tick < 8 {
            Section::new("lib", "lib").status(Status::Waiting)
        } else if tick < 28 {
            let completed = ((tick - 8) * 6) / 20;
            let mut s = Section::new("lib", "lib")
                .status(Status::Running)
                .progress(completed, 6)
                .elapsed(Duration::from_millis(tick_ms * (tick - 8) as u64));
            if tick < 20 {
                s = s
                    .active_item("compile parser.c", ItemStatus::Completed)
                    .active_item("compile lexer.c", ItemStatus::Running)
                    .active_item("compile codegen.c", ItemStatus::Running)
                    .active_item("compile cache.c", ItemStatus::Cached)
                    .active_item("compile old.c", ItemStatus::Skipped);
            } else {
                s = s.active_item("link libcook.a", ItemStatus::Running);
            }
            s
        } else {
            Section::new("lib", "lib")
                .status(Status::Completed)
                .progress(6, 6)
                .elapsed(Duration::from_millis(tick_ms * 20))
                .cache(2, 6)
        };

        // test: waits until tick 28, runs 28-36, fails
        let test = if tick < 28 {
            Section::new("test", "test").status(Status::Waiting)
        } else if tick < 36 {
            let completed = ((tick - 28) * 4) / 8;
            let mut s = Section::new("test", "test")
                .status(Status::Running)
                .progress(completed, 4)
                .elapsed(Duration::from_millis(tick_ms * (tick - 28) as u64));
            if tick >= 34 {
                // Show a failed item alongside running ones
                s = s
                    .active_item("test_parser", ItemStatus::Completed)
                    .active_item("test_codegen", ItemStatus::Failed)
                    .active_item("test_linker", ItemStatus::Running);
            } else {
                s = s.active_item("test_codegen", ItemStatus::Running);
            }
            s
        } else {
            Section::new("test", "test")
                .status(Status::Failed)
                .progress(2, 4)
                .elapsed(Duration::from_millis(tick_ms * 8))
        };

        // bench: always waiting (never starts)
        let bench = Section::new("bench", "bench").status(Status::Waiting);

        let statuses = [&deps, &lib, &test, &bench];
        let running = statuses.iter().filter(|s| s.status == Status::Running).count();
        let done = statuses.iter().filter(|s| matches!(s.status, Status::Completed | Status::Cached)).count();
        let failed = statuses.iter().filter(|s| s.status == Status::Failed).count();
        let waiting = statuses.iter().filter(|s| s.status == Status::Waiting).count();

        let mut footer_parts = Vec::new();
        if done > 0 { footer_parts.push(format!("{done} done")); }
        if running > 0 { footer_parts.push(format!("{running} running")); }
        if failed > 0 { footer_parts.push(format!("{failed} failed")); }
        if waiting > 0 { footer_parts.push(format!("{waiting} waiting")); }

        let frame = Frame::new()
            .section(deps)
            .section(lib)
            .section(test)
            .section(bench)
            .footer(footer_parts.join(" · "));

        renderer.render_frame(&frame, &mut stdout)?;
        stdout.flush()?;
        thread::sleep(Duration::from_millis(tick_ms));
    }

    write!(stdout, "\x1b[?25h")?;
    stdout.flush()?;

    Ok(())
}
```

- [ ] **Step 2: Verify it runs**

Run: `cargo run -p cook-progress --example kitchen_sink`
Expected: all statuses animated — cached, completed, running, failed, waiting — with footer

- [ ] **Step 3: Commit**

```bash
git add crates/cook-progress/examples/kitchen_sink.rs
git commit -m "feat(cook-progress): add kitchen sink example with all status variants"
```

---

### Task 13: Final verification

- [ ] **Step 1: Run all tests**

Run: `cargo test -p cook-progress`
Expected: All tests PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p cook-progress -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Verify all examples compile**

Run: `cargo build -p cook-progress --examples`
Expected: All examples compile

- [ ] **Step 4: Commit any cleanup**

If any fixes were needed from clippy, commit them:

```bash
git add -A crates/cook-progress/
git commit -m "chore(cook-progress): clippy fixes and final polish"
```
