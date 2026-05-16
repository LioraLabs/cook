# Probe Units Redesign — Design (Phase 2 of CS-0074)

**Date:** 2026-05-16
**Status:** Approved (brainstorm)
**Branch:** `feature/probe-units` (continuation; no rebase)
**Standard target:** 0.11 (unreleased; this work finalises 0.11)

## 1. Motivation

The `feature/probe-units` branch (47 commits, CS-0074) shipped a working probe-units implementation: Standard chapter §22.5, contracts types, register-phase API, fingerprint folding, msgpack value walker, scheduler probe-value store, sigil-based template expansion, three runnable examples, and a positive/negative conformance corpus.

Two issues surfaced when authoring real Cookfiles against this implementation:

1. **Consumer placement is misleading in examples.** All three probe examples put `cook.add_unit({..., requires={"k"}})` at the top level of a `register` block. Top-level units fire unconditionally for every recipe invocation under Cook's existing execution model. The probe rides along via its `requires` edge. The visible behaviour "probe runs every time" doesn't match the API's implicit promise that probes only run when their value is needed.

2. **`requires` overloads a generic word.** On `cook.add_unit`, the `requires` field is strictly probe keys, but the same word means "recipe-name dependency" on `cook.recipe` and "upstream-probe-for-cache-fingerprint" on `cook.probe.inputs`. Reading a Cookfile, the reader has to context-switch to know which kind of dependency each `requires` declares.

This redesign fixes both without throwing away the existing implementation. The unit/recipe model and the probe value-and-cache machinery are preserved; only probe scheduling and one field name change.

## 2. Concept

**Probes are demand-driven DAG nodes.** A probe may be declared anywhere a register-phase API can be called (top-level `register` blocks, top-level module calls, or inside recipe bodies via `>>` / `>>{ }`). At execution time, a probe runs **only when** at least one scheduled non-probe consumer transitively declares the probe's key in its `probes` field. Unreached probes are silently pruned from the execution DAG: no execution, no cache read or write, no warning, no diagnostic.

Non-probe units retain the existing model: every unit captured during the register pass for the invoked recipe is added to the DAG and runs.

## 3. Author-facing pattern

The recommended Cookfile shape for a probe + recipe-scoped consumer:

```
register
    cook.probe("demo:slow", {
        inputs = { },
        produce = [[
            local timestamp = cook.sh("sleep 1 && date +%s"):gsub("%s+$", "")
            return { timestamp = timestamp }
        ]],
    })

recipe build
    >>{
        cook.add_unit({
            name    = "record",
            inputs  = {},
            outputs = {"build/probed.txt"},
            probes  = {"demo:slow"},
            command = "mkdir -p build && echo $<demo:slow.timestamp> > build/probed.txt",
        })
    }
    > cook.sh("cat build/probed.txt")
```

Rules:

- Probes live at the top level when they are environment facts shared across recipes; they live inside a recipe body when they are recipe-local.
- Consumer units live wherever the work itself lives — typically inside a recipe body via `>>` (single line) or `>>{ }` (multi-line block) so the unit is captured only when that recipe is invoked.
- The `probes` field on `cook.add_unit` declares the consumer→probe DAG edge. The `$<key:field>` sigil in command strings auto-populates `probes` (existing behaviour, preserved).

Top-level consumer placement remains legal. It is appropriate for genuinely cross-recipe shared work — for example, `cook_cc.bin(...)` which declares a recipe and the unit that recipe builds in one top-level call.

## 4. Engine change

**Locality.** One function in `cli/crates/cook-engine/src/dag_builder.rs`. No changes to `cook.probe` API surface, fingerprint algorithm, msgpack walker, scheduler, probe-value store, sigil pipeline, or cycle-detection.

**Algorithm.** In `build_dag`, before adding nodes:

```
For each RecipeUnits ru:
    // 1. Index probes by key
    probe_by_key: BTreeMap<String, usize>
        = { u.payload.key -> idx | (idx, u) in ru.units, u.payload is Probe }

    // 2. Seed: probe keys directly listed by any non-probe consumer
    consumed: BTreeSet<String>
        = union over (u in ru.units where u.payload is not Probe) of u.probes

    // 3. Transitive close under probe-on-probe (inputs.requires)
    worklist = consumed.clone()
    while !worklist.empty:
        k = worklist.pop()
        if probe_by_key contains k:
            probe = ru.units[probe_by_key[k]].payload  // ProbeUnit
            for upstream in probe.inputs.requires:
                if !consumed.contains(upstream):
                    consumed.insert(upstream)
                    worklist.push(upstream)

    // 4. Skip set: probe indices whose key is not in consumed
    skip: BTreeSet<usize>
        = { idx | (idx, u) in ru.units, u.payload is Probe(key), !consumed.contains(key) }

    // 5. Existing add-node loop, with skip applied:
    //    - skipped indices are not added to dag, not assigned a dag_id
    //    - probe→consumer edge wiring skips edges whose source is a skipped probe
    //    - sequential-barrier semantics treat a skipped probe as absent
    //      (barrier carries forward from the previous non-skipped unit)
```

**Properties:**

- Deterministic: `BTreeSet` / `BTreeMap` iteration is sorted.
- Probe-on-probe chains prune correctly via the transitive close.
- Top-level consumer with `probes = {"k"}` keeps probe `"k"` reachable. Restricting consumer placement is out of scope; the change targets probe scheduling, not all unit placement.
- Fingerprint and cache semantics for reached probes are unchanged. A pruned probe simply isn't a node this invocation, so it doesn't touch the cache backend.
- Multi-recipe invocation (`cook foo bar`): each `RecipeUnits` is pruned independently. A probe reached by `foo` but not by `bar` still runs as part of `foo`'s DAG.
- A cycle in `inputs.requires` among unreached probes is still caught by the existing register-pass cycle detection, which runs before DAG build.

**Test coverage to add** (all in `dag_builder.rs::tests`):

1. Probe with no consumer → DAG has zero probe nodes; `record` (or equivalent non-probe unit) is unaffected.
2. Probe-on-probe chain where only the downstream has a consumer → both run; upstream runs because the downstream's `inputs.requires` pulls it in.
3. Probe-on-probe chain where only the upstream has a consumer → upstream runs, downstream is pruned.
4. Two-`RecipeUnits` wave where probe is reached by one and not the other → probe appears as a node in the reaching recipe's DAG region, absent from the other.

## 5. Field rename

**`cook.add_unit({requires = {...}})` → `cook.add_unit({probes = {...}})`.**

The field carries probe keys exclusively. The rename makes that explicit at the call site.

**Unchanged:**

- `cook.recipe({requires = ...})` — recipe-name dependency list.
- `cook.probe({inputs = {requires = ...}})` — upstream-probe fingerprint contribution. Within `inputs`, "requires" reads naturally alongside `env` / `tools` / `files`, and the field is contextualised by the surrounding cache-fingerprint contract.

**Implementation surface for the rename:**

- `cli/crates/cook-contracts/src/lib.rs` — `CapturedUnit.requires: Vec<String>` → `CapturedUnit.probes: Vec<String>`. Serde tag stays default (`probes`).
- `cli/crates/cook-register/src/unit_api.rs` — read the `probes` key from the Lua table; the `requires` key is rejected with a register-phase diagnostic naming the line so authors get a clear migration message.
- `cli/crates/cook-register/src/engine.rs` — the requires-resolution loop reads `unit.probes`. The diagnostic text updates:
  ```
  unit '<name>' lists probe key '<key>' in `probes` but no such probe was declared
  ```
- `cli/crates/cook-engine/src/dag_builder.rs` — probe→consumer edge wiring iterates `unit.probes`.
- `cli/crates/cook-luagen/src/template.rs` — sigil auto-population writes to `probes` (the existing code path that auto-adds to `requires` is updated to write to `probes`).

**Rejection diagnostic for the old field name** (transitional, source-locatable):

