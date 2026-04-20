# Cook Output Experience — Indicatif Rewrite

Date: 2026-04-20

## Context

This design supersedes the inline-renderer and layout portions of `2026-04-18-cook-output-experience-design.md`. The TUI, `cook recap`, and `u` hot-swap pieces of the earlier design are deferred to a separate brainstorming session and are not implemented here.

The motivation is twofold:

1. The current output looks unprofessional in ways that spec changes alone will fix (`·` padding dots under done/waiting recipes, full-width green bars on completed recipes, always-on `press 'u' for live UI` hint on a 0.5s build, stub bars on zero-node recipes, truncated compile-command tails).
2. The hand-rolled terminal renderer (`cook-progress::{bar, frame, output, renderer, symbols}`) carries its own narrow-terminal / cursor-drift / wrap-math bugs that are cheaper to replace with a maintained crate than to fix.

`indicatif` handles stacked multi-bar terminal UIs with redraw / wrap / resize correctness as its whole purpose. Adopting it lets us delete the hand-rolled renderer and focus cook-progress on what it is actually good at: the pure state machine, the event API, the log store, and the plain / JSON writers.

## Problem

Today's inline renderer produces frames like:

```
✓ liblua  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ 33/33 · 0.3s
    $ clang -c -MMD -MF .cook/deps/liblua/lvm.d -DLUA_USE_LINUX -Wall -O2 lua-5.4.7/src/lvm.c -o build/obj/liblua/lv…
    $ clang -c -MMD -MF .cook/deps/liblua/lzio.d -DLUA_USE_LINUX -Wall -O2 lua-5.4.7/src/lzio.c -o build/obj/liblua/…
    $ ar rcs build/lib/libliblua.a build/obj/liblua/lapi.o build/obj/liblua/lauxlib.o build/obj/liblua/lbaselib.o bu…
✓ lua  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ 2/2 · 0.0s  ← liblua
    ·
    $ clang -c -MMD -MF .cook/deps/lua/lua.d -DLUA_USE_LINUX -Wall -O2 lua-5.4.7/src/lua.c -o build/obj/lua/lua.o
    $ clang++ build/obj/lua/lua.o build/lib/libliblua.a -lreadline -lm -ldl -o build/bin/lua
✓ build  ━━━━━━━━━━ 0/0 · 0.0s  ← lua, luac
    ·
    ·
    ·
────────────────────────────────────────────────────────────
4 done · 0.5s
press 'u' for live UI · ctrl-c to cancel
```

Issues:

1. **`·` padding dots** under done/waiting recipes from the "always 4 rows per recipe" rule.
2. **Full-width bars on completed recipes** are pure visual noise.
3. **Stub bar on `build 0/0`** — the bar-for-everything rule breaks for zero-node recipes.
4. **Truncated compile command tails** under already-done recipes. The last-3-lines-of-output rule surfaces half-useful noise after work has finished.
5. **`press 'u' for live UI` on a sub-second build** is pointless churn; worse, there is no live UI yet.
6. **Narrow-terminal wrap / cursor drift** bugs in the hand-rolled renderer.

## Goals

- Inline output that is stable, deterministic, and wrap-safe on any terminal width, powered by indicatif's `MultiProgress`.
- DAG-aware recipe ordering, frozen at build start.
- Done / waiting / cached recipes collapse to a single line. Running recipes expand to exactly two lines: a progress header plus an artifact status strip.
- Artifact status strip shows what is actively being produced (basename pills with status symbols), not raw compile commands.
- Plain / CI output that is grep-friendly and append-only.
- `--output=json` emits one event per line for tooling.
- Every build persists its full event log and per-node output to `.cook/logs/<build-id>/`.
- `cook logs` subcommand for post-hoc inspection of any past build's per-node output.
- Pure-model core, exhaustive snapshot tests, no flaky terminal dependencies in tests.

## Non-goals (this project)

- TUI renderer (deferred to a dedicated brainstorming session).
- `cook recap` subcommand (depends on TUI).
- `u` hot-swap key between inline and TUI (depends on TUI).
- Remote recap, web viewer of `events.jsonl`, OTLP exporter.
- Split-pane multi-log view.
- Configurable color themes beyond on / off / `NO_COLOR`.

