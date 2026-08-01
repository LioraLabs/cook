# cook-luaotp

`cook-luaotp` runs one captured work item and reports what it did. It is the
execute phase's Lua host: N worker threads, one `mlua` VM each, one `WorkItem`
in and one `WorkResult` out.

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
- **Probe substitution happens here, with no VM in the loop.** `$<key:field>`
  in a shell command is resolved immediately before the spawn, because a
  probe's value is execute-phase and this is the only phase it exists in. The
  rendering is `cook_contracts::sigil::subst`, computed in Rust (CS-0192).
  Under the previous Lua-side `tostring` walk a table interpolated its heap
  address, so the same command line carried different bytes every run, and an
  absent member interpolated the four bytes `nil`. A COOK-361 agreement test
  pins the worker's wrapper to the law it wraps, over the plain value and over
  the CS-0157 tool-path read view, so the two cannot re-fork silently.
- **Register-only API is a guard with a diagnostic, not an absence.** Seven
  names (`exec`, `interactive`, `add_unit`, `step_group`, `recipe`, `probe`,
  `prior_outputs`) raise a §6.3.2 error naming the fix, including the `>>`
  migration hint (SHI-216). This replaced two worse behaviours: `cook.exec`
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

It does not decide what to run, in what order, or how many at once. It owns a
queue and a thread count; readiness is `cook-dag`'s and scheduling is
`cook-engine::executor`'s. It does not consult a cache or compute a
fingerprint, and it does not decide whether a unit needed to run at all.

It does not define the both-phase Lua surface or the sandbox policy: those are
`cook-lua-stdlib`'s, and a fix to `fs.*` belongs there rather than here. It
does not define contracts; `WorkPayload`, `WorkResult`'s chunks, `CommandFailure`,
and the canonical probe-value encoding are all `cook-contracts`'.

It does not spawn processes itself. Every spawn goes through `cook-shell`,
which is also where the ordering guarantee lives; this crate only decides
where the captured chunks go. Disarming `cook-fingerprint`'s stat memo does
stay here, because it is the execute phase's asymmetry: registration is
capture mode and deliberately does not disarm.

It does not run interactive steps. An `Interactive` payload that reaches the
pool is a routing bug and says so in the result rather than trying to cope.

The CS-0045 sandbox it applies is a hermeticity contract, not a security
boundary. The VM is `Lua::unsafe_new()` (required for LuaRocks C extensions on
`package.cpath`), and the confinement is lexical path normalisation, so it
catches an accidental write outside the project rather than a determined one.

## The name

`otp` is Erlang's Open Telecom Platform. In Cook's earliest design, execution
was going to run on a BEAM-style actor platform, and the crate was named for
it. That platform was never built.

So the name describes an architecture that does not exist. What is here is a
fixed pool of OS threads, one `mlua` VM pinned to each, a shared queue, and a
`catch_unwind`: no supervision tree, no behaviours, no restart strategy, no
actors, no message passing between workers. A reader who knows Erlang is
misled more than one who does not, because the acronym leads them to look for
a design the code deliberately abandoned. The docs that describe this crate
already fall back to prose ("worker pool with Lua VMs") every time.

`cook-worker` or `cook-execute` would say what it is. This is recorded rather
than acted on because a rename touches `cook-engine`, the workspace manifest,
the vendored-Lua notes, and five architecture documents; but no one should
have to read `pool.rs` to find out that the crate name is a fossil.

## Standing findings

**The probe-value store has half a contract.** `ProbeValueStore`
(`src/store.rs`) is the read-through cache of `.cook/probes/<key>.json`, and
the function that *writes* those files is `cook_probe::store::materialize_value`.
Two halves of one file contract in two crates, with no dependency edge between
them. It is here because a worker VM's `cook.probes.get` needs it, but the
engine now reaches through `pool.probe_value_store()` from a dozen call sites,
and `cook-engine::why` constructs a bare `ProbeValueStore::new()` with no pool
at all: proof that the store is not the pool's. `resolve_probe_sigils` is the
same story, exported from a Lua-VM crate as a function with no Lua in it, and
`cook-engine::executor` calls it directly for the chore path. `cook-probe`
depends on nothing this crate needs and would not create a cycle, so the store
and the sigil resolver want to move next to the writer.

**`cook.export` visibility depends on which thread ran the producer.** The
execute-phase store (`install_execute_phase_cook_export`, `src/pool.rs`) is a
per-VM Lua table, justified in its own comment by "each recipe is a
self-contained producer/consumer pair within one worker." That is not a
property the pool has: units are pulled off one shared queue by whichever
worker is free, so two units of one recipe routinely land on different
threads. A cross-unit `cook.import` therefore answers `nil` or a table
depending on scheduling. The crate's test pins the isolation half
(`cook_export_store_isolated_per_worker`) and nothing pins the other half,
which is the half that can make a build nondeterministic. Either the store is
shared across the pool or an execute-phase `cook.import` of a name this VM did
not export should be a hard error, as the CS-0152 probe miss already is.

**Smaller ones.** The register-only guard test says it spot-checks
`cook.add_unit` as "representative of all five"; there are seven guards and
one is checked. `run_shell_in_worker` passes a hardcoded line `0` into
`CommandFailure`, where the register-phase twin passes the real Cookfile line;
nothing renders `line()` today, so both sides are filling a field no one
reads. The `io.stdout:flush()` in `execute_lua_chunk` still carries its
pre-CS-0188 rationale ("recipe output (io.write/print)") although both of
those are now captured; it survives only for a body that calls
`io.stdout:write` directly. And `WorkResult::duration`'s doc comment contains
a half-rewritten duplicate sentence.

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