```
unit at <file>:<line> uses legacy field `requires` for probe references; rename to `probes`
```

The rejection is hard (register-phase error), not a deprecation warning. The branch is unmerged and 0.11 is unreleased, so no compat shim is appropriate.

## 6. Standard amendments

All in `standard/src/content/docs/22-probe-units.mdx`. Version stays at 0.11.

**§22.5.5 — Consumer `probes` field.** Rename the field name throughout; update the diagnostic text; rewrite the §22.5.5 narrative to reflect the renamed field. Add a sentence: the relative source order of `cook.probe` and `cook.add_unit({probes = ...})` calls within a single register pass remains unconstrained; resolution is deferred to end-of-pass (unchanged from CS-0074).

**§22.5.7 — Execution semantics, add a "Demand-driven scheduling" subsection:**

> **Demand-driven scheduling.** A probe unit MUST NOT execute on an invocation unless at least one scheduled non-probe unit lists the probe's key in its `probes` field, directly or transitively through probe-on-probe `inputs.requires` chains. A conforming implementation MUST silently omit unreached probe nodes from the execution DAG. The implementation MUST NOT compute, look up, or persist the fingerprint of an unreached probe; MUST NOT emit a diagnostic; and MUST NOT log a warning at the user-facing diagnostic layer. Debug logging at lower verbosity tiers is permitted.

Add a worked example to §22.5.7: probe in top-level `register`, two recipes (`foo` capturing a consumer, `bar` not). `cook foo` runs the probe; `cook bar` does not; neither produces a diagnostic.

**§22.5.6 — Command-template desugaring.** One-sentence amendment: the sigil expander auto-populates the renamed `probes` field. No semantic change to the desugaring algorithm.

**§22.5.3 — Probe inputs.** Unchanged. (`inputs.requires` stays.)

**Appendix E — CS-0074.** Rewrite the existing entry to reflect the final shape. Bullet points summarising:

> CS-0074 — Probe units. Introduces `cook.probe(key, opts)` (register-phase only), the `probes` field on `cook.add_unit` for declaring consumer→probe DAG edges, the `$<key:field>` sigil for probe-value template references, demand-driven probe scheduling (probes execute only when reached from a scheduled consumer), msgpack-encoded probe-value caching, and the `kind: "probe_value"` artifact-meta tag.

No CS-0075 is minted. 0.11 ships CS-0074 as one coherent feature.

## 7. Examples

Three files under `examples/probe-*/Cookfile` get reshaped to the recommended pattern.

**`examples/probe-basic/Cookfile`:**

```
register
    cook.probe("demo:compiler", {
        inputs = { env = {"PATH"}, tools = {"cc"} },
        produce = [[
            local path = cook.sh("which cc"):gsub("%s+$", "")
            return { path = path }
        ]],
    })

recipe build
    >>{
        cook.add_unit({
            name    = "demo-binary",
            inputs  = {"main.c"},
            outputs = {"build/demo"},
            probes  = {"demo:compiler"},
            command = "$<demo:compiler.path> main.c -o build/demo",
        })
    }
    > cook.sh("./build/demo")
```

**`examples/probe-chain/Cookfile`:**

```
register
    cook.probe("demo:cc-path", {
        inputs = { env = {"PATH"}, tools = {"cc"} },
        produce = [[
            local path = cook.sh("which cc"):gsub("%s+$", "")
            return { path = path }
        ]],
    })

    cook.probe("demo:cc-version", {
        inputs = { requires = {"demo:cc-path"} },
        produce = [[
            local cc = cook.cache.get("demo:cc-path").path
            local ver = cook.sh(cc .. " -dumpversion 2>&1"):gsub("%s+$", "")
            return { version = ver }
        ]],
    })

recipe build
    >>{
        cook.add_unit({
            name    = "print-version",
            inputs  = {},
            outputs = {"build/version.txt"},
            probes  = {"demo:cc-version"},
            command = "mkdir -p build && echo $<demo:cc-version.version> > build/version.txt",
        })
    }
    > cook.sh("cat build/version.txt")
```