## Architecture

```
cook-engine ──emits──▶ ProgressEvent stream
                              │
                              ▼
                     ┌────────────────────┐
                     │  cook-progress::model │   pure state machine
                     │   BuildState          │   · topo-ordered recipes
                     │   RecipeState         │   · per-node artifact paths
                     │   NodeState           │   · cached_count collapsed
                     └────────────────────┘
                         │         │         │
        ┌────────────────┤         │         └────────────────┐
        ▼                ▼         ▼                          ▼
┌───────────────────┐ ┌──────────────┐              ┌───────────────────┐
│ inline renderer   │ │ log store    │              │ plain renderer    │
│ (TTY default)     │ │ events.jsonl │              │ (non-TTY default) │
│ indicatif Multi   │ │ nodes/*.log  │              │ append-only text  │
│ Progress          │ │ manifest.toml│              │ · JSON variant    │
└───────────────────┘ └──────────────┘              └───────────────────┘
        │                                                      │
        ▼                                                      ▼
     stderr                                                 stderr
  (live bars)                                           (grep-friendly)
```

Renderers never mutate state. The driver consumes events, updates `BuildState`, records to the log store, then calls `renderer.handle(&state, &event)`. Rendering is a pure function of `BuildState` plus a `RenderOptions` (width, color, symbols).

### Crate layout

```
cook-progress/
├── src/
│   ├── lib.rs         facade — ProgressEvent, Renderer trait, spawn()
│   ├── event.rs       ProgressEvent (public API, no engine types)
│   ├── model/         pure state — no I/O
│   │   ├── mod.rs
│   │   ├── build.rs   BuildState (topo order, counters, elapsed)
│   │   ├── recipe.rs  RecipeState (status, progress, nodes map)
│   │   └── node.rs    NodeState (artifact, status, timestamps)
│   ├── render/
│   │   ├── mod.rs     Renderer trait
│   │   ├── inline.rs  indicatif MultiProgress, per-state templates
│   │   └── plain.rs   chronological text writer + JsonWriter
│   ├── strip.rs       artifact strip layout (pure, width-aware)
│   ├── style.rs       color + symbol config
│   ├── log_store.rs   .cook/logs/<build-id>/ writer
│   └── driver.rs      event loop, mode selection, TTY detection
└── examples/          kitchen-sink, narrow-terminal, failure, interactive
```

## Event model

```rust
pub enum ProgressEvent {
    // Topology — sent once, up front, before any recipe starts
    BuildStarted {
        recipes: Vec<RecipeTopo>,    // topo-sorted, declaration-order tiebreak
        total_nodes: usize,
    },

    // Recipe lifecycle
    RecipeStarted   { recipe: RecipeId },
    RecipeCompleted { recipe: RecipeId, elapsed: Duration,
                      cached: usize, total: usize },
    RecipeFailed    { recipe: RecipeId, elapsed: Duration,
                      completed: usize, total: usize },

    // Node lifecycle — artifact-aware
    NodeStarted   { recipe: RecipeId, node: NodeId,
                    artifact: Option<PathBuf>,   // from CacheMeta.output_path
                    fallback_label: String },     // WorkPayload::display_name()
    NodeCompleted { recipe: RecipeId, node: NodeId, elapsed: Duration },
    NodeFailed    { recipe: RecipeId, node: NodeId, elapsed: Duration,
                    error: String },
    NodeCacheHit  { recipe: RecipeId, node: NodeId, artifact: Option<PathBuf> },
    NodeSkipped   { recipe: RecipeId, node: NodeId, reason: SkipReason },

    // Streaming output — goes to the log store, never into the inline frame
    NodeOutput { recipe: RecipeId, node: NodeId,
                 line: String, stream: Stream },

    // Interactive takeover
    InteractiveStart { recipe: RecipeId, node: NodeId },
    InteractiveEnd   { recipe: RecipeId, node: NodeId,
                       elapsed: Duration, success: bool },

    Finished { success: bool },
}

pub struct RecipeTopo {
    pub id: RecipeId,
    pub name: String,
    pub deps: Vec<RecipeId>,
    pub expected_nodes: usize,
}

pub enum SkipReason { UpstreamFailed, ConditionFalse, Disabled }
pub enum Stream { Stdout, Stderr }
```

