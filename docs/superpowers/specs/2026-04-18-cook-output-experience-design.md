# Cook Output Experience — Design

Date: 2026-04-18

## Problem

Today's cook output experience is poor. Concretely:

1. **Recipe ordering is meaningless.** Recipes appear in the order `RecipeQueued` / `RecipeStarted` events arrive, which does not correspond to DAG dependencies, declaration order, or anything a user can predict.
2. **Output collapses.** Only 3 tail lines are visible per recipe, and recipes collapse down when they transition state — useful context disappears.
3. **Narrow terminals produce runaway new frames.** The clear-last-frame redraw accounts for logical lines but the terminal soft-wraps physical rows when a line exceeds `cols`. The cursor math falls out of sync and a fresh frame prints below stale output on every event.
4. **No drill-down.** A user watching a slow or noisy node has no way to focus on it or see its full log live.

Supporting symptoms (not explicitly called out but present):

- `cook-progress` conflates pure state and rendering in a single `Frame`/`Section` builder, making it hard to test or extend.
- Output buffering lives in `Renderer` as `BTreeMap<String, OutputBuffer>`, keyed by recipe — no way to scope to a node.
- `TtyRenderer` and `PlainRenderer` duplicate state-tracking logic.
- After a build ends there is no way to inspect what happened.

The `cook-progress` crate will be rewritten from scratch, with a new internal architecture that supports three renderers (inline, tui, plain) sharing one pure state machine, plus a persistent per-build log directory that enables post-hoc inspection via a new `cook recap` command.

## Goals

- Inline (scroll-buffer) output that is **stable, deterministic, and wrap-safe** on any terminal width.
- DAG-aware recipe ordering, frozen at build start — nothing jumps.
- Every recipe keeps **3 output lines visible at all times** (running, waiting, done alike). No collapsing.
- A rich opt-in **TUI** (`--ui` or press `u` mid-build) for drill-in on big DAGs.
- Plain / CI output that is grep-friendly and append-only.
- Machine-readable `--output=json` for tooling.
- Every build persists its full event log and per-node output to `.cook/logs/<build-id>/`.
- `cook recap` replays any past build into the TUI using the same widgets as the live build.
- Pure-model core, exhaustive snapshot tests, no flaky terminal dependencies in tests.

## Non-goals

- Remote recap (other machines / URLs).
- Web viewer of `events.jsonl` (possible later; format is stable).
- OTLP / metrics exporter.
- Split-pane multi-log view in TUI.
- Configurable color themes beyond on/off and `NO_COLOR`.

## Architecture

```
cook-engine ──emits──▶ ProgressEvent stream
                              │
                              ▼
                     ┌────────────────────┐
                     │  cook-progress::core  │   pure state machine
                     │   BuildState          │   · topo-ordered recipes
                     │   RecipeState         │   · per-recipe output ring
                     │   NodeState           │   · derived counters
                     └────────────────────┘
                         │         │
        ┌────────────────┘         └────────────────┐
        ▼                                           ▼
┌───────────────────┐                       ┌───────────────────┐
│ inline renderer   │                       │ tui renderer      │
│ (default, stream) │                       │ (--ui / u key)    │
│ crossterm only    │                       │ ratatui + crossterm│
└───────────────────┘                       └───────────────────┘
        │                                           │
        ▼                                           ▼
     stderr                                     alt screen
  (scroll buffer)                              (fullscreen)

                ┌───────────────────┐
                │ plain renderer    │   when stderr is not a TTY
                │ (pipes / CI)      │   · stable line format
                └───────────────────┘   · optional --output=json
```

The three renderers never mutate state — they read from a shared `Arc<Mutex<BuildState>>`. Event consumption updates the state once; each renderer reads the latest snapshot and renders. This makes the inline↔TUI hot-swap possible: both modes observe the same model, so a mid-build swap loses nothing.

### Crate layout

