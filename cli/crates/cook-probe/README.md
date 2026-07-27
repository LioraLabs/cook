# cook-probe

`cook-probe` owns the probe lifecycle: the sequence that turns a declared probe
into a value, and the atomic materialization of that value.

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
- It reports non-fatal conditions as returned warnings rather than printing
  them, leaving the diagnostic channel to the phase.

It does not define contracts, choose a VM or sandbox, schedule work, order
`requires`, or prune undemanded probes. Those decisions remain with the runtime
adapters.

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