Changes from today's `cook-cli::progress::ProgressEvent`:

- **Added** `BuildStarted` with full topology (removes `RecipeQueued`).
- **Added** `NodeOutput` — output is scoped to the node that produced it. Replaces `OutputLine { recipe, line }`.
- **Added** `artifact: Option<PathBuf>` on `NodeStarted` and `NodeCacheHit` — drives the artifact strip.
- **Added** `fallback_label: String` on `NodeStarted` — populated from `WorkPayload::display_name()` for nodes without a declared output path.
- **Added** `SkipReason` — user-visible reason for each skip.
- **Added** typed `RecipeId` / `NodeId` — cheap, avoids name collisions.
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
    pub status: Status,            // Waiting | Running | Completed | Failed | Cached
    pub progress: (usize, usize),  // (done, total)
    pub elapsed: Option<Duration>,
    pub nodes: BTreeMap<NodeId, NodeState>,
    pub cached_count: usize,       // cached nodes collapse into this count, not stored
    pub skipped: Vec<(NodeId, SkipReason)>,
    pub error_summary: Option<String>,   // first-error text for end-of-run block
}

pub struct NodeState {
    pub id: NodeId,
    pub artifact: Option<PathBuf>,   // e.g., "build/obj/liblua/lvm.o"
    pub fallback_label: String,      // e.g., "$ ar rcs libliblua.a ..."
    pub status: NodeStatus,          // Waiting | Running | Completed | Failed | Cached | Skipped
    pub started_at: Option<Instant>,
    pub completed_at: Option<Instant>,
}
```

Invariants:

- `BuildState::apply(&mut self, event: ProgressEvent)` is the only mutation path.
- `order` is set once from `BuildStarted::recipes` (already topo-sorted by the engine, declaration-order tiebreak).
- Cached nodes increment `cached_count` but do not add a `NodeState` entry. This keeps memory bounded for recipes with hundreds of cache hits and makes the "cached nodes always collapse to a count" rule trivial to honor.
- Rendering any frame is a pure function of `BuildState` plus `RenderOptions { cols, color, symbols }`.

## Inline renderer

The default when stderr is a TTY. Built on indicatif's `MultiProgress`. Writes to stderr.

### Structure

```rust
pub struct InlineRenderer {
    multi: MultiProgress,
    recipe_bars: BTreeMap<RecipeId, ProgressBar>,
    footer: ProgressBar,
    color: bool,
}
```

- One `ProgressBar` per recipe, added in topo order at `BuildStarted`.
- One additional `ProgressBar` at the bottom acts as a dynamic footer ("3 running · 12 done · 4.1s").
- `MultiProgress::println` is used for the final failure block — content scrolls into scrollback above the live bars.
- No raw `writeln!(stderr, ...)` inside the inline renderer. Everything goes through indicatif so cursor math stays correct.

### Per-state templates

Templates are `ProgressStyle` strings. State transitions call `set_style()` + `set_message()` + `set_length()` on the recipe's bar. `enable_steady_tick(Duration::from_millis(100))` is called once per bar at creation so `{elapsed}` updates smoothly even when no events arrive.

**Waiting** (no `{bar}` token):
```
{prefix:.dim} {msg:.dim}
```
`prefix` = `◇ liblua     ` (symbol + padded name). `msg` = `waiting  ← lua, luac` if deps, else `waiting`.

**Running** (two-line template via `\n`):
```
{prefix:.cyan.bold} {bar:40.cyan/dim} {pos}/{len} · {elapsed}{msg_upstream}
    {msg}
