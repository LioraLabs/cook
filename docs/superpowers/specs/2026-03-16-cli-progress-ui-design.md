# CLI Progress UI Design

## Overview

Replace cook's plain-text `[recipe] line` output with a modern progress system inspired by Bun/Just. Progress bars with live streaming output that collapse on completion and expand on error.

## Goals

- Make cook feel polished and fast
- Show parallelism — users should see how their DAG executes
- Surface errors clearly without burying them in noise
- Stay out of the way for interactive steps
- Degrade gracefully when not connected to a TTY

## Three-Layer Display Model

Each active recipe shows up to three layers:

### Layer 1: Recipe Progress Bar

```
◇ lib ━━━━━━━━━━━━━━━━ 3/5 · 0.8s
```

- Fills based on completed DAG nodes vs total nodes for this recipe
- The DAG builder must emit per-recipe node counts alongside the `ExecutionDag`
- Shows recipe name, progress fraction, elapsed time

### Layer 2: Active Nodes

```
  ◇ compile a.c  ◇ compile b.c  ◇ compile c.c
```

- Inline list of currently in-flight DAG nodes within the recipe
- Nodes appear when started, disappear when completed
- Reflects step-group parallelism — multiple nodes visible simultaneously

### Layer 3: Streaming Output

```
  gcc -c src/d.c -o build/d.o
```

- Last 2-3 lines from the most recent command, dimmed
- Shows what's actively happening without overwhelming the display

## State Transitions

### On Completion

Layers 2 and 3 collapse. Recipe bar becomes a single completed line:

```
◆ lib ━━━━━━━━━━━━━━━━━━━━ 0.8s ✓
```

### On Error

