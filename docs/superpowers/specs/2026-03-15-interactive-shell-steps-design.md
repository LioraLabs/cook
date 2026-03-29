# Interactive Shell Steps (`@` prefix)

## Summary

Add an `@` prefix for bare shell steps in recipes that gives the child process full terminal access (inherited stdin/stdout/stderr). This enables running interactive programs — TUIs, REPLs, apps with user input — from Cook recipes.

## Motivation

Cook currently captures stdout/stderr from all shell commands to support parallel execution with prefixed output. This breaks interactive programs that need a real TTY. A simple opt-in marker lets recipe authors declare when a step needs the terminal.

## Design

### Syntax

```
recipe run: build
    @./bin/app
end
```

The `@` prefix is only valid on bare shell steps — meaning `Step::Shell` nodes in the AST. It cannot appear in `cook ... using "cmd"` clause strings or `plate "cmd"` command strings. It is allowed in recipes that also contain `cook`/`plate` steps.

### Parser

The `Step::Shell` variant gains an `interactive: bool` field:

```rust
enum Step {
    Shell { command: String, line: usize, interactive: bool },
    // ... rest unchanged
}
```

When parsing a bare shell line starting with `@`, set `interactive: true` and strip the `@` from the command string.

### Codegen

Interactive steps emit a distinct Lua call:

```lua
-- normal shell step
cook.exec("./build.sh", 5)

-- interactive shell step
cook.interactive("./bin/app", 8)
```

Template expansion (`{in}`, `{out}`, `cook.env["VAR"]`, etc.) applies to interactive steps the same as normal steps.

### Runtime

`cook.interactive(cmd, line)` is registered as a new Lua API function in both normal execution mode and capture mode:

- **Normal mode** (registered in `register_cook_api()`): Runs the command with `.status()` instead of `.output()` — stdio is inherited, not captured. Errors on non-zero exit code, same as `cook.exec()`. Uses Cook's merged environment and Cookfile working directory.
- **Capture mode** (registered in `register_cook_api_capture()`): Records a `CapturedUnit` with an `interactive: true` flag into `CaptureState`, mirroring the existing `cook.exec()` capture pattern.

The `--quiet` flag has no effect on interactive steps since their stdio is inherited, not managed by Cook.

### DAG Scheduling

`WorkPayload` gains an `interactive: bool` field (or a new `WorkPayload::Interactive` variant) to carry the flag from `CapturedUnit` through the DAG.

When `execute_dag()` encounters a ready interactive node:

1. **Pause dispatch** — stop sending new work to the `WorkerPool`
2. **Drain in-flight work** — wait for all currently-executing worker tasks to complete
3. **Run on main thread** — execute the interactive command directly (not dispatched to pool), with inherited stdio
4. **Resume scheduling** — continue normal dispatch to the pool

At most one interactive step runs at a time, even across recipes. Multiple `@` steps in a recipe run sequentially, each acting as a barrier.

### Error Handling

- **Parse error** if `@` appears in a `cook using` clause or `plate` command string — message: `"interactive '@' prefix is only valid on bare shell steps (line N)"`
- **Runtime error** on non-zero exit — `CommandFailed { line, exit_code }`
- **No special signal handling** — Ctrl-C terminates Cook normally

### Edge Cases

- **`cook serve` (file-watching mode):** `@` steps are not supported under `cook serve`. If a recipe containing an `@` step is triggered by a file change, Cook should emit a runtime error rather than silently misbehaving.
- **Cross-recipe `@` steps:** If independent recipes both contain `@` steps, they serialize — the barrier logic ensures only one interactive step holds the terminal at a time.
- **Empty `@` command:** Parse error — `@` with no following command is invalid.

## Files Changed

- `src/parser/ast.rs` — add `interactive` field to `Step::Shell`
- `src/parser/mod.rs` — detect `@` prefix, strip it, set flag
- `src/codegen/mod.rs` — emit `cook.interactive()` for interactive steps
- `src/runtime/api.rs` — register `cook.interactive` in both normal and capture modes
- `src/scheduler/dag.rs` — add interactive flag to `WorkPayload` / `DagNode`
- `src/scheduler/mod.rs` — barrier logic in `execute_dag()`

## Future Work

- Built-in interactive primitives (`cook.select()`, `cook.confirm()`) for gathering user input that feeds into subsequent steps
- Signal forwarding to interactive children
- `cook serve` support for `@` steps (pause watcher during interactive step)