```
`prefix` = `◆ liblua`. `msg_upstream` = ` ← deps` or empty. `msg` = the artifact strip (computed from `RecipeState.nodes` each time it changes).

**Completed** (one-line, no `{bar}` token):
```
{prefix:.green.bold} {msg}
```
`prefix` = `✓ liblua`. `msg` = `33/33 · 0.3s` (plus ` · 10 cached` if any).

**Cached** (one-line, distinct symbol):
```
{prefix:.green.bold} {msg}
```
`prefix` = `≋ liblua`. `msg` = `33/33 cached`.

**Failed** (one-line red header, followed by a `MultiProgress::println` of the fenced error block):
```
{prefix:.red.bold} {msg}
```
`prefix` = `✗ liblua`. `msg` = `8/33 · 2.1s  ← deps`.

**Footer bar** (no `{bar}` token):
```
{msg:.dim}
```
`msg` = `3 running · 12 done · 4 waiting · 10/62 cached · 1.8s`.

No `press 'u' for live UI` hint — TUI is out of scope.

### Artifact strip

Pure function `fn artifact_strip(recipe: &RecipeState, cols: usize) -> String` used as `{msg}` on running recipes.

**Rule** (overflow behavior):

Always show, in order:

1. `{cached_count} cached · ` prefix if `cached_count > 0`.
2. The last up-to-3 **completed** nodes (by completion time).
3. All **running** nodes (sorted by start time).
4. The first up-to-2 **waiting** nodes (declaration order).

When width doesn't fit, drop entries from the right in the reverse of the order shown above (waiting pills drop first, then completed pills, then running pills — running pills are the most important and drop last).

Cached nodes never appear as individual pills. Each pill renders `<symbol> <display>` where:

- Symbol: `✓` completed, `◆` running, `◇` waiting, `✗` failed.
- Display: basename of `artifact` if set, else first token of `fallback_label`, truncated to 20 cols.

Pills joined with ` · `. Width tracked via `unicode-width`. If adding the next pill would exceed `cols - 5`, drop entries from the right and emit ` +N` where N is the number of entries not shown.

For nodes with no `artifact` and no obvious fallback token, the pill is `◆ $ar` (`$` prefix + command name) — rare, only shell commands with no declared output path.

**Worked examples at 80 cols:**

liblua, 27 cached, 6 running, 0 remaining:
```
27 cached · ✓ lcode ✓ ldebug ◆ ldo ◆ ldump ◆ lfunc ◆ lgc ◆ lmem ◆ lobject
```

liblua, 27 cached, 2 running, 12 waiting:
```
27 cached · ✓ ldebug ✓ ldo ◆ ldump ◆ lfunc ◇ lgc ◇ lmem +12
```

Small recipe, no cached:
```
✓ lua.o ◆ lua.bin
```

### Example frame, mid-build at 100 cols

```
✓ deps     12/12 · 0.4s · 10 cached
◆ liblua   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  14/33 · 1.8s
    ✓ lcode ✓ ldebug ◆ ldo ◆ ldump ◆ lfunc ◆ lgc ◆ lmem ◆ lobject ◇ lparser +12
◆ lua       ━                                        0/2  · 0.0s  ← liblua
    ◆ lua.o ◇ lua.bin
◇ luac     waiting  ← liblua
◇ build    waiting  ← lua, luac

3 running · 1 done · 2 waiting · 10/47 cached · 1.8s
```

### Failure frame

```
✓ deps     12/12 · 0.4s · 10 cached
✗ liblua    8/33 · 2.1s
◆ lua      ━                                       0/2 · 0.0s  ← liblua
    ◆ lua.o ◇ lua.bin

─── liblua · lvm.c · failed after 1.8s ─────────────────────────────────────────
│  lua-5.4.7/src/lvm.c:42:9: error: 'bar' was not declared in this scope
│      int foo = bar(x);
│                ^~~
│  compilation terminated.
│
│  cause    : exit status 1
│  command  : clang -c -O2 lua-5.4.7/src/lvm.c -o build/obj/liblua/lvm.o
│  full log : .cook/logs/2026-04-20-a3b/liblua/lvm.c.log
│  replay   : cook logs liblua:lvm.c
────────────────────────────────────────────────────────────────────────────────

