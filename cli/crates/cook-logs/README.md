# cook-logs

`cook-logs` is the interactive viewer for a build that has already finished: it
turns one loaded `BuildView` into a browsable terminal session over that
build's captured output.

## How it does that well

- It does not read the log format. `cook_progress::log_reader` produces the
  `BuildView`; this crate receives one already in memory. The writer and the
  reader live together in `cook-progress`, so a change to the `.cook/logs/`
  layout cannot half-land in a viewer some other crate owns.
- It opens on the first failed node rather than the top of the tree
  (`state.rs:110`). The question a postmortem viewer is asked is "what broke",
  and it is answered before the first keystroke.
- It refuses to be a TUI when there is no terminal. `run` checks `is_tty`
  before enabling raw mode and prints a plain-text summary instead
  (`tui.rs:25`), so `cook logs | less` and CI capture text rather than fighting
  an alternate screen. The fallback is factored as
  `write_logs_fallback<W: Write>`, so the path a user actually pipes is the
  path a test asserts.
- Drawing is a pure function of `&UiState`, and `run_with_backend` is generic
  over ratatui's `Backend`. A whole frame renders into a `TestBackend` and is
  asserted for content in-process (`src/tests/tui_tests.rs:181`); only the
  event loop needs a real tty.
- It restores the terminal unconditionally. Raw mode and the alternate screen
  are torn down with discarded results before the run result is returned
  (`tui.rs:36`), so a layout error cannot leave the user's shell in raw mode.
- It carries load damage instead of swallowing it. `LoadDiagnostics` (missing
  `events.jsonl`, N unparseable event lines) rides along with the view and is
  printed in the help overlay (`render/help.rs:30`), so a degraded view
  announces that it is degraded.
- Stderr tinting is applied on top of ANSI passthrough, not instead of it
  (`render/output.rs:69`). A compiler's own colours survive and Cook's stream
  attribution is layered over them. Choosing one and dropping the other is the
  output-fidelity class of defect COOK-180 fixed on the write side.
- Tree scrolling is sticky and doubly clamped: the offset moves only when the
  selection would otherwise leave the viewport (`state.rs:156`), and the
  renderer clamps again in case it is handed a stale offset
  (`render/tree.rs:19`). Both edges and the stale-offset case are tested; this
  is the fix in 98f95a41.
- It preserves the prefs that are about the reader, not the build, when
  switching builds in the picker: filter, timestamp gutter, soft wrap
  (`tui.rs:130`). Selection and scroll reset, because those belong to the build
  you left.

## What it does not do

It does not read, write, or know the on-disk log format: no JSONL parsing, no
manifest handling, no `.log` fallback logic. It does not render live progress;
a running build belongs to `cook-progress`'s renderers, and this crate only
ever opens a directory that has stopped changing. It does not own the terminal
outside `run`: `run_with_backend` takes a terminal someone else constructed,
which is exactly what makes the headless and test paths possible. It writes
nothing under `.cook/`.

It should not be parsing timestamps, and it does. See the breaches below.

## Relationship to `cook-progress`

The boundary is ownership of the persisted log format, not "persisted versus
live". `cook-progress` owns both ends of `.cook/logs/`: `log_store` writes it
during a build, `log_reader` replays it afterwards into `BuildView`, and that
same crate also drives the live renderers. `cook-logs` owns none of it. It
receives an immutable snapshot and decides only what a human sees of it: tree
shape, selection, filters, search, overlays, keys, colours.

That split is principled in the direction that matters. Format decisions have
one home, so a viewer can never disagree with the writer about what a line
means. Its cost is that this crate's public API is stated in another domain
crate's types (`run(view: BuildView, …)`), which pins `cook-logs` to
`cook-progress`'s read model. That is the right trade while there is one
reader: declaring a parallel view model here would be a second copy of one
decision, which `cook-contracts`' constitution forbids.

## Known breaches

Recorded rather than papered over. Each is live at the time of writing.

- **The duration law is delegated at two of four sites.** The header
  (`render/header.rs:82`) and the headless fallback (`tui.rs:60`) call
  `cook_contracts::render::duration_ms`; `render/tree.rs:55` and
  `render/output.rs:38` still hand-roll `{:.1}s`. COOK-392's message states
  that all six sites across the workspace delegate; two of this crate's four
  were missed. Visible consequence: in one frame, 61,500ms reads `1m01s` in
  the header and `61.5s` one pane to the left, and 880ms reads `880ms` above
  `0.9s`.
- **A hand-rolled fake calendar.** `render/header.rs:50` parses the ISO-8601
  timestamps that `cook-progress` wrote with the `time` crate, then computes
  `y*365 + mo*31 + d`. A build spanning the end of a 30-day month over-reports
  by a day; a build spanning New Year's Eve computes a negative interval that
  `.max(0)` renders as `0ms`. The fix is not a better parser here:
  `started_at`/`ended_at` have exactly one emitter and one consumer, which is
  the `cook-contracts` admission bar, or the build elapsed should be carried as
  `u64` milliseconds the way every node's already is.
- **One function, twice.** `centered` (`render/help.rs:48`) and
  `centered_rect` (`render/picker.rs:41`) are the same modal-centring rule with
  no agreement test. This is the one real cost of the one-file-per-overlay
  split: the two files never see each other.
- **Reload and build-switch use the wrong root.** `tui.rs:78` derives the logs
  directory from `std::env::current_dir()`, while `cmd_logs` already resolved a
  `project_root` and then dropped it (`lib.rs:43`). Invoke `cook logs` from a
  subdirectory and the first view is correct, but `r`, `b`, and Enter on a
  build silently do nothing, because the failures are discarded by the `if let
  Ok(…)` at `tui.rs:85`, `tui.rs:102`, and `tui.rs:129`.
- **`G` scrolls past the end.** `input.rs:38` sets `scroll_y = u16::MAX` and
  nothing clamps it against the selected node's line count, so "go to bottom"
  renders an empty pane. The same `u16` caps reachable output at 65,535 lines.
- **`h` does not collapse.** `input.rs:28` and `input.rs:32` both call
  `toggle_fold`, so `h` expands a collapsed recipe; the help overlay advertises
  "h/l collapse/expand" (`render/help.rs:16`).
- **`Theme` is a knob with one setting.** The only caller passes
  `Theme::default()` (`cook-cli/src/main.rs:126`), and `--theme` is parsed but
  never wired.
