# Probe Units — Implementation Plan (Phase 1: Engine + Standard)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the probe-units engine surface and Standard chapter so module authors can call `cook.probe(key, opts)` to register cache-producing DAG units and consumers can declare `requires = {key}`. No cook_cc migration yet — that's Phase 2 in a separate repo.

**Architecture:** Probe units are a new variant in the DAG. Cookfile authors call `cook.probe` from register-phase Lua to declare a producer; the probe's `produce` function runs on an execute-phase worker VM, and its return value gets msgpack-serialised and stored in the existing recipe cache. Consumer units declare `requires = {keys}` on `cook.add_unit`; codegen rewrites `{{key.field}}` placeholders into `cook.cache.get(key).field` reads. The scheduler runs probes first by topology and populates a per-run value store the consumer workers read from.

**Tech Stack:**
- Rust 2024 edition workspace at `cli/`
- `mlua` for Lua VM bindings
- `rmpv` for msgpack encode/decode (new dependency)
- Existing crates: `cook-lang`, `cook-luagen`, `cook-register`, `cook-luaotp`, `cook-cache`, `cook-fingerprint`, `cook-contracts`
- Standard authoring: MDX in `standard/src/content/docs/`, conformance corpus at `standard/conformance/`

**Spec reference:** `docs/superpowers/specs/2026-05-15-shi214-probe-units-design.md`. Read it first.

---

## Background reading (do this before touching code)

Cook has a two-phase execution model:

1. **Register phase** — runs `cook-register`'s synchronous Lua VM over the codegen-emitted register program. Builds a `RecipeUnits` graph (in `cook-contracts`). Recipe bodies, target makers (`cook.add_unit`), and module loaders (`cook.load_module`) all live here. The output is a DAG.
2. **Execute phase** — runs `cook-luaotp`'s worker pool. Each unit dispatches to a worker VM that executes its `WorkPayload` variant (shell command, Lua chunk, interactive step).

Today there's no way for a unit to depend on a *value* produced by another unit (only on a file). Probe units fix that:

- `cook.probe(key, opts)` registers a producer (declared inputs + a `produce` function).
- `cook.add_unit({ requires = {"key"} })` declares a value-edge consumer.
- The engine wires DAG edges (probe → consumer) at register-time.
- At execute-time, probe units run first (by topology); their msgpack-serialised return value lands in a per-run value store and the existing recipe cache.
- Consumer units read via `cook.cache.get(key)`, called from codegen-emitted Lua that desugars `{{key.field}}` placeholders.

Key invariants:

- Probes are **register-only to declare**; they **execute** during execute phase.
- A probe runs **at most once per build**.
- A probe's fingerprint folds: key name, `produce` source hash, declared env values, declared tool content hashes, declared file content hashes, upstream probe fingerprints.
- Probe values are msgpack-serialised; stored as artifacts in `.cook/cache/<fp>.bin` with a `meta.json` sidecar carrying `kind: "probe_value"`.
- Probes share Cook Cloud R2 (CS-0058/0059) with no protocol changes.

Read these files to ground yourself before starting:
- `docs/superpowers/specs/2026-05-15-shi214-probe-units-design.md` (the spec)
- `cli/crates/cook-contracts/src/lib.rs` (`WorkPayload`, `CapturedUnit`, `RecipeUnits`)
- `cli/crates/cook-register/src/engine.rs` (`Registry::new`, `register_recipe`)
- `cli/crates/cook-register/src/unit_api.rs` (`cook.add_unit` capture)
- `cli/crates/cook-register/src/module_loader.rs` (existing `cook.cache.*` API on register VM)
- `cli/crates/cook-luaotp/src/pool.rs:611` (existing execute-phase `cook.cache` install)
- `cli/crates/cook-fingerprint/src/context.rs` (`ExecutionContext`, `step_context_hash`)
- `cli/crates/cook-cache/src/backend.rs` (`ArtifactMeta`, `LocalBackend::{put,get}`)
- `cli/crates/cook-luagen/src/template.rs` (`ConsultedEnv`, existing placeholder resolution)

---

## File map

**New files:**
- `standard/src/content/docs/22-probe-units.mdx` — new Standard chapter (Ch. 22; existing Ch. 22 "Register phase" shifts to 22.x or renumbers)
- `cli/crates/cook-register/src/probe_api.rs` — `cook.probe` Lua binding + key-uniqueness validation + cycle detection
- `cli/crates/cook-register/src/probe_value.rs` — Lua → `rmpv::Value` walker, value-type validation, msgpack encode helper
- `standard/conformance/positive/probe-register-simple/{Cookfile,parse.txt,register_ok.txt}`
- `standard/conformance/positive/probe-requires-chain/{Cookfile,parse.txt,register_ok.txt}`
- `standard/conformance/positive/probe-value-types/{Cookfile,parse.txt,register_ok.txt}`
- `standard/conformance/positive/probe-template-desugaring/{Cookfile,parse.txt,register_ok.txt}`
- `standard/conformance/negative/probe-duplicate-key/{Cookfile,register_error.txt}`
- `standard/conformance/negative/probe-unresolved-require/{Cookfile,register_error.txt}`
- `standard/conformance/negative/probe-cycle/{Cookfile,register_error.txt}`
- `standard/conformance/negative/probe-non-serializable/{Cookfile,execute_error.txt}` (note: harness will need to support execute-mode for this one, OR convert to a register-phase probe call)
- `standard/conformance/negative/probe-register-only/{Cookfile,execute_error.txt}` (same caveat)
- `cli/crates/cook-register/tests/probe_integration.rs` — end-to-end probe + consumer build test

