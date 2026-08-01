# cook-cli

`cook-cli` is the `cook` binary: it turns one command line into one engine
invocation and renders what happened, as terminal output and an exit code.

## How it does that well

- **One chokepoint per surface rule, wired so the next subcommand inherits
  it.** Reserved `//<name>` targets are rejected in `Cmd::reserved_target`
  (`src/cli.rs`), which every target-typed field routes through; a new
  target-typed subcommand inherits the rejection by adding one match arm. It
  stays a post-parse check deliberately: clap's `value_parser` path exits 2 and
  wraps the message in its own `error: invalid value`, while §20.2.4 / CS-0120
  requires exit 1 and the exact `cook: '<target>': …` line.
- **Every command registers the workspace by one path.**
  `build_registered_workspace` validates `@PRESET` once, against the union of
  the named config blocks of every loaded Cookfile rather than just the entry
  one (§11.6 / CS-0165). `cook why` registers in `RegisterMode::Dispatch` for
  the same reason a run does, so the key it reports is the key a run would
  compute; the command it replaced, `cook dag`, registered in `Enumerate` mode,
  which is why its graph was never an explanation of any particular run
  (CS-0171).
- **The read-only commands reuse the runner's closure derivation verbatim.**
  `resolve_reachable_closure` gives `why`, `cache verify`, and `affected` the
  same `build_recipe_infos_from_registered` → `dependency_edges_multi` →
  `edges.keys()` chain `cmd_run` uses, down to the `CycleDetected` /
  `UnknownRecipe` phrasing. A transparency command that computes its own
  closure is a command that can confidently explain a build that will not
  happen.
- **Everything anchors at the workspace root, never at the invocation
  directory.** One resolver, `resolve_project_root`, fixes the cache, the probe
  store, logs, test state, and git-diff scope (§20.2.3 / CS-0120), so `cook
  why` run from a subdirectory reports the key `cook build` computes from the
  root. The current directory gets exactly one job: upward discovery of the
  entry Cookfile in `main.rs`.
- **This crate constructs the cache backend, which is why an argument missing
  here stayed invisible.** `build_registered_workspace` passes a real
  `CacheContext` into the register pass. That slot used to be a literal `None`
  at every call site in the tree, so the register-side probe lookup had never
  run against a backend in any invocation: an `ingredients <probe>` driver
  re-produced on every build while the identical probe consumed through a seal
  was served from cache (COOK-359). CS-0196 then made the same context carry
  key-side project identity (COOK-364).
- **Diagnostics are ordered against the terminal, not against the code that
  produces them.** Stale-output reconciliation lines, zero-match output-glob
  warnings, the store-budget check, and the failing-test count all print after
  the renderer thread has joined, so they cannot fight the progress display.
  `TestFailure`'s `Display` is suppressed in `main` for the same reason: the
  per-node FAILED lines are already on screen.
- **Progress identity is the executor's node index, not the display string.**
  The engine-to-progress bridge interns `NodeId` per `(recipe, unit)`. The
  previous name-keyed scheme collapsed every Lua fan-out member onto one id,
  because they all display as `lua`, and they then overwrote one another's
  state (COOK-213). Output arriving for a unit that never announced itself
  mints a synthetic `NodeStarted`, so a chore window's stdout gets a real label
  instead of a blank one.
- **Completion is one Rust implementation behind `clap_complete`'s dynamic
  engine, and it knows it must not act.** No `completions` subcommand is
  minted, because a new subcommand would grow the reserved set and force `cook
  +completions` on anyone with a recipe by that name. Registration
  (`COMPLETE=fish cook | source`) deliberately does not build the augmented
  command: a shell sourcing its startup file would otherwise execute the
  register-phase Lua of whatever directory it happened to start in. Nothing on
  this path writes to stderr, and every lookup degrades to zero candidates.
- **Cache hygiene cannot turn a green build red.** `check_budget_after_run`
  degrades an unreadable config, a typo'd budget literal, an unwalkable store,
  and a failed sweep to one warning line each, and its `published_count == 0`
  short-circuit precedes both the config load and the store walk, so a settled
  no-op build pays nothing for it (COOK-235).

## What it does not do

It parses nothing, registers nothing, executes nothing, and caches nothing:
`cook-engine` owns all four, and this crate's job at each command is to choose
the entry point and the register mode. It does not render the progress stream
(`cook-progress` owns the renderers; this crate only selects one from
`--output`, TTY, and CI), store build logs (`cook-logs`), collapse a DAG to a
level (`cook-graph`), or define anything two crates must agree on
(`cook-contracts`).

It does not run a shell command either, which is why a `CommandFailure` reaches
`engine_error_to_cook_error` as wire text and is rendered here rather than
built here.

## Relationship to `cook-contracts`

Being the top of the stack, this crate is a consumer of law and an author of
none. Every contract it reaches for has its other end somewhere below:
`CommandFailure::from_wire` against the executor's emitter, `strip_set_e`
against codegen's prelude (COOK-391), `layout::cache_dir` / `probes_dir`
against everything that writes there (COOK-393), `lua_error::sanitize` and
`BACKTRACE_ENV` against the VM that raised the error. When a rendering
question in this crate has a second answer anywhere else, the answer belongs
below, not here.

## Where it falls short of that

`cook modules` is a LuaRocks package manager living inside the build tool's
front end: manifest parsing, a lockfile with integrity verification and TOFU
consent, and a subprocess driver, roughly 1,400 lines that never touch
`cook-engine`. It carries its own error type (`anyhow`) and its own exit path
(`std::process::exit(modules::run(args))` straight out of `dispatch`), so it
sits outside `CookError`'s exit-code classification entirely. It is the "and"
in this crate's one-thing statement, and it wants to be its own crate.

Three decisions are currently implemented twice, each across a dependency edge
that already exists:

- **Whether a name is module-internal** (the `__` prefix).
  `completion.rs`'s copy tests the last dotted segment;
  `cook_progress::naming::is_internal_recipe` tests the whole string. A
  workspace-qualified `game.__cc_config_header__x` is therefore hidden from
  completion and printed raw by the renderer.
- **Whether to emit color.** `progress.rs` treats `NO_COLOR=""` as set and lets
  it override `--color=always`; `test_reporter/style.rs` treats an empty value
  as unset and lets the flag win, with a test citing no-color.org. A single
  `cook test` invocation can answer both ways.
- **The recipe a `TestId` names.** `test_reporter::recipe_of` takes everything
  before the colon, while `cook_engine::id::id_recipe` takes the last dotted
  segment. The JUnit sidecar groups by the first and every other consumer reads
  the second.

And `partition_argv` is a hand-maintained mirror of `Globals`. It re-implements
clap's flag table so that `cook build -v` is honoured the same as `cook -v
build`, which is worth having; the copy has already drifted, though.
`--replay-logs` is missing from it, so `cook build --replay-logs` fails with
"recipes do not take parameters; received 1 positional argument".
