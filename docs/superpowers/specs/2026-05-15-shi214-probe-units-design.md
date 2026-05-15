# SHI-214 — Probe units (design)

**Ticket:** [SHI-214](https://linear.app/shiny-guru/issue/SHI-214) — parent will be re-filed (see §10). Originally filed under [SHI-132](https://linear.app/shiny-guru/issue/SHI-132) (Cook builds Doom 3).
**Date:** 2026-05-15.
**Status:** design.
**Origin:** SHI-214 was filed against an incorrect premise — "`cook.cache` today is per-invocation in-memory only" — that proposed adding disk persistence as a small interim before the worker-pool daemon shipped. Brainstorming surfaced two facts that reshape the work:

1. The **register-phase `cook.cache` is already disk-persistent** via `cli/crates/cook-register/src/module_cache.rs` (per-module JSON file under `.cook/cache/<module>.json`, source-hash invalidation, full get/set/invalidate/clear/flush API).
2. The execute-phase `cook.cache` (CS-0070) is intentionally in-memory only with explicit reference to SHI-214 as the "disk-persistent design."

Brainstorming then reframed the problem entirely: rather than bolt cross-invocation persistence onto the execute-phase kv store, **model cache reads and writes as DAG nodes**. Cache producers become *probe units* with declared inputs; cache consumers declare `requires = { key, … }`; the engine wires DAG edges; the existing recipe-cache machinery handles fingerprinting, persistence, and cloud sharing for free.

**Final design shape:** A new Lua API `cook.probe(key, { inputs, produce })` registers a probe unit. Consumer units declare `requires = { "key1", "key2" }`. Probe units run in the execute phase as ordinary DAG units; their output is the Lua value the `produce` function returns, serialised to bytes and stored as an artifact in the existing recipe cache. Consumer commands are template strings (`"{{cc:zlib.cflags}}"`) that get desugared at codegen time into Lua reads from the populated cache. The current `cook.cache.set/get` Lua surface is deprecated and removed in v1.x; `cook.export/import` is unaffected.

## 1. Decisions recap

| # | Question | Decision |
|---|---|---|
| Q0 | What problem are we solving? | Cross-invocation memoisation of slow probes (pkg-config, cmake-compat, compiler detection) **and** cross-machine sharing of those results via Cook Cloud. |
| Q1 | Phase model? | Probe units run in the execute phase as ordinary DAG units. Register phase stays synchronous — it declares probes and consumers, the engine wires edges, the scheduler runs them in topo order. No new "probe phase," no interruptible register-phase Lua, no async surface at the Lua layer. |
| Q2 | Probe declaration surface? | New dedicated function `cook.probe(key, { inputs, produce })` in the Lua API. Not a Cookfile keyword (probing stays inside the Lua API surface; module-author concern). Not a flavour of `cook.add_unit` (named-after-purpose is clearer for module authors and lint passes). |
| Q3 | Consumer surface? | Unit declares `requires = { "key1", "key2" }`. Engine looks up the probe units that produce those keys and adds DAG edges at register-time. Topo order guarantees probes run before consumers. |
| Q4 | Command-template values? | Command strings support `{{key}}` and `{{key.field}}` placeholders. At codegen time (`cook-luagen`), these desugar to a Lua step that calls `cook.cache.get(key)` and interpolates fields before running the shell command. No new template engine; no command-as-function surface. |
| Q5 | Coexistence with `cook.cache.set/get`? | Deprecate and remove `cook.cache.set/get` (Standard amendment with deprecation window; removed at v1.x). `cook.cache.get(key)` is retained as the **read-side primitive** that the desugared command-Lua calls into — it now reads from the probe-unit cache store, not a free-form kv. **Plain Lua locals/module tables handle scratch state.** `cook.export/import` (CS-0071) is unrelated (output-ABI propagation, not memoisation) and unchanged. |
| Q6 | Probe input categories? | Tabular by kind: `env` (folded by current value), `tools` (folded by content hash via CS-0052 declared-tools), `files` (folded by content hash), `requires` (chained probes — their fingerprints fold in). Other input kinds (e.g. a `commands` escape hatch for shell-output-as-input) deferred to a follow-up. |
| Q7 | Value types? | Lua subset: `nil`, `bool`, `number`, `string`, `table-of-same`. No functions, userdata, threads. Standard normative — implementations MUST raise if a probe returns a non-serialisable value. |
| Q8 | Value serialisation? | msgpack. Compact, schema-free, maps naturally onto the Lua-table value shape. Stored as bytes in the existing recipe-cache artifact format with the same SHA-256 integrity meta sidecar (CS-0054). |
| Q9 | Storage layer? | Reuse the existing recipe cache. Probe-unit output is an artifact, same `CacheBackend::put/get` machinery, same `.cook/cache/<hash>.bin` on-disk layout, same Cook Cloud R2 sync via CS-0058/CS-0059. No new persistence layer; no new cache directory. |
| Q10 | Cloud sharing? | Falls out for free. Probe artifacts ride the same cloud wire as compile outputs. A CI machine that probes `pkg-config zlib` on `x86_64-linux-gnu` uploads the artifact under its fingerprint; any developer with matching fingerprint inputs downloads instead of re-probing. No protocol changes. |
| Q11 | Probe-failure semantics? | "Not found" is **not** an error. Probes return a structured value the consumer branches on (`{ found = false, tried = {…} }`). A probe genuinely erroring (subprocess crashed, malformed output, non-serialisable return) raises and fails downstream consumers via the DAG edge — same as any unit failure. `cc.find_or_error` becomes a thin Lua wrapper that asserts on `found = false`. |
| Q12 | Key namespacing? | Global. Convention-based prefixes (`cc:zlib`, `cc:compiler`, `py:numpy`). Two `cook.probe(key, …)` calls with the same key in one register pass: **hard error** at register-time naming both source locations. Forces module authors to coordinate. |
| Q13 | Probe chaining? | Probes are units, so they can declare `requires` too. `cc:zlib` requires `cc:compiler`; topo order handles it. No special case. |
| Q14 | Within-invocation caching? | A probe runs **at most once per build**. `cook.cache.get("cc:zlib")` invoked from two different consumers hits the same probe-unit output; the probe runs once, the second consumer reads the in-memory store. |
| Q15 | Migration order? | Three phases. Phase 1: ship `cook.probe` infra + Standard § + full conformance, no callers. Phase 2: cook_cc migrates `init()`'s compiler detection (smallest, well-bounded). Phase 3: cook_cc migrates pkg-config + cmake-compat finders. Phase 4 (post-Doom-3): deprecate `cook.cache.set/get` Standard surface; remove in v1.x. |

## 2. Scope

**In scope:**

1. **Cook Standard updates:**
   - New § (likely §9.3 or §6.5): `cook.probe` surface, probe-unit semantics, fingerprinting model, value-serialisation contract.
   - §6.3.4 (cook.cache): amend to deprecate `cook.cache.set`; retain `cook.cache.get` as the read-side primitive for probe values; mark `set` for removal at v1.x.
   - §8.6 (caching): cross-reference probe units as cache participants alongside recipes.
   - §12 (modules): amend the line that permits "in-memory-only storage scoped to the worker's lifetime" — replace with a forward-reference to the new probe-units §.
   - CS-numbers TBD (anticipate three: probe surface, fingerprint inputs, value-serialisation).
2. **Engine changes:**
   - `cook-register`: new `ProbeUnit` variant alongside the existing `Unit`. `cook.probe(...)` registers a probe-unit; `requires` on a regular unit resolves to a probe-unit by key and contributes a DAG edge.
   - `cook-luagen`: extend command-string codegen to detect `{{key}}` / `{{key.field}}` placeholders and emit a desugared Lua step that calls `cook.cache.get` and interpolates.
   - `cook-luaotp`: probe-unit execution runs the `produce` Lua function on a worker VM; return value is serialised via msgpack and handed back to the scheduler for artifact storage. `cook.cache.get(key)` on the worker VM reads from a per-run shared store populated by completed probe units.
   - `cook-fingerprint`: extend `step_context_hash` to fold a probe-unit's declared inputs into its fingerprint (env values, tool content hashes, file content hashes, upstream probe-unit fingerprints).
   - `cook-cache`: probe-unit artifacts use the existing `CacheBackend::put/get` path; the serialised msgpack bytes are the artifact body; the SHA-256 of those bytes goes in the meta sidecar.
3. **cook_cc migration (separate rocks releases):**
   - cook_cc 0.5.0: `init()` compiler detection becomes a `cook.probe("cc:compiler", …)`. Probe is registered eagerly at module load; consumers (`cc.find`) declare `requires = { "cc:compiler" }`.
   - cook_cc 0.6.0: pkg-config finder and cmake-compat finder become probes (`cc:pkgconfig:<name>`, `cc:cmake:<name>`). `cc.find` becomes a thin dispatcher over probe results.
4. **Conformance fixtures:** parse + register-positive fixtures for probe declaration, probe chaining, probe + consumer wiring, fingerprint-input categories, value-serialisation round-trip, duplicate-key error.

**Out of scope:**

1. **Cookfile-level `probe NAME … end` keyword.** Probing stays in the Lua API surface; module-author concern. Reconsider if probe usage grows beyond modules into end-user Cookfiles.
2. **`commands` input category (shell-output-as-fingerprint-input).** Useful for cases like "`git config user.name` as a probe input." Adds fingerprint-input recursion. Followup ticket.
3. **`cook.cache.set` removal in this design's PR series.** Standard amendment lands in v0.10.x with a deprecation note; removal is a v1.x-cut concern.
4. **Probe TTL / time-based invalidation.** Cook builds are deterministic over their inputs; time isn't an input. If a use case appears, file separately.
5. **Probe sandboxing.** A probe's `produce` function has the same Lua-API surface as any worker VM. We don't restrict what it can call. Standard normative claim is "MUST be a pure function of declared inputs"; enforcement is by author convention + lint, not by runtime sandbox.
6. **Interactive probes.** A probe cannot prompt the user. If `produce` blocks on stdin, it hangs the build. Standard normative.
7. **Probe results as DAG-viewer first-class.** Probe units appear in `cook --dag` like any other unit; rendering their *value* in the viewer is a UX follow-up, not part of this design.
8. **`cook.export/import` integration.** Independent surface (CS-0071), independent purpose (output-ABI propagation, not memoisation). Probe + export can compose at the consumer-unit level but neither subsumes the other.

## 3. Architecture overview

### 3.1 Phase model

```
┌──────────────┐      ┌──────────────────────────────────────────┐
│   Register   │      │              Execute phase                │
│    phase     │      │                                           │
│              │      │  ┌──────────┐    ┌──────────┐             │
│  Lua runs    │──▶───┼──▶ Probe     │───▶│ Consumer │             │
│  sync to     │      │  │ unit(s)   │    │ unit(s)  │             │
│  completion  │      │  │ (run only │    │ (read    │             │
│              │      │  │ if cache  │    │ probe    │             │
│  declares:   │      │  │ misses)   │    │ values)  │             │
│  • probes    │      │  └──────────┘    └──────────┘             │
│  • consumers │      │       ▲              ▲                    │
│  • DAG edges │      │       │              │                    │
│              │      │       └──── cache ───┴──── recipe cache   │
└──────────────┘      │             store           + cloud R2     │
                      └──────────────────────────────────────────┘
```

The register phase is unchanged: synchronous Lua, builds the DAG, terminates. The DAG now contains two unit varieties (`Unit` and `ProbeUnit`) and the edge set includes `requires`-derived edges from consumers to probes. The execute phase schedules units in topo order via the existing scheduler; probe units run first by topology, populate a per-run cache store, and the consumer units read from that store via the existing `cook.cache.get` read primitive.

### 3.2 Probe-unit lifecycle within one build

1. **Register-time:** `cook.probe("cc:zlib", {...})` registers a `ProbeUnit` with key, inputs, and produce function. Module Lua source is captured; the produce function's source-hash is part of the fingerprint.
2. **Fingerprint compute:** at the start of execute phase, each probe unit's fingerprint is computed from its declared inputs (env values, tool hashes, file hashes, upstream probe fingerprints).
3. **Cache lookup:** for each probe unit, the scheduler asks `CacheBackend::get(fingerprint)`. On hit, the cached msgpack bytes are deserialised and stuffed into the per-run cache store; the probe is marked complete without running.
4. **Cache miss:** scheduler dispatches the probe-unit to a worker VM. Worker runs `produce()`, captures the return value, validates it's serialisable (Standard §value-types), msgpack-encodes it, and hands bytes back. Scheduler `CacheBackend::put`s the artifact and populates the per-run store.
5. **Consumer dispatch:** consumer units schedule after their `requires`-edge predecessors complete. Their commands have already been desugared at codegen time into Lua steps that call `cook.cache.get(key)` to read the populated store before running the shell.
6. **Cleanup:** per-run cache store is dropped at end of execute phase. Probe artifacts persist in `.cook/cache/<hash>.bin` and (if cloud is enabled) in R2.

### 3.3 Component boundaries

- **`cook-register`** owns probe-unit registration, key-namespace validation (duplicates → error), `requires` resolution against the probe registry, and DAG edge insertion. Adds a `ProbeUnit` data type alongside `Unit`.
- **`cook-luagen`** owns command-template codegen. Detects `{{key}}` / `{{key.field}}` patterns, rewrites into a Lua step that reads `cook.cache.get(key)` and interpolates. Pure compile-time transformation; no runtime template engine.
- **`cook-fingerprint`** owns the input-folding algorithm for probe-unit fingerprints. Reuses existing primitives (env contribution from CS-0036, declared tools from CS-0052) plus a new "upstream probe fingerprint" fold.
- **`cook-luaotp`** owns probe-unit execution on worker VMs. Adds a `produce`-invocation path that captures the Lua return value, validates serialisability, msgpack-encodes, and returns bytes.
- **`cook-cache`** is untouched. Probe-unit artifacts use the same `CacheBackend::put/get` path as recipe outputs. The artifact body is msgpack bytes; the meta sidecar carries SHA-256, same as any file artifact.
- **`cook_cc`** is the first migration target. `init()` compiler detection becomes a probe; pkg-config and cmake-compat finders become probes.

## 4. Lua API surface

### 4.1 `cook.probe(key, opts)`

**Signature:** `cook.probe(key: string, opts: ProbeOpts)`

```lua
cook.probe("cc:zlib", {
  inputs = {
    env      = { "PKG_CONFIG_PATH", "PATH" },
    tools    = { "pkg-config" },
    files    = {},
    requires = { "cc:compiler" },   -- chained probes
  },
  produce = function()
    -- function runs at execute-time on a worker VM, on cache miss
    local r = run_pkg_config("zlib")
    return {
      found  = r.found,
      cflags = r.cflags,    -- table-of-strings
      libs   = r.libs,
    }
  end,
})
```

**Constraints (Standard normative):**

- `key` MUST be a string. Convention: `"<module-prefix>:<name>"` (e.g. `cc:zlib`). The Standard does not enforce the prefix convention but registers the recommendation.
- `key` MUST be unique within one register pass. Duplicate → hard error: `probe key '<key>' declared at <file:line>; previously declared at <file:line>`.
- `opts.inputs` is a table with optional sub-keys `env`, `tools`, `files`, `requires`. Each is a list of strings. Omitted sub-keys default to empty.
- `opts.produce` is a Lua function taking no arguments. MUST return a serialisable value (see §4.3). MAY call any both-phase Lua API (`cook.sh`, `cook.env`, `cook.cache.get` for upstream probe reads, `fs.*`, `path.*`).
- `cook.probe` itself is **register-only**. Calling from execute-phase Lua raises the §6.3.2 register-only-API error (consistent with `cook.add_unit`, etc.).

### 4.2 `requires` on `cook.add_unit`

The existing `cook.add_unit(...)` table grows a new optional field:

```lua
cook.add_unit({
  name     = "myapp.o",
  inputs   = { "myapp.c" },
  outputs  = { "build/myapp.o" },
  requires = { "cc:zlib", "cc:compiler" },   -- NEW
  command  = "{{cc:compiler}} -c {{in}} -o {{out}} {{cc:zlib.cflags}}",
})
```

**Constraints:**

- `requires` is an optional list of probe-key strings.
- At register-time, each key MUST resolve to a registered probe; otherwise hard error: `unit 'myapp.o' requires probe key '<key>' which was not declared`. Order of `cook.probe` / `cook.add_unit` calls is unconstrained — resolution happens after the register pass completes.
- Each resolved probe contributes a DAG edge (probe → consumer).
- Each resolved probe contributes its fingerprint into the consumer's fingerprint (transitive invalidation).

### 4.3 Value-serialisation contract

A probe's return value MUST be one of:

- `nil`
- `bool`
- `number` (integer or float; preserved through msgpack)
- `string` (raw bytes; not constrained to UTF-8)
- A table whose keys are all strings or all integers (1..N, no holes — array shape) and whose values are recursively of these types.

Non-serialisable values (`function`, `userdata`, `thread`, mixed-key tables, tables with cycles) MUST cause the probe to fail with a diagnostic naming the offending path (`probe 'cc:zlib' returned a non-serialisable value at .cflags[3]`).

### 4.4 `cook.cache.get(key)` (retained, semantics tightened)

Within execute-phase Lua (recipe bodies, probe `produce` functions, desugared command-Lua), `cook.cache.get(key)` returns:

- The deserialised probe value if `key` resolves to a probe and the probe has completed.
- `nil` if `key` doesn't resolve, or if the caller hasn't declared `requires = { key, … }` on its containing unit.

The "didn't declare a require" case is detectable because the scheduler tracks per-unit `requires` lists. A `cook.cache.get` for an undeclared key returns `nil`; a future revision MAY upgrade this to a hard error. For now it's a soft contract — the desugared command-Lua always declares its requires correctly because codegen synthesises both sides.

### 4.5 Deprecated: `cook.cache.set`, `cook.cache.invalidate`, `cook.cache.clear`

These three are deprecated in the new §. Implementations SHOULD warn on call. Removed at v1.x (see §10 for the migration window).

## 5. Fingerprinting

### 5.1 Probe-unit fingerprint

A probe unit's fingerprint is computed from:

1. **Key** (string).
2. **Produce function source** — hashed via the existing `hash_str` (xxh3_64) over the function's Lua source text.
3. **Env inputs** — for each listed env-var name, fold `(name, current_value_or_empty)` into the hash. Reuses CS-0036 env-contribution machinery.
4. **Tool inputs** — for each listed tool name, resolve via PATH at probe execution time; fold (name, resolved-path-content-hash). Reuses CS-0052 declared-tools machinery. If the tool isn't on PATH, fold `(name, "<missing>")` — probe runs and presumably fails, but the fingerprint is well-defined.
5. **File inputs** — for each listed file path (relative to the Cookfile root), fold (path, content-hash). Missing file folds as `(path, "<missing>")`.
6. **Upstream probes** — for each `requires` key, fold the upstream probe's full fingerprint. Cycle detection: if A requires B and B requires A, hard error at register-time.

Folding order is **deterministic**: keys sorted within each input category; categories in the fixed order above. SHA-256 over the canonicalised representation. Output is a 32-byte fingerprint that addresses the artifact in `CacheBackend`.

### 5.2 Consumer-unit fingerprint extension

A regular unit's fingerprint today folds: command text, input file hashes, declared tools, env contribution. With probe-unit support it additionally folds, for each `requires` key in sorted order: the upstream probe's fingerprint. This means **changes to a probe input transitively invalidate consumer artifacts**, which is the correctness property we want.

## 6. Storage and cloud sharing

### 6.1 On-disk layout

No new directories. Probe-unit artifacts live in the existing recipe cache:

```
.cook/cache/<32-byte-fingerprint-hex>.bin       -- msgpack bytes (probe value)
.cook/cache/<32-byte-fingerprint-hex>.meta.json -- { content_hash, schema_version, … }
```

The meta sidecar's existing fields (CS-0048 schema_version, CS-0054 SHA-256 content_hash, CS-0055 idempotency) apply unchanged. A new `kind: "probe_value"` field disambiguates probe-artifact sidecars from file-artifact sidecars for tooling that wants to introspect.

### 6.2 Cloud R2 sync

Cook Cloud's CS-0058/CS-0059 wire protocol uploads artifacts by fingerprint. Probe-unit artifacts ride the same protocol unchanged — the artifact body is msgpack bytes, treated as opaque content by the wire. Authentication, deviceAuth, organization, and billing surfaces are unchanged.

**The killer feature:** a developer's first `cook build` on a fresh checkout downloads probe artifacts that CI (or a teammate) already populated, instead of re-running pkg-config / cmake / compiler-probe subprocesses. The configure step that traditional build systems make machine-local becomes shareable.

### 6.3 Schema versioning

Probe-artifact msgpack format is `schema_version = 1` at v1.0 cut. Future revisions follow CS-0048's read/write/evolution policy. The msgpack envelope is:

```
{
  schema_version: 1,
  produced_at:    <RFC3339 timestamp>,  -- informative, not fingerprinted
  value:          <user-returned value>
}
```

## 7. Standard amendments

### 7.1 New § (target: §9.3 in current numbering, or wherever the v0.10 reorg places "Module Lua API")

**Title:** `cook.probe` and probe units.

**Subsections:**
- §9.3.1 — Surface: `cook.probe(key, opts)` signature, key constraints, idempotency.
- §9.3.2 — Inputs: the four input categories (env, tools, files, requires), fingerprint contributions per category.
- §9.3.3 — Value types: the serialisable subset and the diagnostic shape for non-serialisable returns.
- §9.3.4 — Execution semantics: probe units run in the execute phase, at most once per build, cached across invocations via the recipe cache.
- §9.3.5 — Consumer requires: `cook.add_unit({ requires = … })`, unresolved-key diagnostic, DAG-edge contribution.
- §9.3.6 — Command-template desugaring: `{{key}}` and `{{key.field}}` substitution semantics.
- §9.3.7 — Failure semantics: structured "not found" vs raised errors; cycle detection on `requires` chains.

### 7.2 §6.3.4 (cook.cache surface)

Amend:

- `cook.cache.set`, `cook.cache.invalidate`, `cook.cache.clear` marked deprecated; warning required; removed at v1.x.
- `cook.cache.get` retained; semantics tightened to "reads from probe-unit cache store" with forward-reference to §9.3.4.

### 7.3 §8.6 (cache model)

Cross-reference probe units as cache participants. The existing recipe-cache mechanism applies to probe units unchanged.

### 7.4 §12 (modules)

Amend the line allowing in-memory-only execute-phase cache — replace with a forward-reference to §9.3. The execute-phase Lua `cook.cache.get` is now backed by the probe-unit store, populated by the scheduler during execute phase before consumer units run.

### 7.5 Anticipated CS-numbers (placeholders)

- **CS-AA:** Probe units and the `cook.probe` Lua surface.
- **CS-BB:** Probe-unit fingerprint inputs and CacheBackend integration.
- **CS-CC:** `cook.cache.set` deprecation and `cook.cache.get` semantic tightening.

Exact numbers assigned at PR time.

## 8. Migration

### 8.1 Phase 1 — Infra + Standard (1 PR series)

- Land the new Standard §, §6.3.4 amendment, §8.6 cross-reference, §12 amendment, three CS- entries.
- Land engine changes: `cook-register` `ProbeUnit` variant, `cook-luagen` template desugaring, `cook-luaotp` `produce` invocation path, `cook-fingerprint` probe-input folding, `cook-cache` `kind: "probe_value"` meta field.
- Land conformance fixtures: 8–12 fixtures covering parse, register-positive, fingerprint determinism, value-serialisation contract, requires-resolution, duplicate-key error, cycle detection.
- No callers yet. `cook.cache.set/get` continues to work; warning on `set` is **not** emitted yet (until cook_cc migrates off it).

### 8.2 Phase 2 — cook_cc compiler detection (cook_cc 0.5.0)

- `cook_cc/init.lua` rewrites `init()`'s compiler detection as `cook.probe("cc:compiler", …)`.
- `cc.find` and friends declare `requires = { "cc:compiler" }` on their downstream units (via the `cc.bin`/`cc.lib` target-makers).
- Existing tests pin behavioural equivalence: same compiler triple resolved, same find-strategy chain ordering.
- One example migrated: `examples/raylib-game/`.

### 8.3 Phase 3 — cook_cc pkg-config + cmake-compat finders (cook_cc 0.6.0)

- pkg-config finder becomes `cook.probe("cc:pkgconfig:<name>", …)` — one probe per requested library, registered eagerly on `cc.find` call.
- cmake-compat finder becomes `cook.probe("cc:cmake:<name>", …)`.
- `cc.find` collapses to a thin dispatcher: register the appropriate probes, declare requires on the calling target's downstream units.
- All examples migrated: `examples/raylib-game/`, `examples/sdl3-game/`.
- gate-m1 + gate-m2 CI verified to still build cleanly end-to-end.

### 8.4 Phase 4 — Deprecation and removal (post-Doom-3 milestones)

- Emit warning on `cook.cache.set` calls (one-shot per key per run, structured diagnostic with file:line).
- Standard major-version bump (target: v1.0 cut) removes `cook.cache.set/invalidate/clear` from the surface entirely. `cook.cache.get` remains.

## 9. Open questions (deferred)

These are flagged as future-work, not blockers for the Phase-1 PR:

1. **`commands` input category.** The "git config user.name as a probe input" case. Adds fingerprint-input recursion (the probe's input is itself a subprocess output). Probably useful eventually; not needed for cc:* probes.
2. **Probe-aware DAG viewer.** Showing probe values inline in the `cook --dag` HTML output. Nice UX; not load-bearing.
3. **Probe inspection CLI.** `cook probes` to list registered probes, `cook probe show cc:zlib` to print the cached value. Debug aid; not load-bearing.
4. **Probe-result diffing.** If a probe fingerprint changes and the produced value differs from the previous value, optionally surface a diff. Useful for "why did my build break after CI moved." Future polish.
5. **Probe artifact GC.** Probe artifacts accumulate in `.cook/cache/`. The existing recipe-cache GC strategy (TBD generally) applies; nothing probe-specific yet.

## 10. Linear ticketing

The original SHI-214 ("Disk-persistent cook.cache") is too narrow for this design. Recommended re-filing:

1. **Close SHI-214** with a comment pointing at this design doc and the new epic.
2. **File a new epic** under a fresh Linear project (sibling to the existing "Cook worker pool — persistent + affinity-aware" and "Cook persistent linker" projects): **"Probe units — shareable configure-time state."**
   - Phase-1 ticket: Engine + Standard (this design's §8.1 scope).
   - Phase-2 ticket: cook_cc 0.5.0 — compiler-detection probe migration.
   - Phase-3 ticket: cook_cc 0.6.0 — pkg-config + cmake-compat probe migration.
   - Phase-4 ticket: `cook.cache.set` deprecation warning + v1.x removal.
   - Future-work tickets for the §9 deferred items as they come up.

The Cook-builds-Doom-3 project (SHI-132) is **not** the right parent — Doom 3 is an integration target, not a feature project. Probe units are a feature in their own right that Doom 3 benefits from but doesn't drive.

## 11. References

- Brainstorm conversation: 2026-05-14 to 2026-05-15.
- Existing register-phase `cook.cache` impl: `cli/crates/cook-register/src/module_cache.rs`.
- Existing execute-phase `cook.cache` (CS-0070): `cli/crates/cook-luaotp/src/pool.rs:611` `install_execute_phase_cook_cache`.
- Recipe-cache machinery: `cli/crates/cook-cache/` (whole crate); `cli/crates/cook-fingerprint/` (fingerprint primitives).
- Cloud wire protocol: CS-0058, CS-0059 (Cook Cloud v1 design).
- Adjacent Lua surface left untouched: `cook.export/import` (CS-0071) — `cli/crates/cook-register/src/export_api.rs`.
- Standard sections to amend: §6.3.4, §8.6, §12. New §9.3.
- Original SHI-214: <https://linear.app/shiny-guru/issue/SHI-214>.
- Parent context: <https://linear.app/shiny-guru/issue/SHI-132> (Doom 3 epic).