```
cook-progress/
├── src/
│   ├── lib.rs         facade — ProgressEvent, Renderer trait, spawn()
│   ├── event.rs       ProgressEvent (public API, no engine types)
│   ├── model/         pure state — no I/O
│   │   ├── mod.rs
│   │   ├── build.rs   BuildState (topo order, counters, elapsed)
│   │   ├── recipe.rs  RecipeState (status, progress, output ring)
│   │   └── node.rs    NodeState (active nodes, per-node output)
│   ├── render/
│   │   ├── mod.rs     Renderer trait, auto-select
│   │   ├── inline.rs  scroll-buffer renderer (stable slots)
│   │   ├── tui.rs     ratatui alt-screen renderer
│   │   └── plain.rs   pipe/CI renderer + optional JSON lines
│   ├── layout.rs      width-aware line builder (hard-truncate)
│   ├── style.rs       color + symbol config
│   ├── log_store.rs   .cook/logs/<build-id>/ writer & reader
│   └── driver.rs      event loop, mode selection, TTY detection
└── examples/          kitchen-sink, narrow-terminal, failure, huge-dag
```

## Event model

```rust
pub enum ProgressEvent {
    // Topology (sent once, up front)
    BuildStarted {
        recipes: Vec<RecipeTopo>,    // topo-sorted, with deps
        total_nodes: usize,
    },

    // Recipe lifecycle
    RecipeStarted   { recipe: RecipeId },
    RecipeCompleted { recipe: RecipeId, elapsed: Duration,
                      cached: usize, total: usize },
    RecipeFailed    { recipe: RecipeId, elapsed: Duration,
                      completed: usize, total: usize },

    // Node lifecycle (the real unit of work)
    NodeStarted    { recipe: RecipeId, node: NodeId, label: String },
    NodeCompleted  { recipe: RecipeId, node: NodeId, elapsed: Duration },
    NodeFailed     { recipe: RecipeId, node: NodeId, elapsed: Duration,
                     error: String },
    NodeCacheHit   { recipe: RecipeId, node: NodeId },
    NodeSkipped    { recipe: RecipeId, node: NodeId, reason: SkipReason },

    // Streaming output (node-scoped, not recipe-scoped)
    NodeOutput     { recipe: RecipeId, node: NodeId,
                     line: String, stream: Stream },  // Stdout | Stderr

    // Interactive command takeover
    InteractiveStart { recipe: RecipeId, node: NodeId },
    InteractiveEnd   { recipe: RecipeId, node: NodeId,
                       elapsed: Duration, success: bool },

    // Terminal
    Finished { success: bool },
}

pub struct RecipeTopo {
    pub id: RecipeId,
    pub name: String,
    pub deps: Vec<RecipeId>,      // upstream recipe ids
    pub expected_nodes: usize,    // for initial progress bar sizing
}

pub enum SkipReason {
    UpstreamFailed,
    ConditionFalse,
    Disabled,
}

pub enum Stream { Stdout, Stderr }
```

Changes from today's `ProgressEvent`:

- **Added** `BuildStarted` with full topology: renderers know every recipe and its deps before frame 1, so slots are stable.
- **Added** `NodeOutput` — output is scoped to the node that produced it. Replaces `OutputLine { recipe, line }`.
- **Added** `SkipReason` — user-visible reason for each skip.
- **Added** typed `RecipeId` / `NodeId` — cheap, avoids name collisions.
- **Removed** `RecipeQueued` — folded into `BuildStarted`.
- **Removed** `TestResult` — test subsystem emits its own event type separately.

### Pure state

```rust
pub struct BuildState {
    pub order: Vec<RecipeId>,               // topo order, frozen at start
    pub recipes: BTreeMap<RecipeId, RecipeState>,
    pub started_at: Instant,
    pub totals: Counters,                   // done / running / waiting / cached
}

pub struct RecipeState {
    pub id: RecipeId,
    pub name: String,
    pub deps: Vec<RecipeId>,
    pub status: Status,           // Waiting | Running | Completed | Failed | Cached
    pub progress: (usize, usize), // (done, total)
    pub elapsed: Option<Duration>,
    pub active_nodes: Vec<NodeState>,
    pub output_tail: OutputRing,  // fixed-size ring (N=3 default, configurable)
    pub error_log: Vec<String>,   // kept in full for post-build dump
    pub cache_hits: usize,
    pub skipped: Vec<(NodeId, SkipReason)>,
}

pub struct OutputRing {
    capacity: usize,
    lines: VecDeque<String>,
    overflowed: bool,
}
```

Invariants:

- `BuildState::apply(&mut self, event: ProgressEvent)` is pure — the only mutation path.
- `order` is set once from `BuildStarted::recipes` via a topological sort; never reordered.
- Tiebreaker within a topo level is **declaration order from the Cookfile**, not alphabetical or event order.
- Rendering any frame is a pure function of `BuildState` plus a `RenderOptions { width, tail, color, symbols }`.

### Per-node full log

The inline renderer only reads the last 3 lines via `OutputRing`. The TUI needs full per-node history on demand. Solution: `cook-progress` writes every `NodeOutput` line to `.cook/logs/<build-id>/nodes/<recipe>/<node>.log` as it arrives, and the TUI reads this file when a node is drilled into.

```
.cook/logs/
├── 2026-04-18-a3b/
│   ├── events.jsonl            full ProgressEvent stream (append-only)
│   ├── manifest.toml           start/end time, command, exit code, schema version
│   └── nodes/
│       ├── deps/fetch-libcurl.log
│       ├── lib/renderer.cpp.log
│       └── ...
├── 2026-04-18-9f2/
└── 2026-04-17-001/
```

Rotation: keep the N most recent builds (default 20, configurable). Per-node log files are truncated at `max_bytes_per_node` (default 2 MiB) with a `--- truncated ---` marker. Total disk usage is bounded.

## Inline renderer

The default experience. Lives in the scroll buffer. Uses `crossterm` only.

### Row anatomy (fixed 4 lines per recipe)

```
line 1  header    : sym label  bar  counter · elapsed  ← deps
line 2  output[−3]: last-minus-2 output line (or · dot)
line 3  output[−2]: last-minus-1 output line (or · dot)
line 4  output[−1]: most recent output line (or · dot)
```

Every recipe is exactly 4 rows, regardless of state. Waiting recipes render the header followed by three padding dots. Running recipes show the last 3 lines of live output. Done / Failed / Cached recipes freeze the last 3 lines. Total frame height is deterministic: `4 × recipes + 3 (footer)`.

### Ordering

- Topological sort of the recipe DAG, frozen at `BuildStarted`.
- Tiebreaker within a topo level: order of appearance across the loaded Cookfiles (engine-defined "source order" — documented at the point of emission in cook-engine; `cook-progress` receives this as the already-ordered `recipes` list in `BuildStarted` and does not re-sort).
- Waves are gone from the renderer. There are no wave dividers, no wave concept in `cook-progress`.
- Each row shows its upstream deps with a `← deps` arrow (up to 2 direct upstreams, plus `+N` if more).

### Wide terminal example (cols ≥ 80)

```
$ cook build app

✓ deps      ━━━━━━━━━━━━━━━━━━━━━━━━━ 12/12 · 0.4s (10/12 cached)
    cached: libprotobuf@3.25, abseil@20240116
    resolved libcurl@8.5.0 (network, 210ms)
    wrote deps.lock
◆ lib       ━━━━━━━━━━━━━━━━━━━━━━━━━ 14/26 · 1.8s  ← deps
    ▸ compile renderer.cpp  ▸ compile mesh.cpp
    renderer.cpp:42:9: warning: unused variable 'foo'
    [g++] -O2 -c renderer.cpp -o build/renderer.o
◆ test      ━                         1/24 · 0.6s  ← deps
    ▸ test_mesh_intersect
    running test_alloc_fuzz ... ok
    running test_mesh_intersect ...
◇ bench     waiting                              ← lib, test
    ·
    ·
    ·
──────────────────────────────────────────────────
2 running · 1 done · 1 waiting · 10/62 cached · 2.4s
press 'u' for live UI · ctrl-c to cancel
```

### Narrow terminal (cols=60)

```
✓ deps      ━━━━━━━━━ 12/12 · 0.4s (10/12 cached)…
    cached: libprotobuf@3.25, abseil@202401…
    resolved libcurl@8.5.0 (network, 210ms)…
    wrote deps.lock
◆ lib       ━━━━━━━━━ 14/26 · 1.8s  ← deps
    ▸ compile renderer.cpp  ▸ compile mesh…
    renderer.cpp:42:9: warning: unused var…
    [g++] -O2 -c renderer.cpp -o build/rend…
──────────────────────────────────────────
2 running · 1 done · 1 waiting · 10/62 c…
```

### The narrow-terminal fix

