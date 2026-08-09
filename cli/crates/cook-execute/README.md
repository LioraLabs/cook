# cook-execute

`cook-execute` runs one captured work item and reports what it did. It is the
execute phase's Lua host: N worker threads, one `mlua` VM each, one `WorkItem`
in and one `WorkResult` out.

Its twin is `cook-register`, which runs a Cookfile's generated Lua far enough
to know every unit it declares. Two Lua hosts, one per phase, named for the
phases the Standard already names.

The name sits close to `cook-engine`'s `executor.rs`, so the line between them
is stated rather than inferred: **`cook-engine` decides, per unit, whether the
cache already holds the answer; this crate runs the unit that survives that
decision.** Nothing here consults a cache, computes a fingerprint, or asks
whether a unit needed to run.

## How it does that well

- **One VM per thread, built once; everything per-item is a live slot.** The
  VM is `!Send` and never leaves its thread, so it is created and furnished at
  thread start and reused for every item. The working directory, the env maps,
  the sandbox policy, the output sink, and the capture flag are
  `Arc<Mutex<_>>` slots the loop overwrites just before dispatch. That is not
  a micro-optimisation: CS-0017 lets one worker serve items from several
  Cookfiles in one build, so `fs.*` has to resolve against the *active* item's
  cwd rather than the one in effect when the table was installed
  (`WorkingDirSource::Live`), and the same argument makes the sandbox source
  live (CS-0045).
- **The output sink is emptied per item, deliberately.** Before CS-0188 a Lua
  body's `print` and `io.write` went to the worker process's own fd 1, one
  descriptor shared by every thread in the pool; two units printing at once
  interleaved bytes with nothing recording whose were whose. They now route
  into the active unit's accumulator, which makes the clear at the top of each
  item load-bearing: a unit that inherited the previous unit's chunks would be
  reporting someone else's work.
- **The `print` / `io.write` wrappers are written in Lua, not Rust.** Argument
  handling then stays exactly Lua's: `tostring` per argument including
  `__tostring` metamethods, tab joining and a trailing newline for `print`, no
  separators and a returned file handle for `io.write` so `io.write(a):write(b)`
  still chains. Reimplementing that in Rust means reimplementing `tostring` and
  getting it subtly wrong somewhere. The Rust trampoline the wrappers call is
  set back to `nil` afterwards, so a recipe body cannot reach it and write into
  another unit's attribution.
- **Probe substitution happens at the last moment, and not in this crate.**
  `$<key:field>` in a shell command is resolved immediately before the spawn,
  because a probe's value is execute-phase and this is the only phase it
  exists in. The resolver itself is `cook_probe::sigil` (COOK-422): it reads
  bytes and renders them through `cook_contracts::sigil::subst` with no VM in
  the loop, so the phase that happens to be spawning was never part of the
  answer. This crate calls it and hands it the store. Under the pre-CS-0192
  Lua-side `tostring` walk a table interpolated its heap address, so the same
  command line carried different bytes every run, and an absent member
  interpolated the four bytes `nil`; a COOK-361 agreement test still pins the
  store-backed render to the law it wraps, over the plain value and over the
  CS-0157 tool-path read view.
- **Register-only API is a guard with a diagnostic, not an absence.** Seven
  names (`exec`, `interactive`, `add_unit`, `step_group`, `recipe`, `probe`,
  `prior_outputs`) raise a §6.3.2 error naming the fix, including the `>>`
  migration hint (SHI-216) — checked over all seven since COOK-422, where the
  test had been spot-checking one and calling it representative of five. This
  replaced two worse behaviours: `cook.exec`
  silently aliased to a shell-out, which is non-conformant rather than merely
  unhelpful, and the rest surfaced as `attempt to call a nil value`, which is
  compliant by accident and tells the author nothing.
