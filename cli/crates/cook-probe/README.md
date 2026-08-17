# cook-probe

`cook-probe` owns the probe lifecycle: the sequence that turns a declared probe
into a value, the atomic materialization of that value, and every read of it
afterwards.

## How it does that well

- It depends inward on `cook-contracts` for canonical probe value meaning.
- It provides one evaluation sequence for registration and execution: resolve
  declared inputs, fingerprint, decide keylessness, decide whether the value
  is already resolved without running a VM (CS-0243's two no-VM cases; CS-0242's
  cross-phase serve), produce otherwise, materialize, decode.
- **CS-0243: a reached probe always observes.** There is no probe-value
  cache — no GET, no PUT, no publish, no stored artifact addressed by
  fingerprint. A probe's `produce` body runs every time the probe is reached,
  at most once per key per invocation (CS-0242). Only two resolutions skip
  dispatching a VM, and neither reads a prior invocation's answer: a top-level
  `files`/`tools` declaration's value is synthesised fresh from the current
  tree, and a key this same invocation's register pre-pass already resolved is
  served from that pre-pass's own production.
- It injects the one genuinely phase-specific step. Running a `produce` source
  needs the register VM at register phase and a worker VM at execute phase;
  that difference is a `ProduceRunner` parameter rather than a second copy of
  everything around it.
- It intercepts producer kinds in the sequence, so a kind can never be taught
  to one phase and not the other (COOK-353).
- It materializes probe values to `.cook/probes/<key>.json` with same-directory
  atomic replacement, preserving either the old complete value or the new
  complete value. Across invocations that file is a record, read by `cook why`
  and by nothing else; within one invocation it is also how CS-0242's
  cross-phase serve reaches a key the register pre-pass already resolved.
- **A `$<key:field>` reference resolves here too, for the same reason.**
  Rendering a probe reference is reading bytes and applying
  `cook_contracts::sigil::subst`; no VM is involved, so the phase that happens
  to be spawning the command was never part of the answer. It sits beside the
  CS-0157 tool-path view it renders through, which is what keeps
  `cook.probes.get` and `$<key>` from drifting apart.
- It reports non-fatal conditions as returned warnings rather than printing
  them, leaving the diagnostic channel to the phase.

It does not define contracts, choose a VM or sandbox, schedule work, order
`requires`, or prune undemanded probes. Those decisions remain with the runtime
adapters. It does not raise a Lua error either: the CS-0152 not-materialised
sentence is a `String` here, and the execute-phase VM wraps it in an
`mlua::Error` — one sentence, whichever reader hits the miss.

## Relationship to `cook-contracts`

`cook-contracts` says what a probe **is**: `ProbeUnit`, `ProbeInputs`, and the
pure rules for rendering and parsing a probe value. Its own layout test forbids
it stateful standard-library access, so it can describe a value but can never
fetch, store, or produce one.

`cook-probe` says what evaluating one **does**. The dividing question is
whether an answer requires touching disk, a process, or a backend: if it does
not, it belongs upstream in `cook-contracts`.