Today's renderer issues `writeln!` then counts logical lines for `clear_last_frame`. If any line exceeds `cols`, the terminal soft-wraps it into two physical rows, but the count is still 1. The next frame's clear walks up N logical rows while the cursor is N+k physical rows down — old content stays, and a fresh frame prints below.

Fix:

1. A single layout pass before any terminal write builds a `Vec<String>` of rendered lines.
2. Every line is pre-truncated to `cols - 1` **visible columns** (ANSI escapes excluded from width) with a trailing `…`.
3. `cols - 1` (not `cols`) leaves a guard column — a line that happens to be exactly `cols` chars wide can still trigger a soft-wrap in some terminals.
4. Unicode width via `unicode-width` crate so CJK / emoji count correctly.
5. Total frame height is the length of that `Vec`. `clear_last_frame(n)` is always correct.
6. On `SIGWINCH` / resize: re-read `cols`, re-layout, redraw. No partial frames.

### Failure output (same fencing used in both inline end-of-run and TUI)

```
✗ lib       ━━━━━━━━                 8/26 · 2.1s  ← deps
    ...

─── lib · compile renderer.cpp · failed after 1.8s ─────────────
│  renderer.cpp:42:9: error: 'bar' was not declared in this scope
│      int foo = bar(x);
│                ^~~
│  compilation terminated.
│
│  cause        : exit status 1
│  command      : g++ -O2 -c renderer.cpp -o build/renderer.o
│  full log     : .cook/logs/2026-04-18-a3b/lib/renderer.cpp.log
│  replay       : cook logs lib:renderer.cpp
────────────────────────────────────────────────────────────────

skipped 18 downstream nodes: bench/*, app/link
```

On failure the live inline frame is left in scrollback as-is — a user can scroll up and see the state at time of failure. The error block is appended below. Cascaded skips are summarized, not repeated per node.

## TUI renderer

Opt-in. Alt-screen. Built on ratatui (de facto Rust TUI library, layers on crossterm, actively maintained). Same `BuildState` as inline.

### Layout

- **Top bar:** command line + live totals (elapsed · done/total · cache hits).
- **Left pane (fixed 28–32 cols):** tree of recipes → active/recent nodes. Expandable / collapsible with space. Running node auto-highlighted.
- **Right pane:** live log of the selected node. Auto-follows tail when cursor is at bottom (`f` toggles). Scrollable with PgUp/PgDn and mouse wheel.
- **Status bar:** context-aware keybinding hints.

```
┌─ cook build app ─────────────────────────────────── 2.4s · 14/62 · 10 cached ─┐
├──────────────────────────┬─────────────────────────────────────────────────┤
│ Recipes                  │ lib · compile renderer.cpp                    3.1s │
│ ✓ deps       12/12  0.4s │ ─────────────────────────────────────────────── │
│ ◆ lib        14/26  1.8s │  [g++] -O2 -I./include -c renderer.cpp           │
│    ✓ alloc.cpp           │  renderer.cpp: In function 'render()':           │
│    ✓ mesh.cpp            │  renderer.cpp:42:9: warning: unused variable     │
│    ◆ renderer.cpp ▸      │    'foo' [-Wunused-variable]                     │
│    ◇ scene.cpp           │      int foo = bar(x);                           │
│    ◇ link app            │          ^~~                                     │
│ ◆ test        1/24  0.6s │  renderer.cpp: In function 'draw()':             │
│    ◆ test_mesh_intersect │  ...                                             │
│    ◇ test_scene_render   │                                                  │
│ ◇ bench                  │                                                  │
├──────────────────────────┴──────────────────────────────────────────────────┤
│ j/k nav · ↵ drill · space collapse · f follow · / filter · l logs · q quit │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Keybindings

```
j / k / ↓ ↑       move selection in tree
h / l / ← →       collapse / expand, or jump between panes
↵                 drill into selected node (right pane focuses log)
space             collapse / expand recipe subtree
f                 toggle follow-tail on log
g / G             jump to top / bottom of log
/ <pattern>       filter tree (fuzzy match on recipe/node name)
n / N             next / prev failed node
L                 cycle log level (all | stderr | errors-only)
?                 show help overlay
q / Esc           quit TUI (build continues; inline mode resumes)
Ctrl-C            cancel build
```

### Mouse

- Click a tree row → select + drill into log (if leaf node).
- Wheel in left pane → scroll tree. Wheel in right pane → scroll log, disables follow.
- Click and drag the tree/log divider → resize.

### Entering & exiting

- `cook build --ui` — starts directly in TUI mode.
- `u` during inline mode — driver reads stdin non-blocking; `u` triggers a seamless swap. Sequence:
  1. Acquire the BuildState mutex.
  2. `crossterm::terminal::enable_raw_mode()` (already enabled for inline, reconfirm).
  3. `crossterm::execute!(EnterAlternateScreen, Hide)`.
  4. Spawn TUI loop; releases the mutex between frames.
  5. Inline event handler pauses rendering but keeps updating state (it still owns the event consumer).
- `q` in TUI — inverse. Pop alternate screen, reprint a fresh inline frame, build continues streaming inline.
- Build finishes while in TUI — final summary shown in TUI, `q` exits, inline summary printed to scrollback.

This swap is the subtlest piece of the design. The single `Arc<Mutex<BuildState>>` is the serialization point: only one renderer actively draws at a time, state updates keep going regardless.

## Plain renderer

Automatic when `stderr` is not a TTY. Also on `--output=plain`, `--no-ui`, `CI=true`, `TERM=dumb`.

```
$ cook build app 2>&1 | tee build.log
cook build app
  deps/resolve         cached  0.00s
  deps/fetch libcurl                                       0.21s
  deps/lock                                                0.01s
  deps                 done    (12/12, 10 cached)          0.40s
  lib/alloc.cpp                                            0.32s
  lib/mesh.cpp                                             0.41s
  [lib/renderer.cpp] renderer.cpp:42:9: warning: unused variable 'foo'
  [lib/renderer.cpp] [g++] -O2 -c renderer.cpp -o build/renderer.o
  lib/renderer.cpp                                         0.88s
  ...
  test                 done    (24/24)                     1.9s