Layers 2 and 3 remain expanded. Error output gets a red left-border. Failed nodes shown inline with ✗. The completed nodes in the step group still show ✓ (parallel siblings don't cancel each other):

```
◆ lib ━━━━━━━━━━━━━━━━━━━━ 3/6 ✗
  ◆ compile a.c ✓  ◆ compile b.c ✗  ◆ compile c.c ✓  ○ link skipped
  │ src/b.c:42:5: error: use of undeclared identifier 'foo'
  │     foo(bar, baz);
  │     ^~~
  │ 1 error generated.
```

### Waiting

Recipes whose dependencies haven't completed yet:

```
○ app ━━━━━━━━━━━━━━━━━━━━ waiting
```

## Parallel Recipes

Active recipes stack vertically, each with their own three layers. Collapse on completion keeps the list manageable — you only ever see in-flight work plus completed one-liners.

## Status Footer

A live-updating summary line at the bottom:

```
4 recipes │ 1 done │ 2 running │ 1 waiting
```

## Symbols

`◆` means "finished" (color distinguishes success from failure). `◇` means "in-flight" (color distinguishes recipe-level from node-level).

| State            | Symbol | Color      |
|------------------|--------|------------|
| Finished (pass)  | ◆      | green      |
| Finished (fail)  | ◆      | red        |
| Running recipe   | ◇      | blue       |
| Active node      | ◇      | purple     |
| Waiting          | ○      | dim gray   |
| Success mark     | ✓      | green      |
| Failure mark     | ✗      | red        |
| Skipped          | ○      | dim gray   |
| Cache hit        | ≋      | dim gray   |

In no-color mode, finished vs running is distinguished by `◆` (filled) vs `◇` (open). Active nodes are indented under their recipe, so context disambiguates them from recipe-level `◇`. Cache hits use `≋` which is distinct from all other symbols.

## Cache Hits

Cache-aware display at both the node and recipe level.

### Node Level

When a step is a cache hit, it skips execution and immediately appears with the `≋` symbol (dimmed):

```
  ≋ compile a.c
```

Cached nodes never appear as "active" — they resolve instantly. In a step group with 5 compiles where 3 are cached, you'd see the 3 cached ones already done and only the 2 uncached ones in-flight.

### Recipe Level

The completed recipe bar shows the aggregate cache hit rate:

```
◆ lib ━━━━━━━━━━━━━━━━━━━━ 0.4s ✓ (3/5 cached)
```

If all steps are cached:

```
◆ lib ━━━━━━━━━━━━━━━━━━━━ 0.0s ✓ (cached)
```

### Event

- `NodeCacheHit { recipe, node_name }` — emitted instead of `NodeStarted`/`NodeCompleted` for cached steps

### Status Footer

The footer includes a cache summary when hits are present:

```
4 recipes │ 3 done │ 1 running │ 12/18 cached
```

## Test Output (`cook test`)

Tests use the same progress UI during execution. Individual test cases appear as active nodes within their test recipe.

After all test recipes complete, a test-specific summary replaces the status footer:

### All Pass

```
Test Results
unit-tests ··········· 12 passed (1.2s)
integration-tests ···  8 passed (2.1s)

✓ 20 tests passed (2.5s)
```

### With Failures

```
Test Results
unit-tests ··········· 10 passed, 2 failed (1.2s)
integration-tests ···  8 passed (2.1s)

Failures
✗ unit-tests / test_parser_handles_empty_input
  │ assertion failed: expected Some(""), got None
  │ at tests/parser_test.c:142

✗ unit-tests / test_parser_unicode_escape
  │ assertion failed: expected "\u00e9", got "\x65\xcc\x81"
  │ at tests/parser_test.c:287

✗ 18 passed, 2 failed (2.5s)
```

JUnit XML and JSON files are still written to `.cook/test-results.xml` and `.cook/test-results.json`.

## Interactive Steps

Interactive steps (`WorkPayload::Interactive`) are full DAG barriers. The worker pool must be fully drained (all in-flight work completed) before an interactive step runs. When one is reached:

1. **Clear the progress UI entirely** — remove all bars and cursor-controlled output
2. **Hand over the terminal** — the interactive command gets full stdin/stdout/stderr
3. **On exit, redraw from scratch** — progress UI reappears with updated state (completed recipes shown as done)

Multiple interactive steps run sequentially, each getting a full clear/handoff/redraw cycle. Cook is a task runner — when users ask for interactive, get out of the way.

## Terminal Detection

- **Auto-detect** whether stdout is a TTY
- **Fancy mode** (TTY detected): progress bars, cursor manipulation, colors, live updates
- **Plain mode** (piped/redirected): no ANSI escape codes, clean line-by-line output (current `[recipe] line` format)
- `--color=always/never/auto` flag to override **color** only (not the TUI rendering mode)
- TTY detection independently controls whether to use cursor-controlled progress bars vs plain line output — `--color=always` adds color to piped output but does not force progress bars
- Respects `NO_COLOR` environment variable

## Architecture Notes

### Current State

- All output goes through `SharedWriter` which wraps stdout/stderr with `Arc<Mutex>`
- Output is `[recipe-name] line\n` format, plain text, no colors
- No terminal UI libraries in Cargo.toml

### Required Changes

- Replace `SharedWriter` with a new progress renderer that manages cursor-controlled output
- The renderer needs to:
  - Track recipe states (waiting, running, completed, failed)
  - Track active nodes within each recipe
  - Buffer streaming output (last 2-3 lines per recipe)
  - Redraw on state changes
  - Handle terminal resize
- Add a terminal UI library (e.g., `indicatif`, or build on `crossterm`/`console` directly)
- The scheduler/executor needs to emit structured events (recipe started, step completed, output line, etc.) rather than writing directly to stdout
- Pipeline commands (`cmd_build`, `cmd_test`) wire up the renderer and event flow

### Event Model

The scheduler should emit events rather than writing output directly:

- `RecipeStarted { name, total_steps }`
- `RecipeCompleted { name, elapsed }`
- `RecipeFailed { name, elapsed }`
- `NodeStarted { recipe, node_name }`
- `NodeCompleted { recipe, node_name, elapsed }`
- `NodeFailed { recipe, node_name, elapsed }`
- `NodeCacheHit { recipe, node_name }`
- `OutputLine { recipe, line }`
- `InteractiveStart { recipe }`
- `InteractiveEnd { recipe }`
- `TestResult { suite, case, status, output }`

The renderer subscribes to these events and manages the display. This cleanly separates execution from presentation and makes plain-mode output trivial (just print events as lines).

### Node Display Names

Each `WorkPayload` variant needs a display name for Layer 2:

- `Shell { cmd, .. }` — use the command string (truncated if needed)
- `LuaChunk { .. }` — use `"lua"` or a generated label like `"lua:N"` where N is the step index
- `Test { test_name, .. }` — use the test name
- `Interactive` — not shown in Layer 2 (triggers full UI clear instead)