- **Both-phase surfaces are installed, not reimplemented.** `fs.*`, `path.*`,
  `cook.platform.*`, the JSON/YAML codecs, and `cook.tools.id` all come from
  `cook-lua-stdlib` (CS-0044, CS-0123, CS-0158); the module candidate list and
  the `package.path` / `package.cpath` composition come from
  `cook_contracts::layout`, the same two functions the register phase calls
  (COOK-393); every shell spawn goes through `cook-shell`. What is left in this
  crate is the part that is genuinely execute-phase.
- **A panic is a failed result, never a hung build.** The dispatch runs under
  `catch_unwind` and a panic becomes a failure `WorkResult`, so the engine's
  `rx.recv()` always gets its answer. `shutdown` recovers a poisoned queue
  mutex so one panicking worker cannot strand the pool, and `Drop` signals and
  joins, because the workers' `Arc<SharedQueue>` clones would otherwise keep
  the queue alive and leak the threads on the condvar forever.
- **The reported duration is measured, once, around the dispatch only.** Queue
  wait ended when the item was popped, and per-item bookkeeping is the worker's
  own cost, so the clock starts immediately before `execute_work_item`. The
  `Duration::ZERO` in each `execute_*` helper's returned literal is a
  placeholder the loop overwrites on every path out, the panic path included,
  so it can never reach the engine.
- **It knows the difference between a command that failed and one that never
  ran**, records what a command printed *before* it checks the exit code, and
  drops a probe's stdout from the log while keeping its stderr, keyed on
  whether a value was actually produced. A probe's stdout is its value
  (§22.5.2), so logging it would print every finder's answer on each cold run;
  a probe that *failed* produced no value, so all of its output is diagnostic
  again. A chore is the mirror image: it owns the terminal under the CS-0194
  single-drain model, so its body's output and its `cook.sh` streams go
  straight to the real descriptors in call order rather than through the sink.

## What it does not do

It does not decide what to run, in what order, or how many at once — the split
at the top of this file. It owns a queue and a thread count; readiness is
`cook-dag`'s and scheduling is `cook-engine::executor`'s.

It does not define the both-phase Lua surface or the sandbox policy: those are
`cook-lua-stdlib`'s, and a fix to `fs.*` belongs there rather than here. It
does not define contracts; `WorkPayload`, `WorkResult`'s chunks, `CommandFailure`,
and the canonical probe-value encoding are all `cook-contracts`'. It does not
own a probe's value: the store that reads `.cook/probes/<key>.json`, the
CS-0157 read view, and the `$<key:field>` render are `cook-probe`'s, next to
the function that writes the file.

It does not spawn processes itself. Every spawn goes through `cook-shell`,
which is also where the ordering guarantee lives; this crate only decides
where the captured chunks go. Disarming `cook-cache`'s stat memo does
stay here, because it is the execute phase's asymmetry: registration is
capture mode and deliberately does not disarm.

It does not run interactive steps. An `Interactive` payload that reaches the
pool is a routing bug and says so in the result rather than trying to cope.

The CS-0045 sandbox it applies is a hermeticity contract, not a security
boundary. The VM is `Lua::unsafe_new()` (required for LuaRocks C extensions on
`package.cpath`), and the confinement is lexical path normalisation, so it
catches an accidental write outside the project rather than a determined one.

## The name

Settled in COOK-422, and worth recording because the previous name was a
fossil. `otp` is Erlang's Open Telecom Platform: in Cook's earliest design
execution was going to run on a BEAM-style actor platform, and the crate was
named `cook-luaotp` for it. That platform was never built, so the name
advertised an architecture the code deliberately abandoned — a reader who
knew Erlang was misled more than one who did not, sent looking for a
supervision tree, behaviours and message passing that are not here. What is
here is a fixed pool of OS threads, one `mlua` VM pinned to each, a shared
queue, and a `catch_unwind`.

`cook-execute` was chosen over two alternatives. `cook-worker` names the
mechanism, and the mechanism is already the first line of this file; the
constitution asks for boundaries to be named, and the boundary here is the
phase line, with `WorkItem` → `WorkResult` as the value handed across it.
`cook-lua-exec` would have made `cook-register` the odd one out in a Lua
family it belongs to. Naming the pair for the two phases the Standard already
names is what makes them legible as a pair.