cook build app done in 4.1s (62 nodes, 10 cached)
```

- Line format: `  <recipe>/<node>` indented two spaces, right-padded label column, duration right-aligned. Status words (`cached`, `done`, `FAILED`) in a status column.
- Per-node output lines interleaved with `[recipe/node]` prefix so grep stays useful.
- No ANSI, no carriage returns, no live redraws. Pure append-only.
- Ordering: **event order** (as things complete), not topological. Chronology is the useful axis here.

### `--output=json`

Opt-in. One JSON object per line; one ProgressEvent → one line.

```
$ cook build app --output=json
{"ts":"2026-04-18T09:12:04.201Z","v":1,"type":"build-started","recipes":[...],"total_nodes":62}
{"ts":"2026-04-18T09:12:04.214Z","v":1,"type":"recipe-started","recipe":"deps"}
{"ts":"2026-04-18T09:12:04.510Z","v":1,"type":"node-cache-hit","recipe":"deps","node":"protobuf"}
{"ts":"2026-04-18T09:12:04.612Z","v":1,"type":"node-output","recipe":"deps","node":"fetch-libcurl","stream":"stdout","line":"resolving libcurl@8.5.0..."}
...
{"ts":"2026-04-18T09:12:08.301Z","v":1,"type":"finished","success":true,"elapsed_ms":4100}
```

Schema is versioned via a `"v"` field on every event. Same format is written to `.cook/logs/<build-id>/events.jsonl` on every build (always on, not just when `--output=json`).

## End-of-run summary

### Success (inline)

```
✓ build succeeded  · 62 nodes · 10 cached (16%) · 4.1s
  deps  0.4s   lib  2.1s   test  1.9s   bench  1.2s
```

### Failure (inline)

The live frame is left as-is. Error block appended. Cascaded skips summarized.

```
✗ build failed  · 1 failure · 14/62 nodes ran · 2.4s

─── lib · compile renderer.cpp · failed after 1.8s ─────────────
│  renderer.cpp:42:9: error: 'bar' was not declared in this scope
│      int foo = bar(x);
│                ^~~
│  compilation terminated.
│
│  cause        : exit status 1
│  command      : g++ -O2 -c renderer.cpp -o build/renderer.o
│  full log     : .cook/logs/2026-04-18-a3b/lib/renderer.cpp.log
│  replay       : cook logs lib:renderer.cpp
────────────────────────────────────────────────────────────────

