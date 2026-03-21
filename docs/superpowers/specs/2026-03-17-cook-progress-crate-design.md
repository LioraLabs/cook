# cook-progress Crate Design

A general-purpose terminal progress rendering crate. Provides composable primitives — progress bars, collapsible sections, streaming output, status footers — that consumers assemble into frames and render on their own schedule.

Built for Cook's CLI but designed with no Cook-specific language or concepts.

## Architecture

### Two-Layer Model

**Frame description (stateless):** The consumer builds a `Frame` each tick describing what to draw — sections with progress bars, active items, and a footer. Rebuilt from scratch each render cycle; cheap structs, not strings.

**Renderer (stateful):** Owns output buffers (keyed by section ID) and tracks the last frame height for clearing. The consumer pushes output lines into the renderer, then calls `render_frame()` with a `Frame` to draw.

```
Consumer                          cook-progress
────────                          ─────────────
push_output("lib", "compiling")──▶ OutputBuffer storage
                                        │
build Frame { sections, footer } ──▶ render_frame()
                                        │
                              clear previous frame (cursor up + erase)
                              render each section based on Status
                              render footer
                              track new frame height
                                        │
                                  ▼ bytes to &mut impl Write
```

### Consumer-Driven Rendering

The consumer owns the render loop — threading, tick rate, timing. The crate provides:

- `renderer.push_output(section_id, line)` — buffer output lines
- `renderer.set_error(section_id)` — mark a section's buffer for full expansion
- `renderer.set_width(width)` — update terminal width (e.g., on resize)
- `renderer.clear_last_frame(w)` — erase the previous frame from the terminal
- `renderer.render_frame(frame, w)` — draw the current frame (does NOT clear first — consumer must call `clear_last_frame()` before each `render_frame()`)
- `renderer.reset()` — clear frame height tracking and all output buffers (for terminal handoff)

The crate writes to `&mut impl Write`. The consumer handles cursor hiding/showing, signal handlers, terminal lifecycle, and color/NO_COLOR resolution (passing the result as `RenderConfig.colors`).

## Primitives

### Frame

The complete description of what to render in one tick.

```rust
struct Frame {
    sections: Vec<Section>,
    footer: Option<Footer>,
}
```

### Section

The main structural unit. A collapsible group with progress, active items, and associated output.

```rust
struct Section {
    id: String,                        // unique key for output buffer lookup
    label: String,                     // display name
    status: Status,                    // determines rendering behavior
    progress: Option<(usize, usize)>,  // (completed, total)
    elapsed: Option<Duration>,
    active_items: Vec<ActiveItem>,     // shown below bar when Running
    cache_info: Option<CacheInfo>,     // partial cache hits (e.g., "3/5 cached")
    children: Vec<Section>,            // nested sections (optional)
}

struct ActiveItem {
    label: String,
    status: ItemStatus,
}

enum ItemStatus {
    Running,
    Completed,
    Failed,
    Cached,
    Skipped,
}

enum Status {
    Waiting,
    Running,
    Completed,
    Failed,
    Cached,
}

struct CacheInfo {
    hits: usize,
    total: usize,
}
```

### Footer

Consumer-formatted text pinned at the bottom of the frame.

```rust
struct Footer {
    text: String,
}
```

### Renderer

Stateful renderer that owns output buffers and frame tracking.

```rust
struct Renderer {
    last_frame_height: usize,
    output_buffers: BTreeMap<String, OutputBuffer>,
    config: RenderConfig,
}

struct RenderConfig {
    width: u16,              // terminal width
    max_output_lines: usize, // visible lines while Running (default: 3)
    symbols: Symbols,        // customizable symbol set
    colors: bool,            // enable ANSI colors
}
```

### OutputBuffer

Internal storage for captured output lines.

```rust
struct OutputBuffer {
    lines: Vec<String>,
    state: BufferState,  // Normal or Error
}
```

When `state` is `Normal`, only the last `max_output_lines` are displayed. When `state` is `Error`, all buffered lines are displayed.

Output lines grow the visible area incrementally: 1 line when 1 line exists, 2 when 2 exist, up to `max_output_lines`, then it becomes a rolling window showing the most recent N lines.

## Rendering Rules

### Status::Running
```
◆ label ━━━━━━━━━━━━━━━━━━━━ 3/5 · 0.8s
  ◇ active_item_a  ◇ active_item_b  ✓ active_item_c
    output line 1 (dimmed)
    output line 2 (dimmed)
```
- Filled symbol (◆), bold label
- Progress bar: green filled segments, dim empty segments
- Counter: completed/total
- Elapsed time
- Active items on next line (if any), indented, each prefixed with its `ItemStatus` symbol
- Last N output lines below, dimmed, indented

### Status::Completed
```
✓ label ━━━━━━━━━━━━━━━━━━━━ 5/5 · 1.2s
✓ label ━━━━━━━━━━━━━━━━━━━━ 5/5 · 1.2s (3/5 cached)
```
- Check symbol (✓), green
- Full bar, green
- Single line — collapsed
- If `cache_info` is set, appends "(hits/total cached)" suffix