skipped 3 downstream nodes: lua/*, luac/*
```

The error block is printed via `MultiProgress::println` once on `RecipeFailed`. It scrolls into scrollback above the live bars and persists after the build ends. Cascaded skips are summarized, not repeated per node.

### Narrow-terminal fix

Indicatif's `ProgressDrawTarget::stderr` handles width, wrap, and `SIGWINCH` natively. The hand-rolled clear-last-frame math in `cook-progress::renderer::clear_last_frame` is deleted. The artifact strip uses `unicode-width` for column budgeting; width budget is `cols - 5` to leave a guard margin.

### Interactive command takeover

Goal: interactive output stays in scrollback on fresh lines below the live bars. Bars are frozen in place (not cleared) at takeover. When the child exits, bars resume only if more work remains.

**On `InteractiveStart`:**

- `multi.println("─── handing off to <recipe>:<node> ───")` prints a divider above the bar region and leaves the cursor at the end of that region.
- `writeln!(stderr, "")` advances the cursor one line below the frozen bars.
- `multi.set_draw_target(ProgressDrawTarget::hidden())` (or drop) stops future redraws. The last frame stays as static text in scrollback.
- Child inherits stdin/stdout/stderr and writes on the fresh lines below.
- `BuildState` continues updating from engine events during the takeover — nothing is drawn.

**On `InteractiveEnd` — lazy resume:**

The driver does not immediately repaint. It waits for the next event:

- If the next event is `Finished` → interactive step was terminal. Do nothing. The frozen bars stay in scrollback, interactive output stays below, the final summary prints below the interactive output via plain stderr writes.
- Otherwise → construct a fresh `MultiProgress` region at the current cursor position, repopulate bars from `BuildState`, handle the event normally.

The old frozen bars remain in scrollback as a snapshot of build state at takeover time — this matches natural `tee` / scrollback behavior.

Signal handling: cook does not intercept `SIGINT` while a child owns the terminal. The child sees `Ctrl-C` directly. On return, cook resumes its own handling.

Implementation risk flag: indicatif's public API supports the freeze/resume dance but does not document it as a supported use case. Verify during implementation (step 4 of migration). Fallback: wrap a minimal cursor-positioning helper that owns the dance end-to-end.

## Plain renderer

Default when stderr is not a TTY, or `--output=plain`, or `CI=true`, or `TERM=dumb`. Pure append-only, no ANSI, no bars, no redraws. Bypasses indicatif.

```
$ cook build 2>&1 | tee build.log
cook build
  deps      queued  (12 nodes)
  liblua    queued  (33 nodes)
  lua       queued  (2 nodes)
  luac      queued  (2 nodes)
  build     queued  (0 nodes)
  deps/protobuf@3.25        cached
  deps/abseil@20240116      cached
  deps/resolve libcurl                                      0.21s
  deps                      done     (12/12, 10 cached)     0.40s
  [liblua/lvm.c] lua-5.4.7/src/lvm.c:42:9: warning: unused 'foo'
  liblua/lvm.c                                              0.88s
  liblua/lzio.c                                             0.41s
  ...
  liblua                    done     (33/33)                1.82s
  lua/lua.o                                                 0.24s
  lua/lua.bin                                               0.18s
  lua                       done     (2/2)                  0.42s
  ...
cook build done in 4.1s (47 nodes, 10 cached)
```

Rules:

- Two-space indent for per-recipe / per-node lines.
- Recipe lines: `<name>` (24 col, left-pad), status word (`queued` / `done` / `FAILED`), optional detail in parens, duration right-padded.
- Node lines: `<recipe>/<node>` indented, duration on the right.
- `NodeOutput` lines interleaved as `  [<recipe>/<node>] <line>` so grep by recipe or node works.
- `NodeCacheHit`: one line, `<recipe>/<node>        cached`.
- `NodeSkipped`: one line, `<recipe>/<node>        skipped (<reason>)`.
- Failure: full error text appended after the failing node's line, each line prefixed with `  [<recipe>/<node>] `.
- No carriage returns, no ANSI. Safe to `tee`, pipe, log-aggregate.

## `--output=json`

One JSON object per line, one `ProgressEvent` → one line. Writes to stderr. Bypasses both inline and plain.

```
{"ts":"2026-04-20T19:55:06.201Z","v":1,"type":"build-started","recipes":[{"id":1,"name":"deps","deps":[],"expected_nodes":12},...],"total_nodes":47}
{"ts":"2026-04-20T19:55:06.214Z","v":1,"type":"recipe-started","recipe":"deps"}
{"ts":"2026-04-20T19:55:06.410Z","v":1,"type":"node-cache-hit","recipe":"deps","node":"protobuf@3.25","artifact":"deps/protobuf-3.25.tar.gz"}
{"ts":"2026-04-20T19:55:06.512Z","v":1,"type":"node-started","recipe":"liblua","node":"lvm.c","artifact":"build/obj/liblua/lvm.o"}
{"ts":"2026-04-20T19:55:06.612Z","v":1,"type":"node-output","recipe":"liblua","node":"lvm.c","stream":"stderr","line":"lvm.c:42:9: warning: unused variable 'foo'"}
...
{"ts":"2026-04-20T19:55:10.301Z","v":1,"type":"finished","success":true,"elapsed_ms":4100}
```

- Every event has `ts` (RFC3339 UTC) + `v` (schema version, starts at `1`) + `type` (kebab-case variant tag).
- Serde-derived, `#[serde(tag = "type", rename_all = "kebab-case")]`.
- `v` bumps on breaking changes; additive changes (new optional fields, new variants) do not bump `v`.
- `v:1` emitted for every event, not just the first — simplifies line-by-line consumers.
- Same schema written to `.cook/logs/<build-id>/events.jsonl` on every build.

## Log store

Always written on every build unless `[ui.logs] events_jsonl = false`. Independent of active renderer.

```
.cook/logs/
├── 2026-04-20-a3b/
│   ├── events.jsonl         full ProgressEvent stream (append-only)
│   ├── manifest.toml        start/end time, command, exit code, schema version
│   └── nodes/
│       ├── liblua/
│       │   ├── lvm.c.log
│       │   └── lzio.c.log
│       └── lua/
│           └── lua.o.log
├── 2026-04-20-9f2/
└── 2026-04-19-001/
```

**Build ID format:** `<date>-<short-hash>` where hash is the first 3 hex chars of a random UUID. Collisions within one day are vanishingly unlikely; on collision append `-2`, `-3`.

**`manifest.toml`:**

```toml
schema_version = 1
build_id = "2026-04-20-a3b"
command = "cook build app"
started_at = "2026-04-20T19:55:06.201Z"
ended_at = "2026-04-20T19:55:10.301Z"
exit_code = 0
cook_version = "0.3.2"
```

**Per-node log format:** raw bytes captured, one stream tag per line:

```
[out] resolving libcurl@8.5.0...
[err] warning: host verification disabled
[out] downloaded 2.3 MB in 210ms
```

**Rotation and bounds:**

- `keep_builds` (default 20) most recent build directories retained. Older ones removed at the start of every build.
- Per-node log file truncated at `max_bytes_per_node` (default 2 MiB) with `--- truncated (N bytes dropped) ---` footer.
- Total directory size bounded by `max_total_bytes` (default 500 MiB). If adding a new build would exceed this, oldest builds removed until it fits.
- Rotation enforced on every build start **before** writing the first event (prevents rotation bugs from filling disk mid-build).

**Writer:**

```rust
pub struct LogStore {
    root: PathBuf,
    build_id: String,
    events_writer: BufWriter<File>,
    node_writers: BTreeMap<NodeKey, BufWriter<File>>,
    manifest: Manifest,
}

impl LogStore {
    pub fn open(project_root: &Path, cfg: &LogConfig) -> io::Result<Self>;
    pub fn record(&mut self, event: &ProgressEvent) -> io::Result<()>;
    pub fn close(&mut self, success: bool) -> io::Result<()>;
}
```

## `cook logs` subcommand

Text-only. Reads `.cook/logs/<build-id>/` directly.

```
cook logs                              # list recent builds (date, duration, outcome)
cook logs --last                       # picker defaults to most recent
cook logs liblua:lvm.c                 # full per-node log from last build
cook logs liblua:lvm.c --build 2026-04-20-a3b
cook logs liblua                       # dump every node in one recipe
cook logs --failed                     # all failed nodes from last build
cook logs --failed --build <id>
```

Output is raw bytes from the per-node files with `[err]` / `[out]` prefixes preserved. No pager integration — user pipes to their own pager if they want.

Selector format: `<recipe>:<node>` for explicit node selection. `<recipe>` alone dumps every node in that recipe, concatenated with headers between them.

## Mode selection

| Context                                   | Renderer |
|-------------------------------------------|----------|
| stderr is a TTY                           | inline   |
| stderr is not a TTY                       | plain    |
| `--output=plain` or `--no-ui`             | plain    |
| `--output=json`                           | json (overrides inline + plain) |
| `CI=true` env (with no explicit flag)     | plain    |
| `TERM=dumb`                               | plain    |
| `NO_COLOR=1`                              | unchanged renderer, colors stripped |

`NO_COLOR` is an overlay modifier — it does not change which renderer runs, only strips ANSI.

## CLI surface

```
cook build                    # auto-detect (inline if TTY, plain otherwise)
cook build --no-ui            # force plain even on a TTY
cook build --output=plain     # equivalent to --no-ui
cook build --output=json      # JSON lines to stderr
cook build --quiet            # recipe status only, no artifact strip, no node output in plain
cook build --verbose          # include stderr stream label in plain
cook logs                     # list recent builds
cook logs --last              # picker on most recent
cook logs <recipe>:<node>     # dump one node's log from last build
cook logs <recipe>            # dump every node in one recipe
cook logs <selector> --build <id>
cook logs --failed            # all failed nodes from last build
cook logs --failed --build <id>
```

No `--ui` flag — TUI out of scope.

## Configuration

`~/.cook/config.toml`:

```toml
[ui]
color = "auto"              # auto | always | never
symbols = "unicode"         # unicode | ascii

[ui.logs]
keep_builds = 20
max_bytes_per_node = 2_097_152     # 2 MiB
max_total_bytes = 524_288_000      # 500 MiB
events_jsonl = true
```

No `[ui.tui]` section.

## Dependencies

```toml
# cook-progress/Cargo.toml
[dependencies]
indicatif = "0.17"
console = "0.15"                 # indicatif transitive, used for TermLike impl
serde = { version = "1", features = ["derive"] }
serde_json = "1"
unicode-width = "0.2"

[dev-dependencies]
insta = "1"
```

Dropped: `crossterm`. Indicatif + `console` replace it.

## Testing strategy

- **Model tests.** Apply canned event streams to `BuildState`; snapshot derived state with `insta`. No terminal.
- **Artifact strip tests.** Pure function, snapshot at widths {40, 60, 80, 120, 200} × recipe sizes {2, 12, 33, 200 nodes}. Covers the overflow rule.
- **Inline renderer tests.** `ProgressDrawTarget::term_like(Box<CaptureTerm>)` where `CaptureTerm: TermLike` captures writes to an internal buffer. Snapshot raw bytes at widths {40, 60, 80, 120, 200}. Covers the narrow-terminal regression.
- **Plain + JSON tests.** Byte-exact snapshots per scenario.
- **Log store tests.** Write canned events, read back, assert shape of `events.jsonl` / `manifest.toml` / per-node files. Rotation test: write N+1 builds with `keep_builds=N`, assert oldest removed.
- **Integration test.** Full canned build end-to-end; snapshots of captured stderr + written log files + manifest.
- **Resize test.** Narrow-terminal regression guard: drive a build with widths alternating {40, 200, 40}, assert no cursor drift / stale lines.
- **Interactive takeover test.** Canned event sequence with `InteractiveStart` followed by further events (resume case) and followed by `Finished` (terminal case). Snapshot both.

Snapshots strip timing-dependent strings (elapsed seconds) via a test helper that normalizes `0.8s` / `1.23s` → `<secs>`.

## Migration

Each step builds + tests pass on its own:

1. **Scaffolding.** New module layout in `cook-progress` (`model/`, `render/`, `event.rs`, `log_store.rs`, `driver.rs`, `strip.rs`) added alongside existing files. Add `indicatif`, `serde`, `serde_json`, `unicode-width`, `insta` to `Cargo.toml`. Keep `crossterm` until step 10. Old `Renderer` / `Frame` / `bar` / `output` keep working — still wired to cook-cli.
2. **Event API + pure model.** Define new `ProgressEvent` in `cook-progress::event`. Implement `BuildState`, `RecipeState`, `NodeState`, `Counters`. Snapshot tests on `BuildState::apply`. Not yet wired into cook-cli.
3. **Artifact strip function.** Pure `fn artifact_strip(recipe: &RecipeState, cols: usize) -> String`. `insta` snapshot tests at the widths/sizes above.
4. **Inline renderer (indicatif).** `render::inline::InlineRenderer` built on `MultiProgress`. Per-state templates. Custom `TermLike` for tests. Snapshot tests. Interactive takeover freeze/resume exercised with canned events.
5. **Plain + JSON renderers.** `render::plain::PlainRenderer` and `render::plain::JsonWriter`. Byte-exact snapshots.
6. **Log store.** `log_store::LogStore` with `events.jsonl`, per-node files, `manifest.toml`. Rotation enforced at open. Roundtrip tests.
7. **Driver.** `driver::Driver` wires state + renderer + log store. Auto-selects inline vs plain. `--output=json` path. End-to-end integration test.
8. **Wire cook-engine.** Engine emits `BuildStarted` up front (requires topology known eagerly — verify against `docs/architecture/execution-flow.md`). Emit `NodeOutput` instead of `OutputLine`. Plumb `CacheMeta.output_path` through `NodeStarted` / `NodeCacheHit` as the `artifact` field. Populate `fallback_label` from `WorkPayload::display_name()`. Surface `SkipReason`.
9. **Swap cook-cli.** Replace `cook-cli::progress::{TtyRenderer, PlainRenderer, ProgressEvent}` with the new driver. Update `main.rs` / `cli.rs` flags (`--no-ui`, `--output`, drop the `press 'u'` hint). Add `cook logs` subcommand.
10. **Delete old code.** Remove `cook-progress::{bar, frame, output, renderer, symbols}`. Remove `cook-cli::progress::{TtyRenderer, PlainRenderer, RecipeRenderState}`. Drop `crossterm` from `cook-progress/Cargo.toml`. Update `docs/architecture/supporting-modules.md`. Replace old examples with new ones.

## Risk register

**Low risk:**

- Pure-model separation is boring and well-trodden. `insta` snapshots are reliable.
- Plain + JSON are append-only writers — hard to regress visually.
- Log store is a bounded file writer.
- Indicatif handles width / wrap / resize as its core competence.

**Watch carefully:**

- **Interactive takeover freeze/resume.** Indicatif's public API supports the pattern (println divider → advance cursor → hide/drop → child runs → fresh MultiProgress on resume) but does not document it as a supported use case. Verify during step 4. Fallback: wrap a minimal cursor-positioning helper that owns the dance end-to-end.
- **Engine emitting topology up front.** Today's engine may compute topology lazily. Step 8 requires it eagerly, before any `RecipeStarted`. Verify against `docs/architecture/execution-flow.md` and plan the adjustment.
- **Artifact strip for nodes with no declared output.** Most shell steps have a `-o` target, but some (`ar rcs target.a *.o`, `make install`, Lua chunks) don't. Fallback to `fallback_label` works, but needs to be populated from `WorkPayload::display_name()` at event-emit time. Step 8 must wire this.
- **Log rotation filling disk.** Bound total bytes, not just per-node. Enforce at every build start before the first event is written.
- **Snapshot test ergonomics.** Indicatif isn't as test-friendly as a pure `Vec<String>` pipeline. `TermLike` capture plus redaction of timing-dependent strings (elapsed seconds) addresses this. One-time implementation cost.

## Out of scope (follow-up projects)

- TUI renderer (own design session — motivated by this spec's `ratatui` TODO deferred).
- `cook recap` subcommand (depends on TUI).
- `u` hot-swap between renderers (depends on TUI).
- Remote recap (SSH / URL).
- Web viewer of `events.jsonl`.
- OTLP / metrics exporter.
- Split-pane multi-log view in TUI.
- Configurable color themes beyond on / off / `NO_COLOR`.
