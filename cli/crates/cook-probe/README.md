# cook-probe

`cook-probe` owns the probe lifecycle: the sequence that turns a declared probe
into a value, the atomic materialization of that value, and every read of it
afterwards.

## How it does that well

- It depends inward on `cook-contracts` for canonical probe value meaning.
- It provides one evaluation sequence for registration and execution: resolve
  declared inputs, fingerprint, decide keylessness, look up, produce on a miss,
  publish, materialize, decode.
- It injects the one genuinely phase-specific step. Running a `produce` source
  needs the register VM at register phase and a worker VM at execute phase;
  that difference is a `ProduceRunner` parameter rather than a second copy of
  everything around it.
- It intercepts producer kinds in the sequence, so a kind can never be taught
  to one phase and not the other (COOK-353).
- It publishes probe values with same-directory atomic replacement, preserving
  either the old complete value or the new complete value.
- **It owns both ends of `.cook/probes/<key>.json`.** The writer and the
  read-through store are one crate, so the filename is computed by one call
  and a test can write with one and read with the other without a third party
  agreeing on anything (COOK-422). Until then the reader lived in the
  execute-phase VM crate — the first consumer that needed it — and the two
  halves of a file format sat in crates with no edge between them.
- **A `$<key:field>` reference resolves here too, for the same reason.**
  Rendering a probe reference is reading bytes and applying
  `cook_contracts::sigil::subst`; no VM is involved, so the phase that happens
  to be spawning the command was never part of the answer. It sits beside the
  store it reads and the CS-0157 tool-path view it renders through, which is
  what keeps `cook.probes.get` and `$<key>` from drifting apart.
- It reports non-fatal conditions as returned warnings rather than printing
  them, leaving the diagnostic channel to the phase.

It does not define contracts, choose a VM or sandbox, schedule work, order
`requires`, or prune undemanded probes. Those decisions remain with the runtime
adapters. It does not raise a Lua error either: the CS-0152 not-materialised
sentence is a `String` here, and the execute-phase VM wraps it in an
`mlua::Error` — one sentence, whichever reader hits the miss.

## Cache policy

The crate owns the cache *sequence* and the rules that decide whether a lookup
may happen at all: CS-0178 keylessness and its propagation along `requires`,
COOK-168 publish suppression, and the CS-0102 stale-artifact defence. It does
not own the backend, the store layout, or eviction, which belong to
`cook-cache`.

This is a deliberate move of the boundary. Each of those rules previously
existed in the execute-phase copy and not the register-phase one, and the
register copy's cache block turned out never to have run at all (COOK-359).

## Relationship to `cook-contracts`

`cook-contracts` says what a probe **is**: `ProbeUnit`, `ProbeInputs`, and the
pure rules for rendering and parsing a probe value. Its own layout test forbids
it stateful standard-library access, so it can describe a value but can never
fetch, store, or produce one.

`cook-probe` says what evaluating one **does**. The dividing question is
whether an answer requires touching disk, a process, or a backend: if it does
not, it belongs upstream in `cook-contracts`.