### Status::Cached
```
≋ label ━━━━━━━━━━━━━━━━━━━━ 5/5 cached
```
- Wave symbol (≋), green
- Full bar, green
- "cached" instead of elapsed time
- Single line — collapsed

### Status::Failed
```
✗ label ━━━━━━━━━━ 3/5 · 2.1s
  │ error[E0308]: mismatched types
  │   --> src/lib.rs:42:5
  │    |
  │ 42 |     foo(bar)
  │    |     ^^^^^^^ expected &str, found String
```
- X symbol (✗), red
- Partial bar, red filled segments
- Full output buffer displayed (not just last N)
- Red left border on output lines

### Status::Waiting
```
◇ label
```
- Hollow symbol (◇), dimmed
- Label dimmed
- No bar, no progress — single line

### Footer
```
─────────────────────────────────────
2 running · 3 done · 1 waiting
```
- Separator line above
- Consumer-provided text, dimmed

## Frame Clearing

The renderer tracks `last_frame_height` after each render. On the next `clear_last_frame()` call, it emits `last_frame_height` cursor-up + line-erase ANSI sequences to wipe the previous frame before drawing the new one.

The consumer must call `clear_last_frame()` before each `render_frame()`. They are separate calls so the consumer can perform other terminal operations between clearing and rendering (e.g., printing non-managed output).

## Interactive Handoff

When the consumer needs to hand the terminal to an interactive subprocess:

1. `renderer.clear_last_frame(w)` — erase the progress display
2. `renderer.reset()` — clear frame height tracking and output buffers
3. Consumer runs the interactive command (full terminal access)
4. Consumer resumes calling `render_frame()` — starts fresh, no stale state

## Terminal Resize

The consumer can call `renderer.set_width(new_width)` at any time between renders. The next `render_frame()` will use the updated width. The consumer is responsible for detecting resize events (e.g., via `crossterm::terminal::size()`).

## Crate Structure

```
cook-progress/
  Cargo.toml
  src/
    lib.rs          — pub exports
    frame.rs        — Frame, Section, Footer, Status
    renderer.rs     — Renderer, RenderConfig, clear/render logic
    output.rs       — OutputBuffer, BufferState
    symbols.rs      — Symbols struct, defaults
    bar.rs          — progress bar string rendering (fixed-width fill)
  examples/
    basic.rs        — single section completing with output
    parallel.rs     — multiple concurrent sections, footer
    failure.rs      — section fails, error expansion
    kitchen_sink.rs — all states animated together
```

### Dependencies

- `crossterm` — terminal width detection, ANSI styling (colors, bold, dim)
- No other dependencies. No indicatif.

## Testing Strategy

### Unit tests per module
- `bar.rs` — bar strings at various widths and fill percentages
- `output.rs` — buffer growth, rolling window, error expansion
- `symbols.rs` — default symbol set
- `frame.rs` — builder ergonomics, frame composition

### Renderer integration tests
- Render frames to `Vec<u8>`, assert on output strings
- Each status variant produces expected lines
- `clear_last_frame()` emits correct cursor-up + erase count
- Multi-section composite frames match expected output
- `colors: false` mode for clean assertion strings without escape codes

### Animated examples
- `cargo run --example basic` — watch a single section progress and complete
- `cargo run --example parallel` — watch multiple sections run concurrently
- `cargo run --example failure` — watch a section fail and expand
- `cargo run --example kitchen_sink` — all states animating together

Examples use `thread::sleep` to simulate real timing for visual verification.

## Scope Boundaries

**In scope:**
- TTY rendering with ANSI escape codes
- Progress bar rendering with configurable width
- Section collapse/expand based on status
- Output buffering with rolling window and error expansion
- Frame clearing via cursor control
- Customizable symbols and colors
- Animated examples for visual verification

**Out of scope:**
- Non-TTY / plain text rendering (consumer's responsibility)
- Render loop / threading (consumer-driven)
- Cursor hide/show lifecycle (consumer's responsibility)
- Signal handling / panic cleanup (consumer's responsibility)
- Color mode resolution (`--color` flag, `NO_COLOR` env var) — consumer resolves and passes `RenderConfig.colors`
- Test summary rendering — typically printed after progress UI teardown, consumer formats and prints directly
- Cook-specific concepts (recipes, nodes, cache) — consumer maps these to generic primitives

## Symbol Customization

The default symbol set is:

| Status | Symbol |
|--------|--------|
| Running | ◆ |
| Completed | ✓ |
| Failed | ✗ |
| Cached | ≋ |
| Waiting | ◇ |
| Active item (Running) | ◇ |
| Active item (Completed) | ✓ |
| Active item (Failed) | ✗ |
| Active item (Cached) | ≋ |
| Active item (Skipped) | ○ |

All symbols are customizable via `RenderConfig.symbols`. The consumer can override any or all symbols to match their design language.