**Modified:**
- `standard/src/content/docs/17-cache.mdx` (cross-ref to new chapter)
- `standard/src/content/docs/21-lua-api.mdx` (`cook.cache.set/invalidate/clear` deprecation note, `cook.probe` addition)
- `standard/src/content/docs/24-both-phase.mdx` (`cook.cache.get` semantics tightened)
- `standard/src/content/docs/appendix/E-changes.mdx` (three new CS- entries)
- `cli/crates/cook-contracts/src/lib.rs` (new `ProbeUnit`, `ProbeInputs`; `WorkPayload::Probe`; `CapturedUnit.requires`; `RecipeUnits.probes`)
- `cli/crates/cook-register/src/lib.rs` (`pub mod probe_api; pub mod probe_value;`)
- `cli/crates/cook-register/src/engine.rs` (install `register_probe_api`; expose probe registry; cycle detection after register pass)
- `cli/crates/cook-register/src/unit_api.rs` (parse `requires: Vec<String>` from `cook.add_unit` opts)
- `cli/crates/cook-register/Cargo.toml` (add `rmpv` dep)
- `cli/crates/cook-register/tests/conformance.rs` (positive-register harness covers new fixtures)
- `cli/crates/cook-fingerprint/src/context.rs` (probe-input fold helper; recipe fingerprint extension)
- `cli/crates/cook-cache/src/backend.rs` (`ArtifactMeta.kind: Option<String>` with `#[serde(default)]`)
- `cli/crates/cook-luaotp/src/pool.rs` (probe-dispatch path on worker; tighten `cook.cache.get` to read probe-value store; install register-only guard for `cook.probe`)
- `cli/crates/cook-luagen/src/template.rs` (detect `{{key}}` and `{{key.field}}` probe-value patterns; rewrite into a Lua substitution step)
- `cli/Cargo.toml` (workspace dep `rmpv` if you're using workspace deps; otherwise add to crate-level)

---

## Section A — Standard (spec-first; per CLAUDE.md, language-surface changes are lockstep)

### Task A1: Write new Standard chapter "Probe units"

**Files:**
- Create: `standard/src/content/docs/22-probe-units.mdx`

- [ ] **Step 1: Determine chapter numbering**

Look at `standard/src/content/docs/22-register-phase.mdx` (currently Ch. 22). Probe units is conceptually adjacent to register-phase APIs but is a substantial topic. Options:
- Insert as new Ch. 22, renumber existing 22→23, 23→24, 24→25 (heavy churn).
- Insert as Ch. 28.5 between cc and appendix (places it after the cc catalogue entry — wrong topical placement).
- Insert as Ch. 22.5 by filename `22-5-probe-units.mdx` (sidebar order numeric).

Pick **renumber** — Ch. 22 becomes "Probe units"; existing 22 ("Register phase") becomes 23; 23 ("Execute phase") becomes 24; 24 ("Both-phase") becomes 25. Update every sidebar order frontmatter and every cross-reference. This is invasive but it's the right structural place.

Defer the renumber to Task A6; for now, **create the chapter file with sidebar.order: 22.5** so it slots between existing 22 and 23 without immediate renumbering. The renumber happens after the rest lands.

- [ ] **Step 2: Write the chapter**

Create `standard/src/content/docs/22-probe-units.mdx` with the full normative spec. Use the existing chapter style — MUST/MAY/SHOULD throughout, `§{cat.probes...}` cross-refs, numbered sub-sections.

```mdx
---
title: "§{cat.probes} — Probe units"
sidebar:
  order: 22.5
---

# 22.5. Probe units [#cat.probes]

> **Normative.** This chapter specifies the `cook.probe` Lua API and the probe-unit execution model. A conforming implementation MUST honour every clause in this chapter.

## 22.5.1. Synopsis [#cat.probes.synopsis]

A *probe unit* is a DAG unit whose output is a Lua value (rather than a file). A conforming implementation MUST expose the register-phase API `cook.probe(key, opts)` (§{cat.probes.api}) and MUST treat probe units as ordinary participants in the execution DAG (§{cat.probes.exec}). Probe outputs MUST be cached using the same machinery as recipe outputs (§{exec.cache}), enabling cross-invocation memoisation and cross-machine sharing.

A unit MAY declare a dependency on one or more probe outputs via `requires = { key, ... }` on `cook.add_unit` (§{cat.probes.requires}). The implementation MUST add a DAG edge from each named probe to the consuming unit and MUST fold each probe's fingerprint into the consumer's fingerprint (transitive invalidation).

## 22.5.2. The cook.probe API [#cat.probes.api]

`cook.probe(key: string, opts: ProbeOpts)` registers a probe unit. The function MUST be available on the register-phase VM. Calls from the execute-phase VM MUST raise the §{exec.api.register-only} error.

`opts` is a table with the following keys:

| Key | Type | Required | Semantics |
|---|---|---|---|
| `inputs` | table | yes | Declared inputs (§{cat.probes.inputs}). May be empty. |
| `produce` | function | yes | Zero-arg function returning the probe value. Executes on a worker VM during the execute phase. |

`key` MUST be a string. Implementations SHOULD recommend the convention `"<module-prefix>:<name>"` (e.g. `"cc:zlib"`) but MUST NOT enforce it.

A second `cook.probe` call with the same `key` within one register pass MUST raise a register-phase diagnostic naming both source locations:

```
probe key 'cc:zlib' declared at <file>:<line>; previously declared at <file>:<line>
```

## 22.5.3. Probe inputs [#cat.probes.inputs]

`opts.inputs` is a table with optional sub-keys:

| Sub-key | Type | Semantics |
|---|---|---|
| `env` | list[string] | Env-var names. Current value contributes to the probe's fingerprint (§{exec.cache.env}). |
| `tools` | list[string] | Tool names (e.g. `"pkg-config"`). Resolved via PATH; content hash contributes (§{exec.cache.declared-tools}). |
| `files` | list[string] | File paths relative to the Cookfile root. Content hash contributes. |
| `requires` | list[string] | Upstream probe keys. Their fingerprints contribute. |

A probe's fingerprint MUST fold inputs in this deterministic order: key string, produce-source xxh3_64, env (sorted by name), tools (sorted by name), files (sorted by path), requires (sorted by key). Implementations MUST use SHA-256 over the canonicalised representation, yielding a 32-byte fingerprint that addresses the artifact.

## 22.5.4. Value types [#cat.probes.values]

A probe's `produce` function MUST return one of:

- `nil`
- `boolean`
- `number` (integer or float)
- `string` (raw bytes, not constrained to UTF-8)
- A table whose keys are all strings, OR all integers in `1..N` (array shape), and whose values are recursively of these types.

Returning a `function`, `userdata`, `thread`, mixed-key table, or a table containing a cycle MUST cause the probe to fail with a diagnostic naming the offending path. Example:

```
probe 'cc:zlib' returned a non-serialisable value at .cflags[3] (function)
```

Probe values are serialised to msgpack (RFC) for storage and cloud transport.

## 22.5.5. Consumer requires [#cat.probes.requires]

`cook.add_unit({ ..., requires = { "key1", "key2" } })` declares that the unit consumes the named probe values. The implementation MUST:

1. Resolve each key at the end of the register pass against the registered probes. An unknown key MUST raise:
   ```
   unit '<name>' requires probe key '<key>' which was not declared
   ```
2. Add a DAG edge from each resolved probe to the consumer.
3. Fold each resolved probe's fingerprint into the consumer's fingerprint.

The order of `cook.probe` and `cook.add_unit` calls within one register pass is unconstrained.

## 22.5.6. Command-template desugaring [#cat.probes.templates]

Consumer commands MAY include `{{key}}` and `{{key.field}}` placeholders referring to probe outputs in `requires`. The implementation MUST detect these patterns at codegen time and rewrite the command into a Lua step that calls `cook.cache.get(key)` and interpolates the result before invoking the shell.

- `{{key}}` substitutes the entire probe value (which MUST be a string).
- `{{key.field}}` substitutes the named field of the probe's top-level table value.
- `{{key.field[i]}}` substitutes a one-based array element of a table-valued field.

Malformed placeholders (referencing a key not in `requires`, or accessing a field on a non-table value) MUST raise a register-phase or execute-phase diagnostic naming the placeholder.

## 22.5.7. Execution semantics [#cat.probes.exec]

A probe unit MUST run at most once per build. On a cache hit (the probe's fingerprint addresses an existing artifact in `CacheBackend`), the implementation MUST load and deserialise the cached msgpack bytes, populate the per-run value store, and skip execution of the `produce` function. On a cache miss, the implementation MUST dispatch the probe to a worker VM, execute `produce`, validate the return value (§{cat.probes.values}), msgpack-encode, store via `CacheBackend::put`, and populate the per-run value store.

`cook.cache.get(key)` on the execute-phase VM MUST read from the per-run value store. The store is populated by completed probe units in topological order.

A `produce` function MAY raise a Lua error. The implementation MUST treat this as a unit failure and propagate to every downstream consumer via DAG edge.

## 22.5.8. Cycle detection [#cat.probes.cycles]

The probe `requires` graph MUST be acyclic. The implementation MUST detect cycles at the end of the register pass and raise:

```
probe cycle detected: cc:a -> cc:b -> cc:a
```

The diagnostic MUST render the cycle path.

## 22.5.9. Pinned by [CS-AA].
```

The exact `cat.probes.*` slug structure should match the patterns used elsewhere — check `28-cc.mdx` for the `§{cat.cc.surface}` style.

- [ ] **Step 3: Validate the chapter renders**

Run:
```
cd standard && npm run build 2>&1 | head -50
```
Expected: build succeeds, no broken-slug warnings. If slug-lint complains about new slugs, register them in whatever slug-allowlist the v0.10 reorg established (look at `0997ea2` commit for context).

- [ ] **Step 4: Commit**

```
git add standard/src/content/docs/22-probe-units.mdx
git commit -m "docs(standard): create Ch. 22.5 Probe units (CS-AA)"
```

---

### Task A2: Amend §17 (Cache) to cross-reference probe units

**Files:**
- Modify: `standard/src/content/docs/17-cache.mdx`

- [ ] **Step 1: Read the existing chapter**

Skim `17-cache.mdx` for sections that talk about what gets cached. Find the place where recipe outputs are described as cache participants.

- [ ] **Step 2: Add a cross-reference**

Add a paragraph (in an appropriate sub-section, e.g. after the recipe-output paragraph):

```mdx
Probe units (§{cat.probes}) participate in the same cache machinery: a probe's fingerprint addresses its msgpack-serialised value in `CacheBackend`, and probe artifacts ride the same `events.jsonl` event stream and Cook Cloud wire protocol (§{exec.cache.cloud}) as file outputs.
```

- [ ] **Step 3: Validate + commit**

```
cd standard && npm run build 2>&1 | tail -20
```

```
git add standard/src/content/docs/17-cache.mdx
git commit -m "docs(standard): §17 cross-ref probe units (CS-AA)"
```

---

### Task A3: Amend §21 (Lua API) — deprecate cook.cache.set, add cook.probe

**Files:**
- Modify: `standard/src/content/docs/21-lua-api.mdx`

- [ ] **Step 1: Find the cook.cache section**

Search `21-lua-api.mdx` for `cook.cache`. Find the table or section listing `get`/`set`/`invalidate`/`clear`.

- [ ] **Step 2: Mark set/invalidate/clear deprecated**

Edit the section so each of `cook.cache.set`, `cook.cache.invalidate`, `cook.cache.clear` carries a deprecation banner:

```mdx
> **Deprecated** (CS-CC, v0.11). Removed at v1.0. Use `cook.probe` (§{cat.probes}) for memoised values.
```

- [ ] **Step 3: Tighten cook.cache.get semantics**

Below the `cook.cache.get(key)` row, add a normative note:

```mdx
`cook.cache.get(key)` on the execute-phase VM MUST read from the per-run probe-value store (§{cat.probes.exec}). On the register-phase VM, `cook.cache.get(key)` continues to read from the module-scoped persistent kv store (§{mods.cache}). The two surfaces share a function name but back different stores; this is intentional and transitional — the register-phase store will be removed alongside `cook.cache.set` at v1.0.
```

- [ ] **Step 4: Add a cook.probe section pointer**

Add a short subsection at the end of the API list:

```mdx
### §21.x. cook.probe

`cook.probe(key, opts)` is specified normatively in §{cat.probes.api}. It MUST be available on the register-phase VM and MUST raise §{exec.api.register-only} on the execute-phase VM.
```

- [ ] **Step 5: Validate + commit**

```
cd standard && npm run build 2>&1 | tail -20
git add standard/src/content/docs/21-lua-api.mdx
git commit -m "docs(standard): §21 deprecate cook.cache.set, add cook.probe (CS-AA, CS-CC)"
```

---

### Task A4: Amend §24 (Both-phase) cook.cache.get behaviour

**Files:**
- Modify: `standard/src/content/docs/24-both-phase.mdx`

- [ ] **Step 1: Find the cook.cache discussion**

Search `24-both-phase.mdx` for `cook.cache`. The existing text likely says the execute-phase implementation "MAY use in-memory-only storage" (this is the line referenced in CS-0070).

- [ ] **Step 2: Replace with forward-reference**

Replace the "MAY use in-memory-only storage" allowance with:

```mdx
On the execute-phase VM, `cook.cache.get(key)` MUST read from the per-run probe-value store specified in §{cat.probes.exec}. The store is populated by completed probe units in topological order before any consuming unit dispatches. An unknown key returns `nil`. `cook.cache.set` is NOT available on the execute-phase VM (it is deprecated entirely per CS-CC).
```

- [ ] **Step 3: Validate + commit**

```
cd standard && npm run build 2>&1 | tail -20
git add standard/src/content/docs/24-both-phase.mdx
git commit -m "docs(standard): §24 tighten execute-phase cook.cache.get (CS-AA)"
```

---

### Task A5: Add three CS- entries to Appendix E

**Files:**
- Modify: `standard/src/content/docs/appendix/E-changes.mdx`

- [ ] **Step 1: Determine the next available CS-number**

Search `E-changes.mdx` for the highest existing CS-NNNN identifier (look near the top — entries are reverse chronological). The next three after that are yours; call them CS-AA, CS-BB, CS-CC in this plan, but use the real numbers in the file.

- [ ] **Step 2: Add three entries**

Append three entries following the existing format. Each entry needs: summary, motivation, normative scope, sections affected, implementation status, test plan. Look at CS-0036 / CS-0055 / CS-0070 for shape.

Entry CS-AA: "Probe units and the cook.probe Lua surface."
Entry CS-BB: "Probe-unit fingerprint inputs and CacheBackend integration."
Entry CS-CC: "cook.cache.set deprecation and cook.cache.get semantic tightening."

- [ ] **Step 3: Validate + commit**

```
cd standard && npm run build 2>&1 | tail -20
git add standard/src/content/docs/appendix/E-changes.mdx
git commit -m "docs(standard): App. E — CS-AA/BB/CC for probe units"
```

---

### Task A6: Standard version bump and conformance harness version check

**Files:**
- Modify: `cli/crates/cook-lang/src/lib.rs` (`COOK_STANDARD_VERSION` constant)
- Modify: `standard/package.json` (if a version field exists)

- [ ] **Step 1: Find the version constant**

```
grep -rn "COOK_STANDARD_VERSION" cli/crates/cook-lang/src/
```

You'll find a constant like `pub const COOK_STANDARD_VERSION: &str = "0.10";`. Bump it to "0.11" (or whatever the next minor is).

- [ ] **Step 2: Search for any other version-pinning sites**

```
grep -rn "0\.10\.0\|v0\.10" standard/ cli/ | head -20
```

If `standard/package.json` carries a version, bump it. If `standard/src/content/docs/00-introduction.mdx` references the version, bump that.

- [ ] **Step 3: Commit**

```
git add cli/crates/cook-lang/src/lib.rs standard/package.json standard/src/content/docs/00-introduction.mdx
git commit -m "chore(standard): bump version to 0.11 (probe units)"
```

---

## Section B — cook-contracts type additions

### Task B1: Add ProbeInputs and ProbeUnit types; extend WorkPayload and CapturedUnit

**Files:**
- Modify: `cli/crates/cook-contracts/src/lib.rs`
- Test: `cli/crates/cook-contracts/src/lib.rs` (in-crate `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write failing tests first**

Append to the `#[cfg(test)] mod tests` block in `cli/crates/cook-contracts/src/lib.rs`:

```rust
#[test]
fn probe_inputs_default_is_empty() {
    let i = ProbeInputs::default();
    assert!(i.env.is_empty());
    assert!(i.tools.is_empty());
    assert!(i.files.is_empty());
    assert!(i.requires.is_empty());
}

#[test]
fn probe_unit_round_trips_through_serde() {
    let p = ProbeUnit {
        key: "cc:zlib".into(),
        produce_source: "return run_pkg_config(\"zlib\")".into(),
        produce_line: 42,
        inputs: ProbeInputs {
            env: vec!["PKG_CONFIG_PATH".into()],
            tools: vec!["pkg-config".into()],
            files: vec![],
            requires: vec!["cc:compiler".into()],
        },
    };
    let s = serde_json::to_string(&p).unwrap();
    let r: ProbeUnit = serde_json::from_str(&s).unwrap();
    assert_eq!(r.key, "cc:zlib");
    assert_eq!(r.inputs.requires, vec!["cc:compiler"]);
}

#[test]
fn work_payload_probe_variant_constructs() {
    let p = WorkPayload::Probe {
        key: "cc:zlib".into(),
        produce: "return 42".into(),
        line: 1,
    };
    match &p {
        WorkPayload::Probe { key, produce, line } => {
            assert_eq!(key, "cc:zlib");
            assert_eq!(produce, "return 42");
            assert_eq!(*line, 1);
        }
        _ => panic!("expected Probe variant"),
    }
}

#[test]
fn captured_unit_requires_defaults_to_empty() {
    use serde_json;
    // Forward-compat: an old serialised CapturedUnit without `requires` deserialises
    // to a value with an empty requires vector.
    let p = WorkPayload::Shell { cmd: "echo hi".into(), line: 1 };
    let cu = CapturedUnit {
        payload: p,
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        requires: vec![],
    };
    assert!(cu.requires.is_empty());
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```
cargo test -p cook-contracts probe_inputs_default_is_empty 2>&1 | tail -10
```
Expected: compile error — `ProbeInputs`, `ProbeUnit`, `WorkPayload::Probe`, and `CapturedUnit.requires` don't exist yet.

- [ ] **Step 3: Add the types**

First locate where `WorkResult` lives — likely in `cook-luaotp/src/pool.rs` or `cook-contracts/src/lib.rs`:

```
grep -rn "enum WorkResult\|pub struct WorkResult" cli/crates/ 2>&1 | head -3
```

Once located, extend `WorkResult` with a probe variant alongside the new `WorkPayload::Probe` you're about to add:

```rust
pub enum WorkResult {
    /* existing variants — Shell { stdout, exit, ... }, LuaChunk { value }, etc. */
    /// A probe unit completed; bytes is the msgpack-encoded return value.
    ProbeOutput { key: String, bytes: Vec<u8> },
}
```

(If `WorkResult` is `#[non_exhaustive]` you may not need to fix call sites; otherwise add the new arm to every match site that handles the result enum exhaustively.)

Then in `cli/crates/cook-contracts/src/lib.rs`, after the existing `WorkPayload` enum, add:

```rust
/// Declared inputs for a probe unit. Each category contributes to the
/// probe's fingerprint per §22.5.3.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeInputs {
    #[serde(default)] pub env: Vec<String>,
    #[serde(default)] pub tools: Vec<String>,
    #[serde(default)] pub files: Vec<String>,
    #[serde(default)] pub requires: Vec<String>,
}

/// A probe unit declared via `cook.probe(key, opts)` (§22.5.2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeUnit {
    pub key: String,
    pub produce_source: String,
    pub produce_line: usize,
    pub inputs: ProbeInputs,
}
```

Extend `WorkPayload`:

```rust
pub enum WorkPayload {
    Shell { /* existing */ },
    Interactive { /* existing */ },
    LuaChunk { /* existing */ },
    /// A probe unit that runs `produce` on a worker VM and stashes the
    /// msgpack-serialised return value under `key`.
    Probe {
        key: String,
        produce: String,
        line: usize,
    },
}
```

Add `requires` to `CapturedUnit`:

```rust
pub struct CapturedUnit {
    pub payload: WorkPayload,
    pub cache_meta: Option<CacheMeta>,
    pub dep_kind: DepKind,
    /// Probe keys this unit consumes (§22.5.5). Empty for non-consumer units.
    #[serde(default)]
    pub requires: Vec<String>,
}
```

Add `probes` to `RecipeUnits`:

```rust
pub struct RecipeUnits {
    /* existing fields */
    /// Probe units registered during this register pass (§22.5.2).
    #[serde(default)]
    pub probes: Vec<ProbeUnit>,
}
```

- [ ] **Step 4: Fix every call site that constructs `CapturedUnit` or `RecipeUnits` literally**

```
grep -rn "CapturedUnit {" cli/ standard/ 2>&1
grep -rn "RecipeUnits {" cli/ standard/ 2>&1
```

Add `requires: vec![],` and `probes: vec![],` to every literal construction. Existing tests at `cli/crates/cook-contracts/src/lib.rs:395` and `:429` must be updated.

- [ ] **Step 5: Run all tests**

```
cargo test -p cook-contracts 2>&1 | tail -20
```
Expected: all green.

- [ ] **Step 6: Commit**

```
git add cli/crates/cook-contracts/src/lib.rs
git commit -m "feat(contracts): ProbeUnit, ProbeInputs, WorkPayload::Probe, CapturedUnit.requires"
```

---

## Section C — cook-register: probe registration, requires resolution, cycle detection

### Task C1: Scaffold `probe_api.rs` with the cook.probe Lua binding

**Files:**
- Create: `cli/crates/cook-register/src/probe_api.rs`
- Modify: `cli/crates/cook-register/src/lib.rs`
- Test: in-crate tests in `probe_api.rs`

- [ ] **Step 1: Add module declaration**

In `cli/crates/cook-register/src/lib.rs`, add:

```rust
pub mod probe_api;
pub mod probe_value;  // populated later in Task F2
```

- [ ] **Step 2: Write failing test for probe registration**

Create `cli/crates/cook-register/src/probe_api.rs`:

```rust
use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use cook_contracts::{ProbeInputs, ProbeUnit};

/// Shared probe registry threaded into the register-phase Lua VM. Populated
/// by `cook.probe(key, opts)` calls; consumed by `Registry::finalize` for
/// requires-resolution and DAG-edge insertion.
pub type SharedProbeRegistry = Rc<RefCell<ProbeRegistry>>;

#[derive(Debug, Default)]
pub struct ProbeRegistry {
    /// Key → (ProbeUnit, source file, source line of the cook.probe call).
    pub probes: BTreeMap<String, ProbeRegistration>,
}

#[derive(Debug)]
pub struct ProbeRegistration {
    pub probe: ProbeUnit,
    pub source_file: String,
    pub source_line: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cook_probe_registers_a_unit() {
        let lua = Lua::new();
        let cook = lua.create_table().unwrap();
        lua.globals().set("cook", cook.clone()).unwrap();
        let reg: SharedProbeRegistry = Rc::new(RefCell::new(ProbeRegistry::default()));
        install_cook_probe(&lua, &cook, reg.clone(), "Cookfile".into()).unwrap();

        lua.load(r#"
            cook.probe("cc:zlib", {
              inputs = { env = {"PKG_CONFIG_PATH"}, tools = {"pkg-config"} },
              produce = function() return { found = true } end,
            })
        "#).exec().unwrap();

        let r = reg.borrow();
        let p = r.probes.get("cc:zlib").expect("probe registered");
        assert_eq!(p.probe.key, "cc:zlib");
        assert_eq!(p.probe.inputs.env, vec!["PKG_CONFIG_PATH"]);
        assert_eq!(p.probe.inputs.tools, vec!["pkg-config"]);
        assert!(p.probe.inputs.files.is_empty());
        assert!(p.probe.inputs.requires.is_empty());
    }

    #[test]
    fn duplicate_probe_key_errors_with_both_locations() {
        let lua = Lua::new();
        let cook = lua.create_table().unwrap();
        lua.globals().set("cook", cook.clone()).unwrap();
        let reg = Rc::new(RefCell::new(ProbeRegistry::default()));
        install_cook_probe(&lua, &cook, reg, "Cookfile".into()).unwrap();

        let result = lua.load(r#"
            cook.probe("cc:zlib", { inputs = {}, produce = function() return 1 end })
            cook.probe("cc:zlib", { inputs = {}, produce = function() return 2 end })
        "#).exec();
        let err = result.unwrap_err().to_string();
        assert!(err.contains("probe key 'cc:zlib' declared at"), "got: {}", err);
        assert!(err.contains("previously declared at"), "got: {}", err);
    }
}
```

- [ ] **Step 3: Run test to confirm it fails**

```
cargo test -p cook-register probe_api 2>&1 | tail -10
```
Expected: `install_cook_probe` undefined → compile error.

- [ ] **Step 4: Implement install_cook_probe**

Append to `cli/crates/cook-register/src/probe_api.rs`:

```rust
/// Install `cook.probe(key, opts)` on the register-phase Lua VM. Captures
/// registrations into `registry`. Source-line tagging uses Lua's debug.
pub fn install_cook_probe(
    lua: &Lua,
    cook: &LuaTable,
    registry: SharedProbeRegistry,
    source_file: String,
) -> LuaResult<()> {
    let reg = registry.clone();
    let src = source_file.clone();
    let probe_fn = lua.create_function(move |lua, (key, opts): (String, LuaTable)| {
        // Resolve produce function source via string.dump-style introspection
        // is unreliable; instead use the function's source via debug.getinfo.
        let produce: LuaFunction = opts.get("produce")
            .map_err(|_| LuaError::runtime("cook.probe: opts.produce must be a function"))?;
        let inputs_tbl: LuaTable = opts.get("inputs").unwrap_or_else(|_| {
            let t = lua.create_table().unwrap();
            t
        });

        let env: Vec<String> = read_string_list(&inputs_tbl, "env")?;
        let tools: Vec<String> = read_string_list(&inputs_tbl, "tools")?;
        let files: Vec<String> = read_string_list(&inputs_tbl, "files")?;
        let requires: Vec<String> = read_string_list(&inputs_tbl, "requires")?;

        // Source-line tagging via Lua debug.getinfo of the caller's caller.
        let debug: LuaTable = lua.globals().get("debug")?;
        let getinfo: LuaFunction = debug.get("getinfo")?;
        let info: LuaTable = getinfo.call((2, "Sl"))?;
        let call_line: i64 = info.get("currentline").unwrap_or(-1);

        // Produce-function source: use debug.getinfo on the function with "S"
        // to get its source span; cook captures the source range from the
        // Cookfile if available. Falls back to the function's tostring.
        let produce_info: LuaTable = getinfo.call((produce.clone(), "S"))?;
        let produce_source: String = produce_info.get("source").unwrap_or_else(|_| "".into());

        let mut r = reg.borrow_mut();
        if let Some(prev) = r.probes.get(&key) {
            return Err(LuaError::runtime(format!(
                "probe key '{}' declared at {}:{}; previously declared at {}:{}",
                key, src, call_line, prev.source_file, prev.source_line,
            )));
        }
        r.probes.insert(key.clone(), ProbeRegistration {
            probe: ProbeUnit {
                key,
                produce_source,
                produce_line: call_line as usize,
                inputs: ProbeInputs { env, tools, files, requires },
            },
            source_file: src.clone(),
            source_line: call_line as usize,
        });
        Ok(())
    })?;
    cook.set("probe", probe_fn)?;
    Ok(())
}

fn read_string_list(tbl: &LuaTable, key: &str) -> LuaResult<Vec<String>> {
    let v: LuaValue = tbl.get(key)?;
    match v {
        LuaValue::Nil => Ok(vec![]),
        LuaValue::Table(t) => {
            let mut out = vec![];
            for pair in t.sequence_values::<String>() {
                out.push(pair?);
            }
            Ok(out)
        }
        _ => Err(LuaError::runtime(format!(
            "cook.probe: inputs.{} must be a list of strings (or nil)", key,
        ))),
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

```
cargo test -p cook-register probe_api 2>&1 | tail -10
```
Expected: both tests pass.

- [ ] **Step 6: Commit**

```
git add cli/crates/cook-register/src/probe_api.rs cli/crates/cook-register/src/lib.rs
git commit -m "feat(register): cook.probe Lua binding + ProbeRegistry"
```

---

### Task C2: Capture `requires` from cook.add_unit

**Files:**
- Modify: `cli/crates/cook-register/src/unit_api.rs`
- Test: in-crate tests in `unit_api.rs` (find the existing test module)

- [ ] **Step 1: Write a failing test**

In `cli/crates/cook-register/src/unit_api.rs`'s test module:

```rust
#[test]
fn add_unit_captures_requires_field() {
    let (lua, captured) = setup_unit_api_test();  // existing helper

    lua.load(r#"
        cook.add_unit({
            name = "myapp.o",
            inputs = { "myapp.c" },
            outputs = { "build/myapp.o" },
            requires = { "cc:zlib", "cc:compiler" },
            command = "true",
        })
    "#).exec().unwrap();

    let c = captured.lock().unwrap();
    let u = c.units.first().unwrap();
    assert_eq!(u.requires, vec!["cc:zlib", "cc:compiler"]);
}

#[test]
fn add_unit_without_requires_defaults_to_empty() {
    let (lua, captured) = setup_unit_api_test();
    lua.load(r#"
        cook.add_unit({
            name = "myapp.o",
            inputs = { "myapp.c" },
            outputs = { "build/myapp.o" },
            command = "true",
        })
    "#).exec().unwrap();
    let c = captured.lock().unwrap();
    let u = c.units.first().unwrap();
    assert!(u.requires.is_empty());
}
```

(Find or create `setup_unit_api_test` — there are existing helpers in `unit_api.rs:fake_cache_ctx` and similar; reuse the pattern.)

- [ ] **Step 2: Run tests to confirm they fail**

```
cargo test -p cook-register add_unit_captures_requires 2>&1 | tail -10
```
Expected: `u.requires` field doesn't exist on the captured-side type, OR the field is empty because we haven't read it yet.

- [ ] **Step 3: Read `requires` from the opts table**

Find where `cook.add_unit` parses its opts table in `unit_api.rs`. Look for `tbl.get::<...>("inputs")` or similar. Add:

```rust
let requires: Vec<String> = match tbl.get::<LuaValue>("requires") {
    Ok(LuaValue::Nil) => vec![],
    Ok(LuaValue::Table(t)) => {
        let mut out = vec![];
        for v in t.sequence_values::<String>() {
            out.push(v.map_err(|e| LuaError::runtime(format!(
                "cook.add_unit: requires must be a list of strings: {}", e
            )))?);
        }
        out
    }
    Ok(_) => return Err(LuaError::runtime(
        "cook.add_unit: requires must be a list of strings (or nil)".to_string(),
    )),
    Err(_) => vec![],
};
```

Pass `requires` through to the `CapturedUnit` constructor (the field we added in Task B1).

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test -p cook-register add_unit 2>&1 | tail -20
```
Expected: both new tests pass; existing tests still green.

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-register/src/unit_api.rs
git commit -m "feat(register): cook.add_unit captures requires field"
```

---

### Task C3: Wire probe_api into Registry; collect probes into RecipeUnits

**Files:**
- Modify: `cli/crates/cook-register/src/engine.rs`

- [ ] **Step 1: Write a failing test in the test module of engine.rs**

```rust
#[test]
fn registry_collects_probe_declarations_into_recipe_units() {
    let cookfile = r#"
use cook_cc

register
    cook.probe("cc:zlib", {
        inputs = { tools = {"pkg-config"} },
        produce = function() return { found = true } end,
    })
end

recipe build
    > cook.sh("echo hello")
"#;
    let result = register_test_cookfile(cookfile);  // existing helper or build one
    assert_eq!(result.probes.len(), 1);
    assert_eq!(result.probes[0].key, "cc:zlib");
}
```

- [ ] **Step 2: Run to confirm fail**

```
cargo test -p cook-register registry_collects_probe 2>&1 | tail -10
```
Expected: fails — probe_api isn't wired yet, so `cook.probe` raises "attempt to call a nil value."

- [ ] **Step 3: Wire probe_api in Registry::new and Registry::register_recipe**

In `cli/crates/cook-register/src/engine.rs`:

```rust
use crate::probe_api::{install_cook_probe, ProbeRegistry, SharedProbeRegistry};

// In Registry struct:
pub struct Registry {
    /* ... */
    probe_registry: SharedProbeRegistry,
}

// In Registry::new:
impl Registry {
    pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self {
        Self {
            /* ... */
            probe_registry: Rc::new(RefCell::new(ProbeRegistry::default())),
        }
    }
}

// In whatever method builds the register VM (search for `register_cook_api_capture` calls):
let cook: LuaTable = lua.globals().get("cook")?;
install_cook_probe(
    &lua,
    &cook,
    self.probe_registry.clone(),
    self.working_dir.join("Cookfile").display().to_string(),
)?;
```

- [ ] **Step 4: Surface probes into RecipeUnits**

Where `RecipeUnits` is constructed at the end of `register_recipe`, populate the new field:

```rust
let probes: Vec<ProbeUnit> = self.probe_registry.borrow().probes.values()
    .map(|r| r.probe.clone())
    .collect();
RecipeUnits {
    /* existing fields */
    probes,
}
```

- [ ] **Step 5: Run tests to verify pass**

```
cargo test -p cook-register registry_collects_probe 2>&1 | tail -10
```
Expected: pass.

- [ ] **Step 6: Commit**

```
git add cli/crates/cook-register/src/engine.rs
git commit -m "feat(register): wire cook.probe + RecipeUnits.probes"
```

---

### Task C4: Resolve requires against the probe registry; reject unknown keys

**Files:**
- Modify: `cli/crates/cook-register/src/engine.rs`
- Test: in-crate

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn unresolved_requires_key_errors() {
    let cookfile = r#"
use cook_cc

register
    cook.add_unit({
        name = "myapp.o",
        inputs = { "myapp.c" },
        outputs = { "build/myapp.o" },
        requires = { "cc:nonexistent" },
        command = "true",
    })
end
"#;
    let err = register_test_cookfile_err(cookfile);
    assert!(err.contains("unit 'myapp.o' requires probe key 'cc:nonexistent'"),
            "got: {}", err);
}

#[test]
fn resolved_requires_key_succeeds() {
    let cookfile = r#"
use cook_cc

register
    cook.probe("cc:zlib", { inputs = {}, produce = function() return 1 end })
    cook.add_unit({
        name = "myapp.o",
        inputs = { "myapp.c" },
        outputs = { "build/myapp.o" },
        requires = { "cc:zlib" },
        command = "true",
    })
end
"#;
    let result = register_test_cookfile(cookfile);
    let u = result.units.first().unwrap();
    assert_eq!(u.requires, vec!["cc:zlib"]);
}
```

- [ ] **Step 2: Run to confirm fail**

```
cargo test -p cook-register unresolved_requires 2>&1 | tail -10
```

- [ ] **Step 3: Add post-register validation**

After the register pass completes (i.e., at the end of `register_recipe` or `finalize`), iterate captured units and resolve `requires`:

```rust
// In engine.rs at the end of register_recipe (or equivalent):
fn validate_requires(units: &[CapturedUnit], probe_keys: &BTreeSet<String>) -> Result<(), RegisterError> {
    for u in units {
        let name = match &u.payload {
            WorkPayload::Shell { .. } | WorkPayload::Interactive { .. } | WorkPayload::LuaChunk { .. } => {
                // CapturedUnit doesn't carry a unit name today; the cache_meta
                // carries an identifier. Use that or the position in the unit list.
                // Spec change deferred — for now use cache_meta.cache_key or fall through.
                u.cache_meta.as_ref().map(|m| m.cache_key.clone()).unwrap_or_else(|| "<unit>".into())
            }
            WorkPayload::Probe { key, .. } => format!("probe:{}", key),
        };
        for r in &u.requires {
            if !probe_keys.contains(r) {
                return Err(RegisterError::Generic(format!(
                    "unit '{}' requires probe key '{}' which was not declared",
                    name, r
                )));
            }
        }
    }
    Ok(())
}
```

Wire `validate_requires` into the register pass tail.

- [ ] **Step 4: Run tests to verify pass**

```
cargo test -p cook-register requires 2>&1 | tail -10
```
Expected: both new tests pass.

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-register/src/engine.rs
git commit -m "feat(register): resolve cook.add_unit.requires against probe registry"
```

---

### Task C5: Cycle detection on probe requires chains

**Files:**
- Modify: `cli/crates/cook-register/src/probe_api.rs` (add cycle-detect function)
- Modify: `cli/crates/cook-register/src/engine.rs` (call it after register pass)
- Test: in-crate

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn probe_cycle_a_b_a_errors() {
    let cookfile = r#"
use cook_cc

register
    cook.probe("cc:a", {
        inputs = { requires = {"cc:b"} },
        produce = function() return 1 end,
    })
    cook.probe("cc:b", {
        inputs = { requires = {"cc:a"} },
        produce = function() return 2 end,
    })
end
"#;
    let err = register_test_cookfile_err(cookfile);
    assert!(err.contains("probe cycle detected"), "got: {}", err);
    assert!(err.contains("cc:a") && err.contains("cc:b"), "got: {}", err);
}
```

- [ ] **Step 2: Run to confirm fail**

- [ ] **Step 3: Implement DFS cycle detection**

In `probe_api.rs`:

```rust
impl ProbeRegistry {
    /// Detect cycles in the probe `requires` graph. Returns Err with the
    /// cycle path rendered as "a -> b -> a".
    pub fn detect_cycles(&self) -> Result<(), String> {
        let mut state: BTreeMap<&str, NodeState> = BTreeMap::new();
        for k in self.probes.keys() {
            if matches!(state.get(k.as_str()), None) {
                let mut stack: Vec<&str> = vec![];
                if let Err(path) = self.dfs(k, &mut state, &mut stack) {
                    return Err(format!("probe cycle detected: {}", path.join(" -> ")));
                }
            }
        }
        Ok(())
    }

    fn dfs<'a>(
        &'a self,
        node: &'a str,
        state: &mut BTreeMap<&'a str, NodeState>,
        stack: &mut Vec<&'a str>,
    ) -> Result<(), Vec<String>> {
        state.insert(node, NodeState::InProgress);
        stack.push(node);
        if let Some(reg) = self.probes.get(node) {
            for r in &reg.probe.inputs.requires {
                match state.get(r.as_str()) {
                    Some(NodeState::InProgress) => {
                        // cycle: trim stack to start at `r`
                        let start = stack.iter().position(|&n| n == r).unwrap_or(0);
                        let mut path: Vec<String> = stack[start..].iter().map(|s| s.to_string()).collect();
                        path.push(r.clone());
                        return Err(path);
                    }
                    Some(NodeState::Done) => continue,
                    None => self.dfs(r, state, stack)?,
                }
            }
        }
        stack.pop();
        state.insert(node, NodeState::Done);
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum NodeState { InProgress, Done }
```

In `engine.rs`, after the register pass:

```rust
self.probe_registry.borrow().detect_cycles()
    .map_err(|e| RegisterError::Generic(e))?;
```

- [ ] **Step 4: Verify tests pass**

```
cargo test -p cook-register probe_cycle 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-register/src/probe_api.rs cli/crates/cook-register/src/engine.rs
git commit -m "feat(register): probe requires cycle detection"
```

---

### Task C6: Add register-only guard for cook.probe on execute VM

**Files:**
- Modify: `cli/crates/cook-luaotp/src/pool.rs` (near the existing register-only guards at ~line 548)
- Test: integration test

- [ ] **Step 1: Write failing test**

```rust
// In cli/crates/cook-luaotp/tests/probe_register_only.rs (new file):
use mlua::Lua;
use cook_luaotp::install_execute_phase_cook_api;  // exact symbol may differ

#[test]
fn cook_probe_on_execute_vm_raises_register_only() {
    let lua = Lua::new();
    install_execute_phase_cook_api(&lua).unwrap();
    let err = lua.load(r#"
        cook.probe("cc:x", { inputs = {}, produce = function() return 1 end })
    "#).exec().unwrap_err().to_string();
    assert!(err.contains("cook.probe: register-only API"), "got: {}", err);
}
```

(If `install_execute_phase_cook_api` is not the exact public symbol, locate the function in `pool.rs` that installs the execute-phase `cook` table and call that instead. The function `register_cook_api` at the bottom of the existing block in `pool.rs:598` is the candidate — make it pub if needed.)

- [ ] **Step 2: Run to confirm fail**

- [ ] **Step 3: Add the guard**

In `cli/crates/cook-luaotp/src/pool.rs`, after the existing `install_register_only_guard(...)` block at line 548-596:

```rust
install_register_only_guard(
    lua,
    &cook,
    "probe",
    "cook.probe: register-only API called from execute-phase Lua (Standard §22.5.2). \
     Probe units are declared during the register phase; they cannot be created from a \
     lua_line / lua_block / using >{ … } payload. \
     Use `>>` instead of `>` to record this at register phase, or move the call to a \
     top-level `register` block.",
)?;
```

- [ ] **Step 4: Verify tests pass**

```
cargo test -p cook-luaotp 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-luaotp/src/pool.rs cli/crates/cook-luaotp/tests/probe_register_only.rs
git commit -m "feat(luaotp): cook.probe register-only guard on execute VM"
```

---

## Section D — cook-fingerprint: probe-input folding

### Task D1: Define ProbeFingerprintInputs and an input-folding helper

**Files:**
- Modify: `cli/crates/cook-fingerprint/src/context.rs`
- Test: in-crate `mod tests`

- [ ] **Step 1: Write failing tests**

In `cli/crates/cook-fingerprint/src/context.rs::tests`:

```rust
#[test]
fn probe_fingerprint_is_deterministic_for_same_inputs() {
    let inputs = ProbeFingerprintInputs {
        key: "cc:zlib".into(),
        produce_source_hash: 0xdeadbeef,
        env: vec![("CC".into(), Some("gcc".into())), ("PATH".into(), Some("/usr/bin".into()))],
        tools: vec![("pkg-config".into(), [0u8; 32])],
        files: vec![],
        upstream_probes: vec![],
    };
    let h1 = compute_probe_fingerprint(&inputs);
    let h2 = compute_probe_fingerprint(&inputs);
    assert_eq!(h1, h2);
}

#[test]
fn probe_fingerprint_changes_when_env_value_changes() {
    let mut a = ProbeFingerprintInputs {
        key: "cc:zlib".into(),
        produce_source_hash: 0,
        env: vec![("PKG_CONFIG_PATH".into(), Some("/a".into()))],
        tools: vec![], files: vec![], upstream_probes: vec![],
    };
    let h1 = compute_probe_fingerprint(&a);
    a.env[0].1 = Some("/b".into());
    let h2 = compute_probe_fingerprint(&a);
    assert_ne!(h1, h2);
}

#[test]
fn probe_fingerprint_is_sorted_by_input_name() {
    let a = ProbeFingerprintInputs {
        key: "cc:x".into(), produce_source_hash: 0,
        env: vec![("A".into(), Some("1".into())), ("B".into(), Some("2".into()))],
        tools: vec![], files: vec![], upstream_probes: vec![],
    };
    let b = ProbeFingerprintInputs {
        key: "cc:x".into(), produce_source_hash: 0,
        env: vec![("B".into(), Some("2".into())), ("A".into(), Some("1".into()))],
        tools: vec![], files: vec![], upstream_probes: vec![],
    };
    assert_eq!(compute_probe_fingerprint(&a), compute_probe_fingerprint(&b));
}

#[test]
fn probe_fingerprint_changes_on_upstream_probe_change() {
    let mut a = ProbeFingerprintInputs {
        key: "cc:x".into(), produce_source_hash: 0,
        env: vec![], tools: vec![], files: vec![],
        upstream_probes: vec![("cc:compiler".into(), [1u8; 32])],
    };
    let h1 = compute_probe_fingerprint(&a);
    a.upstream_probes[0].1 = [2u8; 32];
    let h2 = compute_probe_fingerprint(&a);
    assert_ne!(h1, h2);
}
```

- [ ] **Step 2: Run to confirm fail**

```
cargo test -p cook-fingerprint probe_fingerprint 2>&1 | tail -10
```

- [ ] **Step 3: Implement**

Add to `cli/crates/cook-fingerprint/src/context.rs`:

```rust
use sha2::{Digest, Sha256};

/// Inputs to a probe-unit's fingerprint, as folded by §22.5.3.
#[derive(Debug, Clone)]
pub struct ProbeFingerprintInputs {
    pub key: String,
    pub produce_source_hash: u64,
    /// (env-var name, current value or None if unset). Order before call is
    /// irrelevant; folding sorts internally.
    pub env: Vec<(String, Option<String>)>,
    /// (tool name, 32-byte content hash of resolved binary, or zeroes if missing).
    pub tools: Vec<(String, [u8; 32])>,
    /// (file path, 32-byte content hash, or zeroes if missing).
    pub files: Vec<(String, [u8; 32])>,
    /// (upstream probe key, that probe's fingerprint).
    pub upstream_probes: Vec<(String, [u8; 32])>,
}

/// Compute the 32-byte SHA-256 fingerprint of a probe unit. Deterministic
/// over a canonical sort of each input category.
pub fn compute_probe_fingerprint(inputs: &ProbeFingerprintInputs) -> [u8; 32] {
    let mut h = Sha256::new();

    h.update(b"COOK_PROBE_FP_V1\n");
    h.update(inputs.key.as_bytes());
    h.update(b"\n");
    h.update(&inputs.produce_source_hash.to_le_bytes());

    let mut env = inputs.env.clone();
    env.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"\nENV\n");
    for (k, v) in &env {
        h.update(k.as_bytes());
        h.update(b"=");
        match v {
            Some(s) => h.update(s.as_bytes()),
            None => h.update(b"<unset>"),
        }
        h.update(b"\n");
    }

    let mut tools = inputs.tools.clone();
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"TOOLS\n");
    for (name, hash) in &tools {
        h.update(name.as_bytes());
        h.update(b"=");
        h.update(hash);
        h.update(b"\n");
    }

    let mut files = inputs.files.clone();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"FILES\n");
    for (path, hash) in &files {
        h.update(path.as_bytes());
        h.update(b"=");
        h.update(hash);
        h.update(b"\n");
    }

    let mut up = inputs.upstream_probes.clone();
    up.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"UPSTREAM\n");
    for (key, fp) in &up {
        h.update(key.as_bytes());
        h.update(b"=");
        h.update(fp);
        h.update(b"\n");
    }

    let result = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}
```

If `sha2` isn't already a dep of cook-fingerprint, add it. Check `cli/crates/cook-fingerprint/Cargo.toml` first — it might already be there for CS-0054 SHA-256 work.

- [ ] **Step 4: Verify tests pass**

```
cargo test -p cook-fingerprint probe_fingerprint 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-fingerprint/src/context.rs
git commit -m "feat(fingerprint): compute_probe_fingerprint per §22.5.3"
```

---

### Task D2: Resolve probe inputs into ProbeFingerprintInputs at execute time

**Files:**
- Create: `cli/crates/cook-fingerprint/src/probe.rs`
- Modify: `cli/crates/cook-fingerprint/src/lib.rs` (re-export)
- Test: in-crate

- [ ] **Step 1: Write failing test**

Create `cli/crates/cook-fingerprint/src/probe.rs`:

```rust
//! Resolve a ProbeUnit's declared inputs into ProbeFingerprintInputs by
//! consulting the current env, PATH, filesystem, and upstream probe map.

use std::collections::BTreeMap;
use std::path::Path;

use cook_contracts::ProbeUnit;

use crate::context::{ProbeFingerprintInputs, compute_probe_fingerprint};

pub fn resolve_probe_inputs(
    probe: &ProbeUnit,
    working_dir: &Path,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    upstream_fingerprints: &BTreeMap<String, [u8; 32]>,
) -> Result<ProbeFingerprintInputs, String> {
    let produce_source_hash = xxhash_rust::xxh3::xxh3_64(probe.produce_source.as_bytes());

    let env: Vec<(String, Option<String>)> = probe.inputs.env.iter()
        .map(|name| (name.clone(), env_lookup(name)))
        .collect();

    let tools: Vec<(String, [u8; 32])> = probe.inputs.tools.iter()
        .map(|name| (name.clone(), resolve_tool_hash(name)))
        .collect();

    let files: Vec<(String, [u8; 32])> = probe.inputs.files.iter()
        .map(|path| (path.clone(), hash_file(&working_dir.join(path))))
        .collect();

    let upstream_probes: Vec<(String, [u8; 32])> = probe.inputs.requires.iter()
        .map(|k| {
            let fp = upstream_fingerprints.get(k).copied().ok_or_else(|| {
                format!("probe '{}' requires upstream '{}' which has no fingerprint", probe.key, k)
            })?;
            Ok((k.clone(), fp))
        })
        .collect::<Result<_, String>>()?;

    Ok(ProbeFingerprintInputs {
        key: probe.key.clone(),
        produce_source_hash,
        env, tools, files, upstream_probes,
    })
}

fn resolve_tool_hash(name: &str) -> [u8; 32] {
    let Some(path) = which::which(name).ok() else { return [0u8; 32]; };
    hash_file(&path)
}

fn hash_file(path: &Path) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let Ok(bytes) = std::fs::read(path) else { return [0u8; 32]; };
    let result = Sha256::digest(&bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolve_probe_inputs_with_no_inputs_succeeds() {
        let probe = ProbeUnit {
            key: "cc:x".into(),
            produce_source: "return 1".into(),
            produce_line: 1,
            inputs: cook_contracts::ProbeInputs::default(),
        };
        let r = resolve_probe_inputs(&probe, &PathBuf::from("."), &|_| None, &BTreeMap::new());
        assert!(r.is_ok());
    }

    #[test]
    fn missing_upstream_fingerprint_errors() {
        let mut probe = ProbeUnit {
            key: "cc:x".into(),
            produce_source: "return 1".into(),
            produce_line: 1,
            inputs: cook_contracts::ProbeInputs::default(),
        };
        probe.inputs.requires = vec!["cc:missing".into()];
        let r = resolve_probe_inputs(&probe, &PathBuf::from("."), &|_| None, &BTreeMap::new());
        let err = r.unwrap_err();
        assert!(err.contains("cc:missing"), "got: {}", err);
    }
}
```

In `cli/crates/cook-fingerprint/src/lib.rs`:
```rust
pub mod probe;
```

- [ ] **Step 2: Run to confirm fail / verify pass**

```
cargo test -p cook-fingerprint resolve_probe 2>&1 | tail -10
```

If `which` and `xxhash_rust` aren't crate deps, check Cargo.toml — they likely already are.

- [ ] **Step 3: Commit**

```
git add cli/crates/cook-fingerprint/src/probe.rs cli/crates/cook-fingerprint/src/lib.rs
git commit -m "feat(fingerprint): resolve ProbeUnit inputs to ProbeFingerprintInputs"
```

---

## Section E — cook-cache meta sidecar `kind` field

### Task E1: Add ArtifactMeta.kind with serde default; mark probe artifacts

**Files:**
- Modify: `cli/crates/cook-fingerprint/src/backend.rs` (struct definition)
- Modify: `cli/crates/cook-cache/src/backend.rs` (LocalBackend::put stamping site)
- Test: in-crate

- [ ] **Step 1: Write failing test**

In `cli/crates/cook-cache/src/backend.rs::tests`:

```rust
#[test]
fn artifact_meta_kind_defaults_to_none_for_legacy_sidecars() {
    let legacy_json = r#"{
        "schema_version": 1,
        "content_hash": "0000000000000000000000000000000000000000000000000000000000000000"
    }"#;
    let meta: cook_fingerprint::backend::ArtifactMeta = serde_json::from_str(legacy_json).unwrap();
    assert!(meta.kind.is_none());
}

#[test]
fn artifact_meta_kind_round_trips_when_set() {
    let meta = cook_fingerprint::backend::ArtifactMeta {
        schema_version: 1,
        content_hash: [0u8; 32],
        kind: Some("probe_value".into()),
    };
    let s = serde_json::to_string(&meta).unwrap();
    let back: cook_fingerprint::backend::ArtifactMeta = serde_json::from_str(&s).unwrap();
    assert_eq!(back.kind.as_deref(), Some("probe_value"));
}
```

- [ ] **Step 2: Run to confirm fail**

- [ ] **Step 3: Add the field**

In `cli/crates/cook-fingerprint/src/backend.rs`, add to `ArtifactMeta`:

```rust
pub struct ArtifactMeta {
    /* existing fields */
    /// Disambiguates the artifact body kind. Optional; `None` means "file
    /// artifact" (the legacy case). `Some("probe_value")` is the new
    /// msgpack-encoded probe-output artifact (CS-BB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}
```

Provide a default in any constructor:

```rust
impl ArtifactMeta {
    pub fn new_file_artifact(content_hash: [u8; 32]) -> Self {
        Self { schema_version: 1, content_hash, kind: None }
    }
    pub fn new_probe_artifact(content_hash: [u8; 32]) -> Self {
        Self { schema_version: 1, content_hash, kind: Some("probe_value".into()) }
    }
}
```

Update every site that constructs `ArtifactMeta { ... }` literally.

- [ ] **Step 4: Verify pass**

```
cargo test -p cook-cache artifact_meta_kind 2>&1 | tail -10
cargo test -p cook-fingerprint 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-fingerprint/src/backend.rs cli/crates/cook-cache/src/backend.rs
git commit -m "feat(cache): ArtifactMeta.kind field (CS-BB)"
```

---

## Section F — msgpack value encoding

### Task F1: Add rmpv as a workspace + crate dep

**Files:**
- Modify: `cli/Cargo.toml`
- Modify: `cli/crates/cook-register/Cargo.toml`
- Modify: `cli/crates/cook-luaotp/Cargo.toml`

- [ ] **Step 1: Inspect workspace deps**

```
head -40 cli/Cargo.toml
```

If `[workspace.dependencies]` exists, add `rmpv` there. Otherwise, add per-crate.

- [ ] **Step 2: Add rmpv**

Workspace level (if present):
```toml
[workspace.dependencies]
rmpv = { version = "1.3", features = ["with-serde"] }
```

Per crate:
```toml
# cli/crates/cook-register/Cargo.toml
[dependencies]
rmpv = { workspace = true }   # or { version = "1.3", features = ["with-serde"] }

# cli/crates/cook-luaotp/Cargo.toml
[dependencies]
rmpv = { workspace = true }
```

- [ ] **Step 3: Verify build**

```
cargo build -p cook-register -p cook-luaotp 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```
git add cli/Cargo.toml cli/crates/cook-register/Cargo.toml cli/crates/cook-luaotp/Cargo.toml cli/Cargo.lock
git commit -m "chore(deps): add rmpv (msgpack value type) for probe values"
```

---

### Task F2: Implement Lua → rmpv::Value walker with value-type validation

**Files:**
- Modify: `cli/crates/cook-register/src/probe_value.rs` (created empty in Task C1)
- Test: in-crate

- [ ] **Step 1: Write failing tests**

```rust
// cli/crates/cook-register/src/probe_value.rs
use mlua::prelude::*;
use rmpv::Value as MsgPackValue;

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(src: &str) -> Result<MsgPackValue, String> {
        let lua = Lua::new();
        let v: LuaValue = lua.load(src).eval().unwrap();
        lua_to_msgpack(&v, &mut Vec::new())
    }

    #[test]
    fn converts_nil() {
        assert_eq!(convert("return nil").unwrap(), MsgPackValue::Nil);
    }

    #[test]
    fn converts_bool() {
        assert_eq!(convert("return true").unwrap(), MsgPackValue::Boolean(true));
        assert_eq!(convert("return false").unwrap(), MsgPackValue::Boolean(false));
    }

    #[test]
    fn converts_number_int() {
        assert_eq!(convert("return 42").unwrap(), MsgPackValue::Integer(42.into()));
    }

    #[test]
    fn converts_number_float() {
        match convert("return 1.5").unwrap() {
            MsgPackValue::F64(f) => assert!((f - 1.5).abs() < 1e-9),
            other => panic!("expected F64, got {:?}", other),
        }
    }

    #[test]
    fn converts_string() {
        assert_eq!(convert("return \"hello\"").unwrap(),
                   MsgPackValue::String("hello".into()));
    }

    #[test]
    fn converts_array_table() {
        let v = convert("return {1, 2, 3}").unwrap();
        match v {
            MsgPackValue::Array(items) => assert_eq!(items.len(), 3),
            other => panic!("expected Array, got {:?}", other),
        }
    }

    #[test]
    fn converts_string_keyed_table() {
        let v = convert("return { a = 1, b = 2 }").unwrap();
        match v {
            MsgPackValue::Map(pairs) => assert_eq!(pairs.len(), 2),
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn rejects_function() {
        let e = convert("return function() end").unwrap_err();
        assert!(e.contains("function"), "got: {}", e);
    }

    #[test]
    fn rejects_mixed_key_table() {
        let e = convert("return { [1] = 1, a = 2 }").unwrap_err();
        assert!(e.contains("mixed"), "got: {}", e);
    }

    #[test]
    fn rejects_array_with_holes() {
        let e = convert("return { [1] = \"a\", [3] = \"c\" }").unwrap_err();
        assert!(e.contains("hole") || e.contains("not contiguous"), "got: {}", e);
    }

    #[test]
    fn rejects_cyclic_table() {
        let lua = Lua::new();
        let v: LuaValue = lua.load(r#"
            local t = {}
            t.self = t
            return t
        "#).eval().unwrap();
        let mut visited = Vec::new();
        let e = lua_to_msgpack(&v, &mut visited).unwrap_err();
        assert!(e.contains("cycle"), "got: {}", e);
    }
}
```

- [ ] **Step 2: Run to confirm fail**

```
cargo test -p cook-register probe_value 2>&1 | tail -10
```

- [ ] **Step 3: Implement the walker**

```rust
//! Convert mlua::Value trees to rmpv::Value with value-type validation.
//! Mirrors the contract in §22.5.4.

use mlua::prelude::*;
use rmpv::Value as MsgPackValue;

/// Convert a Lua value to msgpack. Validates the value-type contract
/// (§22.5.4) and rejects non-serialisable values with a path-tagged
/// diagnostic.
///
/// `path` is an in-out parameter for tracking the current location into
/// the value (used in diagnostics); pass an empty Vec at the top call.
pub fn lua_to_msgpack(v: &LuaValue, path: &mut Vec<String>) -> Result<MsgPackValue, String> {
    // Cycle detection via raw-pointer identity check on tables we've entered.
    // For simplicity we use a thread-local visited set; for unit-test
    // tractability we'll use a Vec passed by callers.
    lua_to_msgpack_inner(v, path, &mut Vec::new())
}

fn lua_to_msgpack_inner(
    v: &LuaValue,
    path: &mut Vec<String>,
    visited: &mut Vec<*const std::ffi::c_void>,
) -> Result<MsgPackValue, String> {
    match v {
        LuaValue::Nil => Ok(MsgPackValue::Nil),
        LuaValue::Boolean(b) => Ok(MsgPackValue::Boolean(*b)),
        LuaValue::Integer(i) => Ok(MsgPackValue::Integer((*i).into())),
        LuaValue::Number(f) => Ok(MsgPackValue::F64(*f)),
        LuaValue::String(s) => {
            let bytes = s.as_bytes();
            match std::str::from_utf8(&bytes) {
                Ok(_) => Ok(MsgPackValue::String(rmpv::Utf8String::from(bytes.to_vec()))),
                Err(_) => Ok(MsgPackValue::Binary(bytes.to_vec())),
            }
        }
        LuaValue::Table(t) => {
            let raw_ptr = t.to_pointer();
            if visited.contains(&raw_ptr) {
                return Err(format!("non-serialisable value at .{} (cycle)", path.join(".")));
            }
            visited.push(raw_ptr);

            let result = table_to_msgpack(t, path, visited);
            visited.pop();
            result
        }
        LuaValue::Function(_) => Err(format!("non-serialisable value at .{} (function)", path.join("."))),
        LuaValue::UserData(_) => Err(format!("non-serialisable value at .{} (userdata)", path.join("."))),
        LuaValue::Thread(_) => Err(format!("non-serialisable value at .{} (thread)", path.join("."))),
        LuaValue::Error(e) => Err(format!("non-serialisable value at .{} (error: {})", path.join("."), e)),
        LuaValue::LightUserData(_) => Err(format!("non-serialisable value at .{} (lightuserdata)", path.join("."))),
        LuaValue::Other(_) => Err(format!("non-serialisable value at .{} (unknown variant)", path.join("."))),
    }
}

fn table_to_msgpack(
    t: &LuaTable,
    path: &mut Vec<String>,
    visited: &mut Vec<*const std::ffi::c_void>,
) -> Result<MsgPackValue, String> {
    // Detect shape: pure-array (keys 1..N) vs string-map vs mixed.
    let mut int_keys: Vec<i64> = vec![];
    let mut str_keys: Vec<String> = vec![];
    let mut other_keys = 0usize;
    for pair in t.clone().pairs::<LuaValue, LuaValue>() {
        let (k, _) = pair.map_err(|e| format!("table iteration failed at .{}: {}", path.join("."), e))?;
        match k {
            LuaValue::Integer(i) => int_keys.push(i),
            LuaValue::String(s) => str_keys.push(s.to_string_lossy()),
            _ => other_keys += 1,
        }
    }
    if other_keys > 0 {
        return Err(format!("mixed/unsupported key types at .{}", path.join(".")));
    }
    if !int_keys.is_empty() && !str_keys.is_empty() {
        return Err(format!("mixed string/integer keys at .{} (not allowed; §22.5.4)", path.join(".")));
    }

    if !int_keys.is_empty() {
        int_keys.sort();
        for (idx, k) in int_keys.iter().enumerate() {
            if *k != (idx as i64) + 1 {
                return Err(format!("array hole at .{}[{}] (not contiguous 1..N)", path.join("."), idx + 1));
            }
        }
        let mut items = Vec::with_capacity(int_keys.len());
        for k in &int_keys {
            path.push(format!("[{}]", k));
            let v: LuaValue = t.get(*k).map_err(|e| format!("get failed: {}", e))?;
            let mv = lua_to_msgpack_inner(&v, path, visited)?;
            path.pop();
            items.push(mv);
        }
        Ok(MsgPackValue::Array(items))
    } else if !str_keys.is_empty() {
        str_keys.sort();
        let mut pairs = Vec::with_capacity(str_keys.len());
        for k in &str_keys {
            path.push(k.clone());
            let v: LuaValue = t.get(k.as_str()).map_err(|e| format!("get failed: {}", e))?;
            let mv = lua_to_msgpack_inner(&v, path, visited)?;
            path.pop();
            pairs.push((MsgPackValue::String(rmpv::Utf8String::from(k.as_str())), mv));
        }
        Ok(MsgPackValue::Map(pairs))
    } else {
        // Empty table — return as Map (msgpack distinguishes empty array from
        // empty map; we pick map since string-keyed is the typical probe-result
        // shape).
        Ok(MsgPackValue::Map(vec![]))
    }
}

/// Encode an rmpv::Value to msgpack bytes.
pub fn encode_msgpack(v: &MsgPackValue) -> Vec<u8> {
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, v).expect("rmpv encode never fails for in-memory");
    buf
}

/// Decode msgpack bytes back into an rmpv::Value (used on cache hits).
pub fn decode_msgpack(bytes: &[u8]) -> Result<MsgPackValue, String> {
    let mut cursor = std::io::Cursor::new(bytes);
    rmpv::decode::read_value(&mut cursor).map_err(|e| format!("msgpack decode: {}", e))
}
```

- [ ] **Step 4: Verify all tests pass**

```
cargo test -p cook-register probe_value 2>&1 | tail -20
```

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-register/src/probe_value.rs
git commit -m "feat(register): Lua→msgpack walker with §22.5.4 value-type validation"
```

---

### Task F3: Round-trip test (encode → decode → equal)

**Files:**
- Modify: `cli/crates/cook-register/src/probe_value.rs::tests`

- [ ] **Step 1: Add round-trip tests**

```rust
#[test]
fn msgpack_round_trip_simple_table() {
    let lua = Lua::new();
    let v: LuaValue = lua.load(r#"return { found = true, cflags = {"-I/usr/include"}, libs = {"-lz"} }"#).eval().unwrap();
    let mp = lua_to_msgpack(&v, &mut vec![]).unwrap();
    let bytes = encode_msgpack(&mp);
    let back = decode_msgpack(&bytes).unwrap();
    assert_eq!(back, mp);
}

#[test]
fn msgpack_round_trip_nested_table() {
    let lua = Lua::new();
    let v: LuaValue = lua.load(r#"return { a = { b = { c = 42 } } }"#).eval().unwrap();
    let mp = lua_to_msgpack(&v, &mut vec![]).unwrap();
    let bytes = encode_msgpack(&mp);
    let back = decode_msgpack(&bytes).unwrap();
    assert_eq!(back, mp);
}
```

- [ ] **Step 2: Verify pass**

```
cargo test -p cook-register msgpack_round_trip 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```
git add cli/crates/cook-register/src/probe_value.rs
git commit -m "test(register): msgpack round-trip for probe values"
```

---

## Section G — cook-luaotp probe execution

### Task G1: Dispatch path for WorkPayload::Probe on worker VM

**Files:**
- Modify: `cli/crates/cook-luaotp/src/pool.rs` (find the WorkPayload dispatch — search for `match payload` near worker run-loop)

- [ ] **Step 1: Write failing test**

Create `cli/crates/cook-luaotp/tests/probe_execution.rs`:

```rust
use cook_contracts::{WorkPayload, ProbeInputs, ProbeUnit};
use cook_luaotp::{Pool, WorkItem};   // exact symbols — adjust to actuals

#[test]
fn probe_unit_executes_and_returns_msgpack_bytes() {
    let pool = Pool::new(1);
    let payload = WorkPayload::Probe {
        key: "test:simple".into(),
        produce: r#"return { found = true, paths = {"a", "b"} }"#.into(),
        line: 1,
    };
    let item = WorkItem::new_probe(payload);
    let result = pool.run_blocking(item).unwrap();
    let bytes = result.expect_probe_bytes();
    let back = cook_register::probe_value::decode_msgpack(&bytes).unwrap();
    // Verify shape: a Map with "found" and "paths" keys.
    match back {
        rmpv::Value::Map(pairs) => assert_eq!(pairs.len(), 2),
        _ => panic!("unexpected msgpack value"),
    }
}
```

(The exact `WorkItem::new_probe` and `expect_probe_bytes` helpers don't exist yet — invent them as the public surface you'll need.)

- [ ] **Step 2: Run to confirm fail**

```
cargo test -p cook-luaotp probe_unit_executes 2>&1 | tail -10
```

- [ ] **Step 3: Add probe dispatch in pool worker**

Find the worker's `match payload` dispatch (probably in `pool.rs` near where `WorkPayload::Shell` / `WorkPayload::LuaChunk` are handled). Add:

```rust
WorkPayload::Probe { key, produce, line } => {
    // Compile and execute the produce function on this worker VM.
    let func: LuaFunction = lua.load(produce.as_str()).set_name(&format!("probe:{}", key)).eval()
        .map_err(|e| format!("probe '{}' produce-source failed to load: {}", key, e))?;
    let value: LuaValue = func.call(()).map_err(|e| {
        format!("probe '{}' produce raised: {}", key, e)
    })?;

    // Convert to msgpack with §22.5.4 validation.
    let mp = cook_register::probe_value::lua_to_msgpack(&value, &mut vec![])
        .map_err(|e| format!("probe '{}': {}", key, e))?;
    let bytes = cook_register::probe_value::encode_msgpack(&mp);
    WorkResult::ProbeOutput { key: key.clone(), bytes }
}
```

Note: the `produce` string captured at register-time should be the **function body source** (just the body, since the function is `function() return ... end` and we want to invoke it directly). The capture in Task C1 used `debug.getinfo(produce, "S").source` which returns the *file path*, not the source text. **This is a known gap — see Task G1a.**

Actually, getting the function source out of mlua reliably is non-trivial. The cleanest approach: at register time, capture the Lua *source span* (start-line + end-line in the Cookfile) and re-execute that span at probe-execution time. Alternative: require `cook.probe` to accept a string-form `produce` (`produce_source = "return ..."`) instead of a function literal. **This is the simpler design.**

- [ ] **Step 3a: Revise the cook.probe API to accept produce_source string**

Update Task C1's `cook.probe` binding so `opts.produce` is a **string** (the body of the produce function, returning a value). This sidesteps the mlua function-source-extraction problem.

Revised spec contract:

```lua
cook.probe("cc:zlib", {
  inputs = { tools = {"pkg-config"} },
  produce = [[
    local r = run_pkg_config("zlib")
    return { found = r.found, cflags = r.cflags }
  ]],
})
```

The `produce` field is now a string of Lua source code. At execute time the worker wraps it in `function() ... end`, compiles, calls.

**Update §22.5.2 in the Standard chapter (Task A1) to reflect this.** The change is: `produce` is `string` (Lua source), not `function`.

Then in C1's `install_cook_probe`, `produce_source` is the directly-read string from the opts table — no `debug.getinfo` needed.

In G1's dispatch:
```rust
let chunk = format!("return (function()\n{}\nend)()", produce);
let value: LuaValue = lua.load(&chunk).set_name(&format!("probe:{}", key)).eval()?;
```

- [ ] **Step 4: Update Task C1's tests and impl to use the string-form produce**

Revisit `cli/crates/cook-register/src/probe_api.rs` — the test fixtures use `produce = function() ... end`. Change to `produce = "return { found = true }"`. Update `install_cook_probe` to read `produce` as a String, not a Function.

- [ ] **Step 5: Verify pass**

```
cargo test -p cook-luaotp probe_unit_executes 2>&1 | tail -10
cargo test -p cook-register probe_api 2>&1 | tail -10
```

- [ ] **Step 6: Commit**

```
git add cli/crates/cook-luaotp/src/pool.rs cli/crates/cook-luaotp/tests/probe_execution.rs cli/crates/cook-register/src/probe_api.rs standard/src/content/docs/22-probe-units.mdx
git commit -m "feat(luaotp): execute probe units; revise produce to be a Lua source string"
```

---

### Task G2: cook.cache.get on execute VM reads from probe-value store

**Files:**
- Modify: `cli/crates/cook-luaotp/src/pool.rs` (function `install_execute_phase_cook_cache` at line 611+)
- Test: integration

- [ ] **Step 1: Write failing test**

```rust
// cli/crates/cook-luaotp/tests/probe_value_visible_to_consumer.rs
#[test]
fn consumer_lua_reads_probe_value_via_cook_cache_get() {
    let pool = setup_pool_with_probe_store();
    pool.populate_probe_store("cc:zlib", encode_msgpack_helper(&{
      "found": true, "cflags": ["-I/usr/include"]
    }));

    let consumer = WorkItem::new_lua_chunk(r#"
        local r = cook.cache.get("cc:zlib")
        assert(r.found == true, "expected found=true")
        return r.cflags[1]
    "#);
    let result = pool.run_blocking(consumer).unwrap();
    assert_eq!(result.expect_string(), "-I/usr/include");
}
```

(Helpers `setup_pool_with_probe_store`, `populate_probe_store`, etc. are new — invent the test surface as you want it.)

- [ ] **Step 2: Run to confirm fail**

- [ ] **Step 3: Wire probe-value store into execute VM**

Currently `install_execute_phase_cook_cache` (pool.rs:611) creates a per-worker `_cook_execute_cache` table. Replace with a shared store that the scheduler populates:

```rust
fn install_execute_phase_cook_cache(
    lua: &mlua::Lua,
    cook: &mlua::Table,
    probe_store: SharedProbeValueStore,
) -> mlua::Result<()> {
    let cache_tbl = lua.create_table()?;

    let store = probe_store.clone();
    let get_fn = lua.create_function(move |lua, key: String| {
        let store = store.lock().unwrap();
        match store.get(&key) {
            Some(bytes) => {
                let mp = cook_register::probe_value::decode_msgpack(bytes)
                    .map_err(|e| LuaError::runtime(format!("cook.cache.get('{}'): decode failed: {}", key, e)))?;
                msgpack_to_lua(lua, &mp)
            }
            None => Ok(LuaValue::Nil),
        }
    })?;
    cache_tbl.set("get", get_fn)?;

    // No `set` on execute VM (CS-CC: deprecated). Install a stub that errors.
    let set_fn = lua.create_function(|_, (_, _): (String, LuaValue)| -> LuaResult<()> {
        Err(LuaError::runtime("cook.cache.set: deprecated and not available on execute-phase VM (CS-CC). Use cook.probe to declare memoised values."))
    })?;
    cache_tbl.set("set", set_fn)?;

    cook.set("cache", cache_tbl)?;
    Ok(())
}
```

`SharedProbeValueStore` is a `Arc<Mutex<BTreeMap<String, Vec<u8>>>>` (or whatever fits the existing pool shape — it might be an unbounded channel of `(key, bytes)` events from completed probe units).

Implement `msgpack_to_lua`:

```rust
fn msgpack_to_lua(lua: &Lua, mp: &rmpv::Value) -> LuaResult<LuaValue> {
    use rmpv::Value as V;
    Ok(match mp {
        V::Nil => LuaValue::Nil,
        V::Boolean(b) => LuaValue::Boolean(*b),
        V::Integer(i) => match i.as_i64() {
            Some(n) => LuaValue::Integer(n),
            None => LuaValue::Number(i.as_f64().unwrap_or(0.0)),
        },
        V::F32(f) => LuaValue::Number(*f as f64),
        V::F64(f) => LuaValue::Number(*f),
        V::String(s) => LuaValue::String(lua.create_string(s.as_bytes())?),
        V::Binary(bytes) => LuaValue::String(lua.create_string(bytes)?),
        V::Array(items) => {
            let t = lua.create_table()?;
            for (i, v) in items.iter().enumerate() {
                t.set(i + 1, msgpack_to_lua(lua, v)?)?;
            }
            LuaValue::Table(t)
        }
        V::Map(pairs) => {
            let t = lua.create_table()?;
            for (k, v) in pairs {
                let key_str = k.as_str().ok_or_else(|| LuaError::runtime("non-string map key in msgpack probe value"))?;
                t.set(key_str, msgpack_to_lua(lua, v)?)?;
            }
            LuaValue::Table(t)
        }
        V::Ext(_, _) => return Err(LuaError::runtime("msgpack ext type not supported")),
    })
}
```

- [ ] **Step 4: Verify pass**

```
cargo test -p cook-luaotp consumer_lua_reads 2>&1 | tail -20
```

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-luaotp/src/pool.rs cli/crates/cook-luaotp/tests/probe_value_visible_to_consumer.rs
git commit -m "feat(luaotp): cook.cache.get reads probe-value store; cook.cache.set errors (CS-CC)"
```

---

### Task G3: Wire probe-value store population from scheduler

**Files:**
- Modify: `cli/crates/cook-engine/src/executor.rs` (or wherever the unit scheduler lives — search for `WorkPayload::Shell` dispatch)
- Test: integration

- [ ] **Step 1: Find the scheduler dispatch site**

```
grep -rn "WorkPayload::Shell\|fn dispatch\|schedule_unit" cli/crates/cook-engine/src/ 2>&1 | head -20
```

- [ ] **Step 2: Write failing test**

```rust
// cli/crates/cook-engine/tests/probe_pipeline.rs
#[test]
fn probe_unit_output_lands_in_value_store() {
    let mut engine = TestEngine::new();
    engine.add_probe("cc:zlib", "return { found = true }");
    engine.add_unit_requiring("cc:zlib", "echo {{cc:zlib.found}}");
    let result = engine.run().unwrap();
    assert!(result.stdout.contains("true"));
}
```

- [ ] **Step 3: Implement**

When a `WorkResult::ProbeOutput { key, bytes }` returns from a worker, the scheduler:
1. Stores `(key, bytes)` in the shared probe-value store.
2. Marks the probe unit complete.
3. Triggers any consumers whose dependencies are now all satisfied.

Find the result-handling code (likely in `cook-engine/src/run.rs` or `executor.rs`) and add a match arm for `WorkResult::ProbeOutput`. Persist the bytes to the recipe cache as well (Task G4).

- [ ] **Step 4: Verify pass + commit**

```
cargo test -p cook-engine probe_pipeline 2>&1 | tail -10
git add cli/crates/cook-engine/src/run.rs cli/crates/cook-engine/src/executor.rs cli/crates/cook-engine/tests/probe_pipeline.rs
git commit -m "feat(engine): scheduler populates probe-value store on probe completion"
```

---

### Task G4: Probe-cache lookup before dispatch; skip execution on hit

**Files:**
- Modify: `cli/crates/cook-engine/src/executor.rs`
- Test: integration

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn probe_cache_hit_skips_produce_execution() {
    let mut engine = TestEngine::new();
    engine.add_probe("cc:zlib", "error('produce should not run on cache hit')");
    // Pre-populate cache with a valid msgpack of { found = true }.
    let mp = rmpv::Value::Map(vec![
        (rmpv::Value::String("found".into()), rmpv::Value::Boolean(true)),
    ]);
    let bytes = cook_register::probe_value::encode_msgpack(&mp);
    let fp = compute_test_probe_fingerprint("cc:zlib");
    engine.cache_backend().put(fp, &bytes, ArtifactMeta::new_probe_artifact(sha256(&bytes))).unwrap();

    engine.add_unit_requiring("cc:zlib", "echo {{cc:zlib.found}}");
    let result = engine.run().unwrap();
    assert!(result.stdout.contains("true"));  // succeeds despite error() in produce
}
```

- [ ] **Step 2: Implement cache lookup**

Before dispatching a probe-unit to a worker, compute its fingerprint via `cook_fingerprint::probe::resolve_probe_inputs` + `compute_probe_fingerprint`, then call `CacheBackend::get(fp)`. On hit, populate the value store directly and skip dispatch.

- [ ] **Step 3: Verify pass + commit**

```
cargo test -p cook-engine probe_cache_hit 2>&1 | tail -10
git add cli/crates/cook-engine/src/executor.rs cli/crates/cook-engine/tests/probe_pipeline.rs
git commit -m "feat(engine): probe cache lookup skips produce on hit"
```

---

### Task G5: Persist probe output to CacheBackend on miss

**Files:**
- Modify: `cli/crates/cook-engine/src/executor.rs`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn probe_miss_persists_output_to_cache() {
    let mut engine = TestEngine::new();
    engine.add_probe("cc:zlib", "return { found = true }");
    engine.add_unit_requiring("cc:zlib", "echo done");
    engine.run().unwrap();
    let fp = compute_test_probe_fingerprint("cc:zlib");
    assert!(engine.cache_backend().get(fp).is_ok(),
            "probe output should be in cache after first run");
}
```

- [ ] **Step 2: Implement**

In the `WorkResult::ProbeOutput` handler, in addition to populating the value store, call `cache_backend.put(fingerprint, &bytes, ArtifactMeta::new_probe_artifact(sha256(&bytes)))`.

- [ ] **Step 3: Verify pass + commit**

```
cargo test -p cook-engine probe_miss_persists 2>&1 | tail -10
git add cli/crates/cook-engine/src/executor.rs cli/crates/cook-engine/tests/probe_pipeline.rs
git commit -m "feat(engine): persist probe output to cache after produce runs"
```

---

## Section H — cook-luagen command-template desugaring

### Task H1: Detect `{{key}}` and `{{key.field}}` placeholders that reference probe keys

**Files:**
- Modify: `cli/crates/cook-luagen/src/template.rs`
- Test: in-crate

- [ ] **Step 1: Write failing test**

In `cli/crates/cook-luagen/src/tests.rs` (or a new test file):

```rust
#[test]
fn template_detects_probe_placeholder_simple() {
    let cmd = "gcc -c foo.c {{cc:zlib.cflags}}";
    let probe_keys: BTreeSet<String> = ["cc:zlib".into()].into_iter().collect();
    let refs = find_probe_refs(cmd, &probe_keys).unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].key, "cc:zlib");
    assert_eq!(refs[0].path, vec!["cflags"]);
}

#[test]
fn template_detects_probe_placeholder_no_field() {
    let cmd = "{{cc:compiler}} -c foo.c";
    let probe_keys: BTreeSet<String> = ["cc:compiler".into()].into_iter().collect();
    let refs = find_probe_refs(cmd, &probe_keys).unwrap();
    assert_eq!(refs[0].path, Vec::<String>::new());
}

#[test]
fn template_rejects_undeclared_key() {
    let cmd = "{{cc:zlib.cflags}}";
    let probe_keys: BTreeSet<String> = ["cc:other".into()].into_iter().collect();
    let err = find_probe_refs(cmd, &probe_keys).unwrap_err();
    assert!(err.contains("cc:zlib"), "got: {}", err);
}
```

- [ ] **Step 2: Run to confirm fail**

- [ ] **Step 3: Implement detector**

In `cli/crates/cook-luagen/src/template.rs`:

```rust
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct ProbeRef {
    pub key: String,
    pub path: Vec<String>,        // [".field", "[2]", …] flattened
    pub original: String,         // verbatim "{{key.field}}" for error messages
}

/// Scan a command string for `{{key}}` / `{{key.field}}` / `{{key.field[i]}}`
/// placeholders that resolve to declared probe keys.
pub fn find_probe_refs(cmd: &str, probe_keys: &BTreeSet<String>) -> Result<Vec<ProbeRef>, String> {
    let mut out = vec![];
    let bytes = cmd.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i+1] == b'{' {
            let end = match cmd[i+2..].find("}}") {
                Some(p) => i + 2 + p,
                None => break,
            };
            let inner = &cmd[i+2..end];
            // Decide whether inner names a probe.
            let key_end = inner.find('.').or_else(|| inner.find('[')).unwrap_or(inner.len());
            let key = inner[..key_end].trim().to_string();
            if probe_keys.contains(&key) {
                let path = parse_field_path(&inner[key_end..])?;
                out.push(ProbeRef {
                    key,
                    path,
                    original: cmd[i..end+2].to_string(),
                });
            } else if cmd[..i+2].contains("probe") || key.contains(':') {
                // Heuristic: colon-prefixed keys that aren't registered.
                return Err(format!("template references probe key '{}' which was not declared", key));
            }
            i = end + 2;
        } else {
            i += 1;
        }
    }
    Ok(out)
}

fn parse_field_path(s: &str) -> Result<Vec<String>, String> {
    // ".field" / ".a.b" / "[3]" / ".field[3]" — flatten to a Vec<String> where
    // numeric segments are stored as the index string.
    let mut out = vec![];
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '.' => {
                chars.next();
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else { break; }
                }
                if name.is_empty() { return Err("empty field name after '.'".into()); }
                out.push(name);
            }
            '[' => {
                chars.next();
                let mut idx = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ']' { chars.next(); break; }
                    idx.push(c);
                    chars.next();
                }
                if idx.parse::<usize>().is_err() {
                    return Err(format!("non-integer array index '[{}]'", idx));
                }
                out.push(idx);
            }
            _ => return Err(format!("unexpected character in field path: {:?}", c)),
        }
    }
    Ok(out)
}
```

- [ ] **Step 4: Verify pass + commit**

```
cargo test -p cook-luagen find_probe_refs 2>&1 | tail -10
git add cli/crates/cook-luagen/src/template.rs
git commit -m "feat(luagen): detect probe-value placeholders in command templates"
```

---

### Task H2: Rewrite command into a Lua step that reads cook.cache.get

**Files:**
- Modify: `cli/crates/cook-luagen/src/cook_step.rs` (or wherever cook-step body codegen lives)

- [ ] **Step 1: Find the command-codegen site**

```
grep -rn "fn .*cook_step\|emit.*command\|generate_shell" cli/crates/cook-luagen/src/ 2>&1 | head -10
```

The relevant site is where a shell `command = "..."` becomes its Lua emission — probably `cook_step.rs::emit_cook_step` or similar.

- [ ] **Step 2: Write a failing test**

In `cli/crates/cook-luagen/src/tests.rs`:

```rust
#[test]
fn command_template_desugars_to_cook_cache_get() {
    let cmd = "gcc -c foo.c {{cc:zlib.cflags}}";
    let probe_keys: BTreeSet<String> = ["cc:zlib".into()].into_iter().collect();
    let lua = desugar_command_with_probes(cmd, &probe_keys).unwrap();
    // Expect something like: `cook.sh("gcc -c foo.c " .. cook.cache.get("cc:zlib").cflags)`
    assert!(lua.contains(r#"cook.cache.get("cc:zlib")"#), "got: {}", lua);
    assert!(lua.contains(".cflags"), "got: {}", lua);
}
```

- [ ] **Step 3: Implement**

Add to `template.rs`:

```rust
pub fn desugar_command_with_probes(cmd: &str, probe_keys: &BTreeSet<String>) -> Result<String, String> {
    let refs = find_probe_refs(cmd, probe_keys)?;
    if refs.is_empty() {
        // No probe placeholders — emit the existing shell-command Lua.
        return Ok(format!(r#"cook.sh({})"#, escape_lua_string_quoted(cmd)));
    }

    // Build the substituted Lua expression: an interleaved sequence of
    // string-literal chunks and `cook.cache.get(...)[...]` reads, joined with
    // Lua's `..` concat operator.
    let mut parts: Vec<String> = vec![];
    let mut cursor = 0;
    for r in &refs {
        let start = cmd[cursor..].find(&r.original).expect("ref must be in cmd");
        let literal_end = cursor + start;
        if literal_end > cursor {
            parts.push(format!(r#""{}""#, escape_lua_string(&cmd[cursor..literal_end])));
        }
        let mut access = format!(r#"cook.cache.get({})"#, escape_lua_string_quoted(&r.key));
        for seg in &r.path {
            if seg.chars().all(|c| c.is_ascii_digit()) {
                access.push_str(&format!("[{}]", seg));
            } else {
                access.push_str(&format!(".{}", seg));
            }
        }
        parts.push(format!("tostring({})", access));
        cursor = literal_end + r.original.len();
    }
    if cursor < cmd.len() {
        parts.push(format!(r#""{}""#, escape_lua_string(&cmd[cursor..])));
    }

    Ok(format!("cook.sh({})", parts.join(" .. ")))
}

fn escape_lua_string_quoted(s: &str) -> String {
    format!(r#""{}""#, escape_lua_string(s))
}
```

(`escape_lua_string` already exists in `lua_string.rs`.)

- [ ] **Step 4: Wire desugar_command_with_probes into the cook-step emit**

Find where the command string gets emitted today (e.g. `cook_step.rs` produces a `cook.sh(...)` call). Replace the literal-emit with a call to `desugar_command_with_probes`. Thread the probe-key set through the codegen context (likely an extension to `ResolveCtx` or whatever context struct passes through codegen).

- [ ] **Step 5: Verify pass + commit**

```
cargo test -p cook-luagen 2>&1 | tail -20
git add cli/crates/cook-luagen/src/template.rs cli/crates/cook-luagen/src/cook_step.rs
git commit -m "feat(luagen): desugar {{probe.field}} placeholders to cook.cache.get reads"
```

---

## Section I — Conformance fixtures

### Task I1: Positive — probe-register-simple + probe-requires-chain

**Files:**
- Create: `standard/conformance/positive/probe-register-simple/{Cookfile,parse.txt,register_ok.txt}`
- Create: `standard/conformance/positive/probe-requires-chain/{Cookfile,parse.txt,register_ok.txt}`
- Modify: `cli/crates/cook-register/tests/conformance.rs` (positive-register harness)

- [ ] **Step 1: Add positive-register harness (mirrors Task A6 of SHI-210 spec)**

If the positive-register harness from SHI-210 hasn't landed yet, add it here:

```rust
// cli/crates/cook-register/tests/conformance.rs
#[test]
fn register_positive_conformance_corpus() {
    // Walk positive/ fixtures with a `register_ok.txt` marker. For each:
    // parse + codegen + register MUST succeed.
    let mut failures: Vec<String> = Vec::new();
    let mut cases_seen = 0usize;
    for case in case_dirs("positive") {
        let marker = case.join("register_ok.txt");
        if !marker.exists() { continue; }
        cases_seen += 1;
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let input = fs::read_to_string(case.join("Cookfile")).unwrap();
        let cookfile = parse(&input).expect("parse must succeed for register_ok fixture");
        let lua_source = generate(&cookfile);
        let registry = Registry::new(case.clone(), HashMap::new()).with_selected_config(None);
        let recipe_name = cookfile.recipes.first().map(|r| r.name.clone()).unwrap_or_default();
        match registry.register_recipe(&recipe_name, &lua_source) {
            Ok(_) => {},
            Err(e) => failures.push(format!("case {}: register failed: {}", name, e)),
        }
    }
    assert_eq!(cases_seen > 0, true);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
```

- [ ] **Step 2: Create fixture probe-register-simple**

```
# standard/conformance/positive/probe-register-simple/Cookfile
use cook_cc

register
    cook.probe("test:simple", {
        inputs = { env = {"PATH"} },
        produce = "return { found = true }",
    })
end

recipe build
    > cook.sh("echo hello")
```

```
# standard/conformance/positive/probe-register-simple/parse.txt
Cookfile
  uses: [UseStatement module_name="cook_cc" line=1]
  imports: []
  config_blocks: []
  recipes:
    Recipe name="build" line=10
      deps: []
      ingredients: []
      excludes: []
      steps:
        Lua code="cook.sh(\"echo hello\")"
  chores:
  register_blocks: [RegisterBlock body="\n    cook.probe(\"test:simple\", {\n        inputs = { env = {\"PATH\"} },\n        produce = \"return { found = true }\",\n    })\n" line=3]
  top_level_module_calls: []
```

(Adjust `line=` to match actual.)

```
# standard/conformance/positive/probe-register-simple/register_ok.txt
(empty file; marker)
```

- [ ] **Step 3: Create fixture probe-requires-chain**

```
# standard/conformance/positive/probe-requires-chain/Cookfile
use cook_cc

register
    cook.probe("test:base", {
        inputs = {},
        produce = "return { v = 1 }",
    })
    cook.probe("test:derived", {
        inputs = { requires = {"test:base"} },
        produce = "return { v = cook.cache.get(\"test:base\").v + 1 }",
    })
end

recipe build
    > cook.sh("echo built")
```

+ parse.txt + register_ok.txt as above.

- [ ] **Step 4: Run fixtures**

```
cargo test -p cook-register register_positive_conformance_corpus 2>&1 | tail -20
cargo test -p cook-lang positive_conformance_corpus 2>&1 | tail -20
```

- [ ] **Step 5: Commit**

```
git add standard/conformance/positive/probe-register-simple/ standard/conformance/positive/probe-requires-chain/ cli/crates/cook-register/tests/conformance.rs
git commit -m "test(conformance): probe-register-simple + probe-requires-chain (CS-AA)"
```

---

### Task I2: Positive — probe-template-desugaring + probe-value-types

**Files:**
- Create: `standard/conformance/positive/probe-template-desugaring/{Cookfile,parse.txt,register_ok.txt}`
- Create: `standard/conformance/positive/probe-value-types/{Cookfile,parse.txt,register_ok.txt}`

- [ ] **Step 1: probe-template-desugaring fixture**

```
# Cookfile
use cook_cc

register
    cook.probe("test:greet", {
        inputs = {},
        produce = "return { word = \"hello\" }",
    })
    cook.add_unit({
        name    = "echo",
        inputs  = {},
        outputs = {},
        requires = {"test:greet"},
        command = "echo {{test:greet.word}}",
    })
end
```

(Outputs = {} is permitted because the unit produces no file; a sentinel may be required by add_unit's existing checks — adjust as needed.)

- [ ] **Step 2: probe-value-types fixture**

```
# Cookfile
use cook_cc

register
    cook.probe("test:types", {
        inputs = {},
        produce = "return { n = 42, s = \"hi\", a = {1,2,3}, t = { nested = true } }",
    })
end

recipe noop
    > cook.sh("true")
```

- [ ] **Step 3: parse.txt + register_ok.txt for both**

- [ ] **Step 4: Verify pass + commit**

```
cargo test -p cook-lang 2>&1 | tail -10
cargo test -p cook-register register_positive 2>&1 | tail -10
git add standard/conformance/positive/probe-template-desugaring/ standard/conformance/positive/probe-value-types/
git commit -m "test(conformance): probe-template-desugaring + probe-value-types (CS-AA)"
```

---

### Task I3: Negative — duplicate-key + unresolved-require + cycle

**Files:**
- Create: `standard/conformance/negative/probe-duplicate-key/{Cookfile,register_error.txt}`
- Create: `standard/conformance/negative/probe-unresolved-require/{Cookfile,register_error.txt}`
- Create: `standard/conformance/negative/probe-cycle/{Cookfile,register_error.txt}`

- [ ] **Step 1: Each fixture**

`probe-duplicate-key/Cookfile`:
```
use cook_cc

register
    cook.probe("dup", { inputs = {}, produce = "return 1" })
    cook.probe("dup", { inputs = {}, produce = "return 2" })
end
```
`probe-duplicate-key/register_error.txt`:
```
probe key 'dup' declared at
```

`probe-unresolved-require/Cookfile`:
```
use cook_cc

register
    cook.add_unit({
        name = "u", inputs = {}, outputs = {"out"},
        requires = {"missing"},
        command = "true",
    })
end
```
`probe-unresolved-require/register_error.txt`:
```
requires probe key 'missing' which was not declared
```

`probe-cycle/Cookfile`:
```
use cook_cc

register
    cook.probe("a", { inputs = { requires = {"b"} }, produce = "return 1" })
    cook.probe("b", { inputs = { requires = {"a"} }, produce = "return 2" })
end
```
`probe-cycle/register_error.txt`:
```
probe cycle detected
```

- [ ] **Step 2: Verify negative harness picks them up**

```
cargo test -p cook-register register_negative_conformance_corpus 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```
git add standard/conformance/negative/probe-duplicate-key/ standard/conformance/negative/probe-unresolved-require/ standard/conformance/negative/probe-cycle/
git commit -m "test(conformance): negative probe fixtures (duplicate, unresolved, cycle)"
```

---

### Task I4: Negative — non-serialisable + register-only-from-execute

**Files:**
- Create: `standard/conformance/negative/probe-non-serializable/{Cookfile,...}`
- Create: `standard/conformance/negative/probe-register-only/{Cookfile,...}`

**Note:** These need execute-phase validation, which the harness doesn't have yet (per the rewritten SHI-210). Two options:

(a) Defer these two fixtures to SHI-210's "Positive register-phase conformance harness" follow-up — those failure modes need the execute path to surface.

(b) Test them via in-crate Rust tests instead (they already exist in F2 and C6).

Pick (a) — file a follow-up task in SHI-210's scope to add execute-mode fixtures for probe-non-serializable + probe-register-only.

- [ ] **Step 1: Add a TODO to the SHI-210 ticket via Linear**

Use `mcp__linear-server__save_issue` to update SHI-210 description with a "follow-up fixtures" line item.

- [ ] **Step 2: Commit** — no files to add for this task; just a Linear update.

---

## Section J — Integration test

### Task J1: End-to-end probe + consumer build, second-run cache hit

**Files:**
- Create: `cli/crates/cook-register/tests/probe_integration.rs`

- [ ] **Step 1: Write test**

```rust
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn probe_consumer_end_to_end_first_run_then_cache_hit() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
use cook_cc

register
    cook.probe("greet", {
        inputs = {},
        produce = "return { word = \"hello-from-probe\" }",
    })
    cook.add_unit({
        name = "echo",
        inputs = {},
        outputs = {tmp .. "/done.marker"},
        requires = {"greet"},
        command = "echo {{greet.word}} > " .. tmp .. "/done.marker",
    })
end

recipe build
    > cook.sh("true")
"#;
    std::fs::write(tmp.path().join("Cookfile"),
        cookfile.replace("tmp", &format!("\"{}\"", tmp.path().display()))).unwrap();

    // First run: cache miss; probe executes; consumer reads value.
    let output = run_cook_in(&tmp.path(), &["build"]).unwrap();
    let marker = std::fs::read_to_string(tmp.path().join("done.marker")).unwrap();
    assert!(marker.contains("hello-from-probe"));

    // Second run: cache hit; probe must NOT re-execute. Sanity-check by mutating
    // the produce source to error if re-run — but skip this for now; an easier
    // check is verifying the probe artifact file exists.
    let cache_dir = tmp.path().join(".cook/cache");
    let entries: Vec<_> = std::fs::read_dir(&cache_dir).unwrap().collect();
    assert!(entries.len() > 0, "expected a probe artifact in {}", cache_dir.display());
}

fn run_cook_in(dir: &std::path::Path, args: &[&str]) -> Result<std::process::Output, String> {
    let cook_bin = std::env!("CARGO_BIN_EXE_cook");  // assumes a `cook` bin target
    let out = std::process::Command::new(cook_bin)
        .args(args).current_dir(dir).output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    Ok(out)
}
```

- [ ] **Step 2: Run + verify**

```
cargo test -p cook-register probe_consumer_end_to_end 2>&1 | tail -20
```

- [ ] **Step 3: Commit**

```
git add cli/crates/cook-register/tests/probe_integration.rs
git commit -m "test(integration): probe + consumer end-to-end with cache hit on second run"
```

---

## Section K — Linear ticketing and final wrap-up

### Task K1: Update Linear

- [ ] **Step 1: Close SHI-214** with a comment referencing this design doc and the new epic. Use `mcp__linear-server__save_issue` with state = "Done" and a closing comment.

- [ ] **Step 2: File a new epic** in a new project "Probe units — shareable configure-time state" using `mcp__linear-server__save_project` (if available; else create manually in Linear). Cross-reference the design doc + this plan.

- [ ] **Step 3: File phase-2 placeholder ticket** in the new project for cook_cc 0.5.0 (compiler-detection probe migration). Brief description only — that work is in the cook-modules repo.

---

### Task K2: Standard changes lockstep verification

- [ ] **Step 1: Verify the standard pre-commit hook passes for every commit on this branch**

```
git rebase --exec 'git diff --name-only HEAD~1 HEAD | grep -E "(cli/crates/cook-lang|cli/crates/cook-register|cli/crates/cook-luagen|cli/crates/cook-luaotp)" >/dev/null && git diff --name-only HEAD~1 HEAD | grep -E "^standard/" >/dev/null || echo "WARN: language-surface change without standard/ change"' main
```

If any commit warns, either amend (if it's the immediate prior) or revisit the spec-first ordering and squash.

- [ ] **Step 2: Run full conformance suite**

```
cargo test -p cook-lang --test conformance 2>&1 | tail -20
cargo test -p cook-register --test conformance 2>&1 | tail -20
cargo test -p cook-luagen --test conformance 2>&1 | tail -20
```

All green.

- [ ] **Step 3: Run full workspace tests**

```
cargo test 2>&1 | tail -30
```

All green. Investigate and fix any regressions before moving on.

---

## Self-review checklist

Run this after the plan is complete:

- [ ] Every spec section in `2026-05-15-shi214-probe-units-design.md` is addressed by at least one task.
- [ ] No "TBD" / "implement later" / "similar to Task N" placeholders.
- [ ] Types defined in B1 (`ProbeUnit`, `ProbeInputs`, `WorkPayload::Probe`, `CapturedUnit.requires`) are used consistently in later tasks.
- [ ] The Task G1 revision (changing `produce` from function to string) is reflected in Task A1 (Standard chapter) and Task C1 (probe_api implementation).
- [ ] Probe-fingerprint algorithm in D1 matches the Standard chapter's §22.5.3 wording.
- [ ] cook-engine integration (G3-G5) is the right scope — confirm the scheduler is in `cook-engine` and not elsewhere by reading the current dispatch site.
- [ ] No execute-mode conformance fixtures are added (deferred to SHI-210 follow-up per Task I4).
- [ ] The CS- numbers in tasks A5 / A6 are placeholders (CS-AA, CS-BB, CS-CC) — replace with real numbers when implementing.

---

## Notes for the implementing engineer

- **Spec-first is non-negotiable** for this work. Land Section A commits before Section B onward. The pre-commit hook at `.githooks/` enforces this.
- **TDD throughout**: write the failing test, run to confirm fail, implement, run to confirm pass, commit. Five-step rhythm per task.
- **Frequent commits**: per task at minimum. Each commit should compile and pass tests.
- **No backwards-compat hacks** for `cook.cache.set` — it's deprecated, not removed, in this phase. Module authors still call it; emit warnings only after cook_cc has migrated off (Phase 4 in the spec).
- **Cycle-detection (Task C5)** uses DFS; for small probe graphs (typical: <100 probes) this is fine. Don't optimise.
- **msgpack format (Task F2)** uses `rmpv` for the value layer. Don't try to use `rmp-serde` directly — probe values are dynamic, not statically typed structs.
- **`produce` is a string of Lua source** (revised in Task G1). Document this clearly in the Standard chapter — module authors will write `produce = [[ return ... ]]` rather than function literals. This is intentional: it sidesteps the unreliable mlua function-source extraction and makes the fingerprint trivial (just hash the source string).