The known cost is the near-collision with `cook-engine`'s `executor.rs`, and
the top of this file and of `cook-engine`'s answer it directly rather than
leaving a reader to work it out.

## Standing findings

None. The four this file carried are closed, and how each closed is worth
more than the fact that it did.

**The probe-value store had half a contract**, with `ProbeValueStore` here and
`materialize_value` in `cook-probe`, no dependency edge between them, and the
engine building a bare store with no pool in sight. Both halves live in
`cook-probe` now (COOK-422), along with `resolve_probe_sigils` — which was a
function with no Lua in it exported from a Lua-VM crate — so the reader of
`.cook/probes/<key>.json` is testable against its writer in one crate.

**`cook.export` visibility depended on which thread ran the producer**, and
the crate's own test pinned the isolation half while nothing pinned the half
that made a build nondeterministic. CS-0200 withdrew the surface instead:
`cook.export` and `cook.import` are register-phase only and the worker VM
refuses both by name. This finding was already stale when COOK-422 read it,
which is the argument for re-reading a charter's findings rather than trusting
them — a standing finding is a claim with a date on it.

**The register-only guard test checked one of seven** while calling itself
representative of five. It is a table over all seven (COOK-422).

**`run_shell_in_worker` passed a hardcoded line `0`.** Recorded here as
harmless because nothing read `CommandFailure::line()`; `cook-cli`'s
`render_command_failure` does, and drops the location entirely when the line
is zero, so every execute-phase `cook.sh` failure was reported without one.
CS-0211 makes the phase symmetry normative and the line comes from the walk
the register phase already had, shared from `cook-lua-stdlib`.

## What this VM shares with the register VM, and what it keeps

The doors here — `member_to_string`, `dep_output`, `dep_output_list`,
`add_unit`, `step_group`, `prior_outputs`, `interactive`, the `cook.probes`
table, the read-only `var` global — were each spelled and implemented twice,
once here and once in `cook-register`, and the constitution's waiver files
listed nine of them by name. COOK-439 / CS-0213 closed that, and the split it
settled on is worth stating because every future door faces the same choice.

The NAMES are pure law and live in `cook_contracts::registration`. Nothing
about a door name needs mlua, both crates already depend on contracts, and the
gate can see a name in two crates — which is how these were found.

The doors whose whole behaviour is identical live in `cook-lua-stdlib`:
`install_member_to_string` is the entire door, name and body and diagnostic.
The doors whose behaviour is *mostly* identical live there too, with the
difference passed in — `install_probes_api` takes this phase's read and its
CS-0074 refusal as two arguments, `install_var_proxy` takes this phase's
lookup and its refusal sentence. That is `caller_line_in_source`'s shape
(COOK-422) applied at scale: parameterise over the one thing that genuinely
differs, and leave the phase-specific spelling at the call site.

What stays here is what execute phase actually means: the §6.3.2 guards that
refuse the register-only doors (they refuse them by the shared constant, so a
guard cannot come to name something nothing installs), the read-only
resolution of `dep_output` against the registration snapshot, and the
per-run probe-value store the shared `cook.probes` table reads through.

`cook-engine/tests/both_phase_door_agreement.rs` is the guard on all of it: it
drives one Lua fragment through both VMs and asserts they answer identically.
It is trivially true today, which is the goal state — it exists to notice a
re-fork, not to report a bug.

## Relationship to `cook-contracts`

`cook-contracts` says what a work item and its result **are**: `WorkPayload`,
`StepKind`, `OutputChunk`, `CommandFailure`, the sigil grammar and its
substitution rules, the canonical probe-value encoding, the module candidate
order. Its own layout test forbids it stateful standard-library access and it
may not touch `mlua`, so it can describe a unit of work but never run one.

This crate runs one. The dividing question is whether an answer requires a VM,
a thread, or a process: if it does not, it belongs upstream, in
`cook-contracts` if it is pure and in `cook-lua-stdlib` if it merely needs
`mlua`.