skipped 18 downstream nodes: bench/*, app/link
```

## `cook logs` and `cook recap`

Two new subcommands, both reading `.cook/logs/<build-id>/`.

### `cook logs`

Text-only. Pulls full per-node output to stdout.

```
cook logs                          # list recent builds
cook logs lib:renderer.cpp         # full output of that node from last build
cook logs lib:renderer.cpp --build 2026-04-18-a3b
cook logs --failed                 # dump all failed nodes from last build
```

### `cook recap`

Opens the **TUI** against a past build. Same widgets, same keybindings, fixed state.

```
cook recap                     # TUI on last build
cook recap --list              # picker in TUI
cook recap 2026-04-18-a3b      # specific build
cook recap --last-failed       # jump to last failed build
cook recap --replay            # replay events.jsonl at original speed
```

Inside the recap TUI, `b` / `B` jump between neighboring builds without exiting. `n` / `N` jump between failed nodes in the current build. `r` toggles replay mode, which animates `BuildState` through the event log at original timestamps (space pauses, `→` / `←` step).

Recap is implemented by constructing a synthetic event consumer that reads `events.jsonl` instead of the live `mpsc::Receiver`. Everything downstream of that reuses the live TUI code path.

## Interactive command takeover

Some nodes are marked interactive (gdb, Node inspector, interactive prompts). The renderer must hand off the terminal cleanly and take it back on exit.

```
InteractiveStart { recipe, node }
  inline: clear live frame · leave raw mode · print divider · flush stdin
  tui:    leave alternate screen · print divider
InteractiveEnd { recipe, node, elapsed, success }
  inline: print divider · re-enter raw mode · redraw live frame
  tui:    enter alternate screen · repaint model
```

Interactive output lives in the scrollback forever, never in the live frame.

## Mode selection

| Context                    | Default renderer              |
| -------------------------- | ----------------------------- |
| `stderr` is a TTY          | inline                        |
| `stderr` is not a TTY      | plain                         |
| `--ui` flag                | tui (error if not TTY)        |
| `--no-ui` / `--output=plain` | plain                       |
| `--output=json`            | json (overrides all)          |
| `CI=true` (no explicit flag) | plain                       |
| `NO_COLOR=1`               | unchanged renderer, colors stripped |
| `TERM=dumb`                | plain                         |
| `cook recap`               | tui (replay / recap mode)     |

`NO_COLOR` is an overlay modifier — it does not change which renderer runs, only suppresses ANSI color codes. All other rows are renderer selection rules.

The `u` hot-swap key is only active when `stdin` is a TTY (not redirected / piped). When stdin isn't a TTY, the driver skips reading keystrokes entirely, so the same binary works identically under `cook build < some-pipe`.

### CLI surface

```
cook build                    # auto-detect (inline if TTY, plain otherwise)
cook build --ui               # force TUI
cook build --no-ui            # force inline
cook build --output=plain
cook build --output=json
cook build --tail=5           # output tail size (default 3)
cook build --quiet            # recipe status only, no per-node output
cook build --verbose          # expand tail to 10, show stream labels

cook logs                     # list builds
cook logs lib:renderer.cpp    # full log for one node (text)
cook logs --failed            # dump all failed nodes from last build

cook recap                    # TUI on last build
cook recap --list
cook recap <build-id>
cook recap --last-failed
cook recap --replay
```

## Configuration

`~/.cook/config.toml`:

```toml
[ui]
default = "inline"          # inline | tui | plain | auto
tail_lines = 3
color = "auto"              # auto | always | never
symbols = "unicode"         # unicode | ascii

[ui.tui]
follow_tail = true
tree_width = 30             # columns for left pane
vim_bindings = true

[ui.logs]
keep_builds = 20            # rotate .cook/logs/ to this many entries
max_bytes_per_node = 2_097_152     # 2 MiB/node log file
max_total_bytes = 524_288_000      # 500 MiB total across .cook/logs/
events_jsonl = true         # write events.jsonl per build (set false to opt out of recap)
```

## Dependencies added

```toml
[dependencies]
crossterm = "0.28"
ratatui   = "0.29"
serde     = { version = "1", features = ["derive"] }
serde_json = "1"
unicode-width = "0.2"

[dev-dependencies]
insta = "1"
vt100 = "0.15"
```

## Testing strategy

- **Model tests:** apply canned event streams to `BuildState`; snapshot the derived state with `insta`. No terminal needed.
- **Inline renderer tests:** render a `BuildState` to `Vec<u8>` at fixed widths `{40, 60, 80, 120, 200}`; ANSI-strip; snapshot. Covers the narrow-terminal fix.
- **TUI tests:** use ratatui's `TestBackend` to render to a fake cell grid; snapshot. Deterministic, no real terminal.
- **Plain / JSON tests:** byte-exact snapshots per scenario.
- **Integration tests:** drive a real cook build under a `vt100` parser; assert cursor position never drifts and no stale lines persist after N ticks (regression guard for the narrow-terminal bug). Include a resize-mid-build case.
- **Recap tests:** write a canned `events.jsonl`, open it with the recap code path, snapshot the final TUI grid. Reuses TUI fixtures.

## Migration (sequenced; each step builds + tests pass)

1. **Scaffolding.** New module layout in `cook-progress` (`model/`, `render/`, `event.rs`, `layout.rs`, `style.rs`, `log_store.rs`, `driver.rs`) added alongside the existing files. Add `ratatui`, `serde`, `serde_json`, `unicode-width`, `insta`, `vt100` to `Cargo.toml`. Old `Renderer` keeps working.
2. **Event API + pure model.** Move `ProgressEvent` from `cook-cli/src/progress.rs` into `cook-progress::event`. Add `BuildStarted`, `NodeOutput`, `SkipReason`, typed ids. Implement `BuildState`, `RecipeState`, `NodeState`, `OutputRing`. Snapshot tests on the state machine.
3. **Inline renderer.** Implement `render::inline::Renderer` against `BuildState`. Width-aware `layout.rs` with `unicode-width`. Snapshot tests at widths `{40, 60, 80, 120, 200}`. `vt100` integration test for cursor drift.
4. **Plain renderer.** `render::plain` chronological text; `render::plain::json` for JSON lines. Snapshot tests.
5. **Log persistence.** `.cook/logs/<build-id>/` writer. `nodes/<recipe>/<node>.log` rotation. `events.jsonl` always-on. `manifest.toml`.
6. **TUI renderer.** `render::tui` with ratatui (tree + log panes). Vi bindings, mouse. `TestBackend` snapshot tests. `u` hot-swap from inline → tui sharing `Arc<Mutex<BuildState>>`.
7. **`cook logs` subcommand.** cook-cli: list builds, dump node log. Roundtrip tests.
8. **`cook recap` subcommand.** Replays `events.jsonl` into `BuildState`; reuses TUI renderer. `--list`, `--last-failed`, `--replay` modes.
9. **Wire cook-engine.** Engine emits `BuildStarted` up front (requires topo computed before execution starts — confirm against `docs/architecture/execution-flow.md`). Emit `NodeOutput` instead of `OutputLine`. Surface `SkipReason`.
10. **Delete old code.** Remove `cook-progress::{bar, frame, output, renderer, symbols}`. Remove `cook-cli::progress::{TtyRenderer, PlainRenderer, RecipeRenderState}`. Update examples and `docs/architecture/supporting-modules.md`.

## Risk register

**Low risk:**

- Pure-model separation is well-trodden; `insta` snapshots are boring and reliable.
- `ratatui` + `crossterm` is already cook's terminal stack; no new backend.
- Plain / JSON are pure append-only writers; hard to regress visually.

**Watch carefully:**

- **Inline ↔ TUI hot swap** is the subtlest piece. Raw-mode enter/leave ordering, stdin handoff, events arriving mid-swap. Mitigation: all rendering goes through one mutex-guarded `BuildState`; a swap is a single serialized transition with no event loss.
- **Narrow terminal fix** must cover resize mid-build, not just startup cols. Add a `SIGWINCH` regression test.
- **Log rotation** bugs can fill disk. Bound total bytes across `.cook/logs/`, not only per-node. Enforce on every build start.
- **Engine emitting topology up front** requires cook-engine's topo computation to run before any recipe starts. Today's engine may compute it lazily; check `execution-flow.md` and plan the adjustment in step 9.

## Out of scope for v1

- Remote recap (SSH / URL).
- Web viewer of `events.jsonl`.
- OTLP / metrics exporter.
- Split-pane multi-log view in TUI.
- Configurable color themes beyond on/off + `NO_COLOR`.
