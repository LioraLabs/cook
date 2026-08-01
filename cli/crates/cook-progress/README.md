# cook-progress

`cook-progress` owns Cook's build-progress event stream: it folds the stream
into one state, and every surface a build is seen through is a rendering of
that state. The live terminal, the plain CI lines, `--output=json`, the
retained `.cook/logs/` record, and the replay `cook logs` reads back are the
same events through the same `BuildState`.

## How it does that well

- **One fold, one input.** `BuildState::apply` takes a `ProgressEvent` and
  nothing else; no renderer reaches past it to the engine. That is what lets a
  renderer resolve a node's name at all, and it is load-bearing: `apply`
  registers an interactive node on `InteractiveStart` purely so the JSON writer
  can look the name up from state rather than re-reading the inline field
  (`src/model/build.rs:176`).
- **One serde type is the wire format, and both ends use it** (COOK-394).
  Before that, the writer built about forty `json!` key literals, the reader
  hand-`.get()`ed the same keys, the failed-count scan was a *second*
  independent parser, `NodeKind` was respelled kebab-case by hand beside its own
  derive, and `"upstream-failed"` was hardcoded beside `SkipReason::as_str`.
  Reader defaults were silent, so drift showed up as misattribution in `cook
  logs` replay rather than as an error. `wire::WireEvent` now serializes for the
  streaming writer, for `LogStore`, and deserializes for both readers.
- **The version policy is stated and enforced at the read edge** (CS-0048,
  CS-0035). `v` gates the line; unknown fields are ignored rather than rejected,
  so evolution stays additive; and an unrecognised `type` under an *accepted*
  `v` is treated as a newer writer's event and skipped, not counted as corrupt
  (`src/log_reader.rs:315`). Key order is lexicographic because the writer goes
  through `serde_json::to_value`, whose `Map` is BTreeMap-backed, so adding a
  field never churns a downstream diff.
- **It does not render durations; it calls the law** (COOK-392, and the CS-0198
  round that made duration rendering one user-visible rule). Three sites here
  call `cook_contracts::render::duration_ms`. That decision once existed six
  times across four crates, and the same 61,500 ms printed as `1m01s`, `1m1s`,
  `61.5s`, and `61500ms` in a single terminal session. This crate carries its
  one deliberate `cook-*` dependency edge precisely so it cannot become a
  seventh copy.
- **The warm build collapses to one line per recipe.** Cached node lines are
  held per recipe and released only on evidence of real work; a recipe that
  finishes having done nothing but hit cache prints a single dim
  `Cached <recipe> (N nodes)`. Toolchain probes group into one
  `Resolved <module> toolchain` line, and a fully-cached probe set stays silent.
  Overflow past the per-recipe threshold is reported once, at the recipe's final
  flush, as `… (N more cached)`.
- **A node's label is its own output path, never raw command text** (COOK-213).
  `NodeState::display` prefers the declared artifact *in full*, so the
  distinguishing directory segment (`packages/`, `build/`) is never dropped;
  otherwise it cleans the fallback label to one token and `$`-prefixes it to
  mark it as command text. A state miss yields a placeholder rather than an
  empty string, which is the bug that used to render a bare `report/`. The `@N`
  source-line token and `probe:` prefix are passed through as-is.
- **The status line is a pure function plus a thread.**
  `render_status_line(snapshot, opts, cols)` takes no I/O and is snapshot-tested
  at fixed widths; `status_line.rs` owns only `arc_swap` publication,
  visibility, and the stderr lock. indicatif and console were deleted rather
  than configured. The inline renderer also composes event lines into a buffer
  first, so an event that produces no output emits no clear sequence: spraying
  `\r\x1b[2K` on every event bloated recorded ptys for nothing.
- **Terminal handoff is a latched state, not a guess.** A terminal
  `InteractiveEnd` sets `after_terminal_chore`, after which every event renders
  nothing, so a chore that took the terminal remains the user's last view. That
  is the `cargo run` shape, and it is one flag rather than a condition
  re-derived per event type.

## What it does not do

It does not decide what happened. `cook-engine` deliberately does not depend on
this crate; it keeps its own `NodeKind` / `RecipeKind` and the CLI translates in
an exhaustive match (`cook-cli/src/pipeline.rs:106`), so a new variant on either
side is a compile error rather than a silent default.

It does not own the log-reading UI. `cook logs` is `cook-logs`: ratatui widgets,
search, theme, key handling. The boundary is the format, not the direction of
travel; this crate owns `events.jsonl` and *both* its ends, because COOK-394
proved that a reader living apart from its writer drifts silently. `cook-logs`
consumes `log_reader::BuildView` and the event enums, nothing else.

It does not render a duration, parse a probe label, or spell the logs directory.
Those are `cook-contracts`.

It has no live frame, no cursor addressing beyond a single clear-line, and no
terminal library. Everything above one sticky line is `writeln!`.

## Two things to fix before building on this

**The two append-only renderers are an inadmissible copy.**
`render/event_writer.rs` and `render/plain.rs` implement the same noise
decisions twice: the per-recipe cached hold, probe grouping (down to the
singular/plural noun and the `(N probes, M cached)` detail), internal-recipe
suppression, the interactive `@N` label strip, and the "no real work" test
`cached + probes_ran >= total` (`event_writer.rs:224`, `plain.rs:139`). Each has
its own `RecipeBuffer`. `plain.rs:105` names its twin, which satisfies one third
of the deliberate-copy protocol; there is no agreement test and no comment
naming a rejected home, because there is no rejected home: these are pure
functions of plain data. The copy has already drifted. `plain.rs:210` hand-spells
the three `SkipReason` strings in a local match, beside the
`SkipReason::as_str` that `event_writer.rs:200` calls: the exact drift family
COOK-394 closed on the wire, still open on the renderer.

**`check_schema_version` has no production caller.** It is public, exported from
the crate root, and unit-tested (`render/json.rs:232`), and the one place in the
workspace that needs the CS-0048 read policy re-derives it with a local
`Envelope` struct (`log_reader.rs:298`). Worse, the failed-count scan at
`log_reader.rs:187` does not gate on `v` at all, so a future v2 `node-failed`
line would be counted in a build summary that the full replay refuses to read.

## Relationship to `cook-contracts`

`cook-contracts` owns what a rendered value MEANS: the duration law, the
probe-label parse pair, the layout constants, the `PROBE_LABEL_PREFIX` literal.
Its own layout test forbids it stateful standard-library access, so it can
decide how a duration reads but can never take a clock or a terminal width.

`cook-progress` owns the state and the effects: the fold, the files, the thread,
the escape codes. The dividing question is whether more than one surface must
agree on the answer and the answer is a pure function of its arguments. If both,
it belongs upstream, and the edge in `Cargo.toml` is the cheap half of the deal.

## Module map

| Area | Files | What it is |
|---|---|---|
| `event.rs`, `wire.rs` | 2 | The event enum and the `events.jsonl` schema |
| `model/` | 4 | The fold: build, recipe, node state |
| `render/` | 7 | Renderer trait; inline, plain, JSON, status line |
| `log_store.rs`, `log_reader.rs` | 2 | Write and replay `.cook/logs/<build-id>/` |
| `driver.rs`, `naming.rs`, `style.rs`, `lib.rs` | 4 | Event loop, display naming, verb table, index |

Nineteen source modules, about 3,000 lines, averaging 160 each. The remaining
files are eighteen `#[path = "tests/…"]` extractions following the workspace
convention plus one example, not further fragmentation.