**`examples/probe-cache-share/Cookfile`:** already in the recommended shape from in-tree edits. Apply `requires` → `probes` rename only.

## 8. Conformance corpus

Under `standard/conformance/`.

**Updated positive fixtures:** `probe-register-simple`, `probe-requires-chain`, `probe-template-desugaring`, `probe-value-types`. For each: apply the `probes` rename in fixture `Cookfile`; re-baseline `parse.txt` and `register_ok.txt`. `probe-requires-chain` additionally moves its consumer inside a recipe body via `>>{ }` so the chain remains reachable end-to-end.

**Updated negative fixtures:** `probe-duplicate-key`, `probe-unresolved-require`, `probe-cycle`. Apply `probes` rename in fixture Cookfiles and any expected diagnostic strings that reference the field name. `probe-cycle`'s Cookfile exercises `inputs.requires`, which is unchanged — the fixture's Cookfile body needs no edit, only the rename for any consumer add_unit it contains.

**New positive fixture:** `probe-unreached-pruned/`. Cookfile declares a top-level `cook.probe("demo:unused", ...)` and a `recipe build` whose body captures no unit referencing `"demo:unused"`. Harness asserts:

- parse succeeds (`parse.txt` baselined)
- register succeeds (`register_ok.txt` baselined)
- DAG has zero nodes whose payload is `WorkPayload::Probe { key: "demo:unused", .. }` — verified by a new harness assertion that inspects the built DAG
- the cache backend writes no artifact under the fingerprint that `"demo:unused"` would have hashed to

The DAG-inspection assertion is the load-bearing one; the cache-absence check is a belt-and-braces follow-up.

## 9. Integration test

`cli/crates/cook-cli/tests/probe_integration.rs`:

- Rename `requires` → `probes` in the existing `probe_consumer_end_to_end_first_run_then_cache_hit` fixture.
- Add `probe_unreached_is_not_executed`: builds a project with a top-level probe and a recipe whose body captures nothing referencing it. Asserts that running `cook <recipe>` exits 0 and that no probe-value artifact is written to `.cook/cache/`.

## 10. What does not change

- `cook.probe(key, opts)` Lua API, `ProbeRegistry`, capture-state probe push.
- `WorkPayload::Probe`, `RecipeUnits.probes`, the `CapturedUnit` shape (only the field name `requires` → `probes` on `CapturedUnit`).
- Fingerprint algorithm, `ProbeFingerprintInputs`, upstream-probe folding (`inputs.requires`).
- msgpack walker, value-type validation per §22.5.4, encode/decode round-trip.
- `ArtifactMeta.kind` field for probe-value artifacts.
- Scheduler probe-value store, worker-VM dispatch, cache-hit / cache-miss paths in `cook-luaotp` and `cook-engine/executor.rs`.
- `$<key:field>` sigil pipeline in `cook-luagen` and `cook-register/unit_api.rs`.
- Cycle detection on `inputs.requires`, register-only guard on the execute-phase VM.

## 11. Branch hygiene

The redesign lands as additional commits on top of `feature/probe-units`, not a rebase. Honest history. The implementation plan that follows this spec produces a focused commit series; expected scope is ~6–10 commits covering: dag_builder reachability prune, contracts field rename, register diagnostic update, sigil-pipeline rename plumbing, Standard amendments (single commit or paired with the field rename), example reshapes, conformance updates, integration test.

## 12. Out of scope

- General reachability pruning for non-probe units. The existing "all captured top-level units fire" model is preserved.
- Restricting `cook.add_unit` to recipe bodies. Top-level placement remains legal; the recommendation is encoded in examples and Standard prose, not in the parser or register-phase guard.
- A probe-as-lazy-function model (Approach C from brainstorming). Probes remain DAG nodes with explicit `inputs` + `produce`.
- Cross-Cookfile probe sharing (Cook Cloud transport). Cache-format is unchanged; cross-machine sharing follows the existing artifact-store contract.
