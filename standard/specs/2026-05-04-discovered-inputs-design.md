# Design: Discovered inputs and depfile-as-implicit-output

**Date:** 2026-05-04
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time)
**Predecessor:** [2026-05-02-cache-restore-and-dep-inputs-design.md](./2026-05-02-cache-restore-and-dep-inputs-design.md)
**Scope:** `cli/crates/cook-contracts`, `cli/crates/cook-cache`, `cli/crates/cook-fingerprint`, `cli/crates/cook-register`, `cli/crates/cook-engine`, `examples/lua-build/cook_modules/cpp.lua`, and amendments to Cook Standard §6 (Lua API) and §8 (Execution model).

## 1. Motivation

### 1.1. Observed behaviour

Compile units in `examples/lua-build` exhibit a three-run warmup before caching converges:

| Run | `.o` nodes | Linked artifacts |
|---|---|---|
| 1 (post-clean) | rebuild all | rebuild |
| 2 | rebuild all | cached |
| 3 | cached | cached |

The cause is a depfile bootstrap. `cpp.lua` registers each `.c → .o` unit with `inputs = [source]` because no `.d` file exists on the first invocation. The compile runs with `-MMD -MF <dep>` and emits the depfile as a side effect. On run 2, `cpp.lua` parses the now-existing depfile, registers the unit with `inputs = [source, h1, h2, ...]`. The new input set differs from run 1's stored set; `cook-fingerprint::check::check_inputs` returns `Rebuild(InputSetChanged)`. The recompile produces identical bytes (deterministic), so downstream link steps still hit. On run 3 the fat input set matches and all units settle into cache hits.

### 1.2. Why this matters for the cache backend

The 2026-05-02 spec (cross-recipe dep inputs and restore-on-hit) made the local `StepEntry.inputs` correct for cross-recipe dependencies. Compile units remain incorrect during the bootstrap window: a Run-1 entry's `inputs` is a strict subset of what the command actually consumed. Two consequences:

1. **Local cache wastes a build.** The Run-2 recompile is unnecessary work. For a small project the cost is one rebuild; for a large monorepo it can be tens of CPU-minutes.
2. **Cross-machine readiness is blocked.** Future cross-machine primary lookup (the missing piece for fresh-clone cache hits) requires that an uploaded artifact's input-content hashes reflect every file the command consumed. A thin Run-1 entry uploaded to a shared backend lures other machines into hits whose correctness depends on the source file alone — a soundness hole that grows with toolchain and sysroot drift.

The fix is to surface the post-execution-discovered input set as a first-class concept in the cache layer.

### 1.3. Language-agnosticism

Make-format depfiles originated in C/C++ but are the de-facto inter-tool convention: `gcc -MMD`, `clang -MD`, `swiftc --emit-dependencies`, `glslc -MD`, and Ninja's `deps = gcc` all consume them. `rustc --emit=dep-info` defaults to a Make-shaped variant. The design must avoid embedding C/C++ assumptions into the language. It exposes a generic "discovered inputs" surface with a format strategy parameter; `"make"` is the only built-in format today.

## 2. Non-goals

- **Cookfile grammar extension.** `cook_step` (§{recipes.cook-single-output}, §{recipes.cook-multi-output}) is unchanged. Discovered inputs is a Lua-API-only addition; module authors (`cpp.lua`, future `rust.lua`, `swift.lua`) are the consumers.
- **Cross-machine fresh-clone hits.** Primary cloud lookup on `NoCacheEntry` is a separate spec. This spec leaves `cloud_key` composition unchanged: post-execution upload uses the augmented (fat) input set; fresh-clone bootstrap remains a future-work item.
- **Dual-key uploads.** No "upload under both thin and fat keys" optimisation. That is the natural follow-up to primary cloud lookup.
- **Pluggable depfile-format registry.** The `format` field is reserved for future formats but only `"make"` is implemented. Adding a new format is an additive change at that point; the wire surface is forward-compatible.
- **Schema bump.** `StepEntry`'s on-disk shape is unchanged. `CACHE_VERSION` does not bump. Existing entries grow naturally via the InputSetChanged → rebuild self-heal path.
- **Trace-based discovery.** `strace`/`fsatrace`/BPF-based input discovery is out of scope. The design accommodates it (a future `format = "fsatrace"` would slot in cleanly) but doesn't implement it.

## 3. Architecture

### 3.1. Modules touched

```
cli/crates/
├── cook-contracts/
│   └── lib.rs              CacheMeta gains discovered_inputs: Option<DiscoveredInputs>
├── cook-cache/
│   ├── depfile.rs          NEW — Make-format parser
│   ├── manager.rs          record_completion appends discovered records to StepEntry.inputs
│   └── store.rs            unchanged on-disk schema
├── cook-fingerprint/
│   └── check.rs            needs_rebuild_cook augments current_inputs pre-check
├── cook-register/
│   └── unit_api.rs         cook.add_unit reads `discovered_inputs` table; populates CacheMeta
└── cook-engine/
    └── executor.rs         post-execution: parse depfile, augment StepEntry, upload depfile artifact

examples/
└── lua-build/cook_modules/cpp.lua    remove parse_depfile; pass discovered_inputs

standard/
├── src/content/docs/06-cook-lua-api.mdx     §6.2 add discovered_inputs field on cook.add_unit
└── src/content/docs/08-execution-model.mdx  §{exec.cache} amend with discovery semantics
```

### 3.2. Architectural invariants preserved

- **Single source of truth for "what the command read."** `StepEntry.inputs` post-augmentation includes both declared and discovered paths. No separate "declared vs discovered" field. Augmentation is symmetric: post-execution from the just-written depfile, pre-check from the prior depfile.
- **Cache miss > cache poison.** Any failure to read or parse the depfile post-execution falls back to a thin `StepEntry`. The cache never lies about discovered inputs.
- **Backend errors never fail the build.** Implicit depfile artifact upload follows the existing fire-and-forget logging pattern (2026-05-02 spec §3.2).
- **Schema unchanged.** `StepEntry` JSON layout is identical. Pre-amendment caches read transparently. Re-running over them produces a one-shot `InputSetChanged` rebuild for affected units, after which the entry's `inputs` is fat and steady-state caching resumes.

### 3.3. Build-time flow (amends 2026-05-02 §3.3)

```
8. For each unit:
     a. local hit (RecipeCache + on-disk outputs match)?      skip
     b. local entry matches but on-disk drift OR missing?     try Backend::get for each output
                                                              (incl. depfile artifact, if any);
                                                              if all hit, write to disk; skip
     c. else, cloud hit?                                       try Backend::get for each output;
                                                              if all hit, write to disk; skip
     d. else execute, then:
          i.   record_completion()                             local index update
          ii.  if discovered_inputs is some:
                 parse depfile; append discovered FileRecords  augment StepEntry.inputs
                 to step_entry.inputs and the stored entry
          iii. for each output, Backend::put()                 upload, fire-and-forget
          iv.  if discovered_inputs is some:                   upload depfile artifact under
                 Backend::put(artifact_key(cloud_k,            artifact_key index N (where N is
                 outputs.len(), depfile_path), depfile_bytes)  the unit's declared output count)
```

Step 8a's "RecipeCache match" check now invokes pre-check augmentation (§5.1) when the unit declares `discovered_inputs`. Steps 8d-ii and 8d-iv are new.

## 4. Data structures

### 4.1. `DiscoveredInputs` and `CacheMeta` extension

`cli/crates/cook-contracts/src/lib.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredInputs {
    /// Path (working-directory-relative) of the file the command emits to
    /// record the inputs it consumed. Treated as an implicit restorable
    /// output: uploaded under its own artifact_key alongside the user-declared
    /// outputs, restored on cache hit.
    pub from: String,

    /// Format strategy. Conforming implementations MUST support "make";
    /// MAY support others. The engine raises a register-phase error on
    /// an unrecognised value.
    pub format: String,
}

pub struct CacheMeta {
    // existing fields unchanged ...
    pub recipe_name: String,
    pub project_id: String,
    pub cookfile_path: String,
    pub cache_key: String,
    pub input_paths: Vec<String>,
    pub output_paths: Vec<String>,
    pub command_hash: u64,
    pub context_hash: u64,
    pub env_contribution: u64,
    pub consulted_env: BTreeMap<String, String>,

    // NEW
    pub discovered_inputs: Option<DiscoveredInputs>,
}
```

`output_paths` does not include the depfile path. The depfile is tracked through `discovered_inputs.from` and surfaces in `StepEntry.outputs` (so the existing restore-on-hit path walks it) but is invisible at the Cook Lua API surface (`outputs[]` bindings inside `using >{...}` blocks per §{lua.using-block-globals} continue to enumerate user-declared outputs only).

### 4.2. `StepEntry` — schema unchanged

`cli/crates/cook-cache/src/store.rs`:

```rust
pub struct StepEntry {
    pub inputs: Vec<FileRecord>,   // declared + discovered, merged
    pub outputs: Vec<FileRecord>,  // declared outputs + depfile (when discovered_inputs.is_some())
    pub command_hash: u64,
    pub context_hash: u64,
    pub env_contribution: u64,
}
```

No new fields. `CACHE_VERSION` is unchanged.

### 4.3. Depfile parser

`cli/crates/cook-cache/src/depfile.rs` (new):

```rust
/// Parse a Make-format depfile. Returns paths in input order, deduped,
/// matching the filter rules below.
///
/// Filter rules:
///   - Strip the leading target text up to and including the first ':'.
///   - Join continuation lines (`\\\n` and `\\\r\n`).
///   - Skip entries beginning with `/` (absolute paths — system headers).
///   - Skip entries equal to `source_path` (the unit's primary input,
///     already present in the declared inputs).
///   - Skip entries whose paths do not exist on disk relative to
///     `working_dir` (stale depfile).
pub fn parse_make_depfile(
    depfile_path: &Path,
    source_path: &str,
    working_dir: &Path,
) -> Result<Vec<String>, DepfileError>;

pub enum DepfileError {
    NotFound,
    Io(io::Error),
    Malformed { byte_offset: usize, reason: String },
}
```

The parser is content-only: it does not stat, hash, or open the listed paths beyond the existence check. The caller (engine commit path or check path) feeds the result into `collect_records` for hashing.

### 4.4. `cook.add_unit` register-phase reading

`cli/crates/cook-register/src/unit_api.rs`:

The `cook.add_unit` closure reads:

```lua
discovered_inputs = {
    from = "<path>",
    format = "make",
}
```

Validation (register-phase Lua errors):

- `discovered_inputs` MUST be a Lua table when present.
- `from` MUST be a non-empty string.
- `from` MUST be a relative path (no leading `/`); the engine rejects absolute paths and paths that escape `working_dir` via `..` segments.
- `format` MUST be a string. The engine accepts `"make"` and raises on any other value with the message:

  > `cook.add_unit: discovered_inputs.format = "<x>" is not supported by this implementation (supported: "make")`

The reader populates `CacheMeta.discovered_inputs`. It does not read or parse the depfile at register time.

## 5. Algorithms

### 5.1. Pre-check augmentation

`cli/crates/cook-fingerprint/src/check.rs::needs_rebuild_cook` gains a new optional parameter alongside `restore_ctx`:

```rust
pub fn needs_rebuild_cook(
    entry: Option<&StepEntry>,
    current_inputs: &[&str],
    current_outputs: &[&str],
    command_hash: u64,
    context_hash: u64,
    env_contribution: u64,
    working_dir: &Path,
    restore_ctx: Option<&RestoreCtx>,
    discovered_inputs: Option<&DiscoveredInputs>,  // NEW
) -> (RebuildResult, Option<StepEntry>);
```

Immediately before the existing `check_inputs` call, the function performs:

```rust
let augmented: Vec<&str>;
let current_inputs_for_check: &[&str] = if let Some(di) = discovered_inputs {
    let source_for_skip = current_inputs.first().copied().unwrap_or("");
    match parse_make_depfile(&working_dir.join(&di.from), source_for_skip, working_dir) {
        Ok(discovered_paths) => {
            augmented = current_inputs
                .iter()
                .copied()
                .chain(discovered_paths.iter().map(String::as_str))
                .collect();
            &augmented
        }
        Err(_) => current_inputs,
    }
} else {
    current_inputs
};

let updated_inputs = match check_inputs(&entry.inputs, current_inputs_for_check, working_dir) {
    Err(reason) => return (RebuildResult::Rebuild(reason), None),
    Ok(u) => u,
};
```

Plate steps (`needs_rebuild_plate`) are unaffected — they don't carry discovered inputs.

### 5.2. Post-execution augmentation and upload

`cli/crates/cook-engine/src/executor.rs` (both call sites: interactive ~1052 and worker ~1219):

After `record_completion()` succeeds, before composing `cloud_key`:

```rust
if let Some(di) = &meta.discovered_inputs {
    let abs_depfile = dag.node(id).payload().working_dir.join(&di.from);
    let source_for_skip = meta.input_paths.first().map(String::as_str).unwrap_or("");
    match parse_make_depfile(&abs_depfile, source_for_skip, &dag.node(id).payload().working_dir) {
        Ok(discovered_paths) => {
            let discovered_records = collect_records(
                &discovered_paths,
                &dag.node(id).payload().working_dir,
            );
            for record in discovered_records {
                step_entry.inputs.push(record);
            }
            cm.replace_step_entry(&meta.recipe_name, &meta.cache_key, &step_entry);
        }
        Err(e) => {
            tracing::warn!("discovered-inputs: depfile parse failed for {}: {e}", di.from);
        }
    }
}
```

`replace_step_entry` is a new method on `CacheManager`:

```rust
pub fn replace_step_entry(&self, recipe_name: &str, cache_key: &str, entry: &StepEntry);
```

It overwrites the entry the engine just wrote via `record_completion`, under the existing dirty-set tracking so the recipe cache file is rewritten on flush.

After the augmentation block, the existing `cloud_key` composition runs over the augmented `step_entry.inputs`. The existing per-output upload loop (2026-05-02 §5.1) runs unchanged. After it, a new block uploads the depfile:

```rust
if let Some(di) = &meta.discovered_inputs {
    let depfile_idx = meta.output_paths.len() as u32;
    let abs_depfile = dag.node(id).payload().working_dir.join(&di.from);
    if let Ok(bytes) = std::fs::read(&abs_depfile) {
        let artifact_k = artifact_key(&cloud_k, depfile_idx, &di.from);
        let artifact_meta = ArtifactMeta {
            recipe_namespace: recipe_namespace.clone(),
            command_hash: meta.command_hash,
            context_hash: meta.context_hash,
            env_contribution: meta.env_contribution,
            schema_version: CACHE_VERSION,
            size_bytes: bytes.len() as u64,
            tags: BTreeSet::new(),
            consulted_env_keys: meta.consulted_env.keys().cloned().collect(),
            output_index: depfile_idx,
            output_path: di.from.clone(),
        };
        if let Err(e) = cache_ctx.backend.put(&artifact_k, &bytes, &artifact_meta) {
            tracing::warn!("cache backend put failed for depfile {}: {e}", di.from);
        }
    }
}
```

The depfile is also recorded in `step_entry.outputs` so `try_restore` (2026-05-02 §5.2) walks it on a future check. `record_completion` is amended:

```rust
let mut new_outputs = collect_records(&meta.output_paths, working_dir)?;
if let Some(di) = &meta.discovered_inputs {
    if let Some(rec) = file_record_for(&di.from, working_dir) {
        new_outputs.push(rec);
    }
}
```

### 5.3. Restore-on-hit interaction

The 2026-05-02 spec's `try_restore` walks `entry.outputs` and pulls bytes for any drift-or-missing output. Because §5.2 adds the depfile to `entry.outputs`, `try_restore` walks it like any other output: missing-on-disk + present-in-backend → restored.

This is the path that handles the "user wiped `.cook/deps/` but kept `.cook/cache.json`" case:

1. Pre-check augmentation tries to read the depfile → fails (missing) → returns thin `current_inputs`.
2. `check_inputs` sees thin current vs fat entry → would return `InputSetChanged`.
3. **Optional refinement (see §10):** before returning `InputSetChanged`, check whether the missing path is the depfile and attempt to restore it from the backend; if successful, retry pre-check augmentation.

This refinement is left as a future item; the baseline design accepts the rebuild as an acceptable self-heal.

### 5.4. cloud_key composition

`cloud_key` is composed from `sorted([fr.hash for fr in step_entry.inputs])` exactly as today. After §5.2, that includes discovered hashes. The existing `machine_a_uploads_thin_entry_machine_b_pulls_correctly` test in `cli/crates/cook-cache/tests/integration_first_build_depfile.rs` continues to assert key-composition stability; with this design that test now matches end-to-end behaviour rather than an isolated key invariant.

Cross-machine fresh-clone hits remain a future concern. This spec does not introduce dual-key uploads or primary cloud lookup; the existing flow uploads a single artifact set under a single (fat) `cloud_key`.

### 5.5. Failure modes

| Scenario | Behaviour |
|---|---|
| Depfile missing post-execution | Warn; persist thin `StepEntry`; next check rebuilds and self-heals. |
| Depfile malformed post-execution | Same as missing — warn, thin `StepEntry`. |
| Depfile lists nonexistent paths | Parser filters them out; they don't appear in `StepEntry.inputs`. |
| Depfile missing pre-check | Augmentation no-ops; `check_inputs` returns `InputSetChanged` → rebuild → self-heal. |
| `discovered_inputs.format` unknown | Register-phase Lua error before any work runs. |
| `discovered_inputs.from` is absolute or escapes working_dir | Register-phase Lua error. |
| Backend `put` fails for the depfile | Warn and continue (existing fire-and-forget contract). |
| Backend `get` fails during restore-on-hit for the depfile | `try_restore` returns `false` for that output; falls back to rebuild. |

## 6. Module slim-down: `cpp.lua`

Two changes in `examples/lua-build/cook_modules/cpp.lua` (and parallel changes in any sibling cpp.lua copies under `examples/`):

1. Remove `local function parse_depfile(...)` and its three call sites at lines 329, 578, 824. The engine now owns Make-format parsing.
2. In `cpp.compile` and the C++20 module-build paths, replace the manual `parse_depfile` + augmented-`inputs` construction with a `discovered_inputs` table on the `cook.add_unit` call:

```lua
cook.add_unit({
    inputs = { source },
    output = obj_out,
    command = cmd,
    discovered_inputs = {
        from = dep_file,
        format = "make",
    },
})
```

After this slim-down, `cpp.lua` no longer reads or parses depfiles itself. The unit's declared `inputs` shrinks back to just the source(s), and the engine handles input augmentation.

The other consumers of `parse_depfile` (lines 578, 824 — module-bmi compile paths) follow the same pattern.

## 7. Cook Standard amendments

### 7.1. §6 (Cook Lua API)

`standard/src/content/docs/06-cook-lua-api.mdx` §6.2 (`cook.add_unit`) adds a row to the field table:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `discovered_inputs` | table | absent | Declares a file the command writes during the execute phase that records additional input paths the command consumed (§{exec.cache.discovered-inputs}). Subfields: `from` (string, required, working-dir-relative path) and `format` (string, required; the only mandatory format is `"make"`). |

A new normative subsection (§6.2.X) formalises the `from` and `format` semantics from §4.4 of this design.

### 7.2. §8 (Execution model)

`standard/src/content/docs/08-execution-model.mdx` §{exec.cache} gains a new normative subsection §{exec.cache.discovered-inputs}:

> A unit whose declaration carries `discovered_inputs` (§{lua.add-unit}) participates in post-execution input discovery. When such a unit is checked against an existing cache entry, a conforming implementation:
>
> 1. **MUST** attempt to read the file at `discovered_inputs.from` from the unit's working directory before composing the input-content set passed to the cache check. If the file is present and well-formed under `discovered_inputs.format`, the implementation **MUST** union the discovered paths with the unit's declared `inputs[]` for the purposes of the check.
> 2. **MUST** treat a missing or malformed discovery file as no augmentation — the check proceeds with the thin (declared-only) input set. A subsequent rebuild **MUST** regenerate the discovery file.
> 3. **MUST**, after a successful execution, parse the discovery file at `discovered_inputs.from` and amend the recorded `StepEntry`'s input set to include the discovered paths before persisting the entry to the local cache index.
> 4. **MUST** treat the discovery file as an implicit cache artifact: uploaded under its own artifact key during commit, restored from the backend on a hit-with-drifted-outputs check (§{exec.cache.restore-on-hit}).
>
> The `"make"` discovery format is the Make rule format produced by GCC's `-MMD` flag and equivalent compiler options. The conforming parser **MUST**: strip the leading target text up to and including the first `:`; join continuation lines (`\\\n` and `\\\r\n`); ignore entries beginning with `/`; ignore the unit's first declared input; ignore entries whose paths do not exist on disk.

### 7.3. No grammar change

The `cook_step` grammar (Appendix A §{grammar-appendix.steps}) is unchanged. No new top-level keyword is introduced.

## 8. Backwards compatibility

- **On-disk cache:** `StepEntry` shape is identical. v3/2026-05-02 caches written before this addendum remain readable. Compile units that previously stored thin entries see their entries grow on the next rebuild via the existing `InputSetChanged` self-heal path — a one-shot rebuild per affected unit, then steady-state caching.
- **Artifact store:** unchanged layout (`<aa>/<bb…>`). Pre-amendment uploads coexist; new depfile uploads use a fresh `artifact_key` slot at index `outputs.len()` and don't collide.
- **Backend trait:** unchanged. The `LocalBackend` and any in-flight cloud backends pick up depfile artifact uploads/restores transparently.
- **`CacheMeta` ABI:** the new `discovered_inputs: Option<DiscoveredInputs>` field defaults to `None` for any unit that doesn't opt in. Existing call sites compile unchanged after `..Default::default()` cleanup.
- **Cook Lua API:** purely additive. Cookfiles that don't pass `discovered_inputs` see no behaviour change.

## 9. Test plan

### 9.1. Unit tests

- **`cook-cache::depfile`** — `parse_make_depfile` against fixtures: well-formed single-line, multi-line with `\\` continuations, source-self-skip, absolute-path skip, nonexistent-path skip, malformed input (no colon) returns `Malformed`, missing file returns `NotFound`.
- **`cook-fingerprint::check`** — `needs_rebuild_cook` with `discovered_inputs` configured:
  - depfile present, augmented current matches stored fat entry → `Skip`
  - depfile present, one header content drifts → `Rebuild(InputChanged)`
  - depfile missing, stored entry is fat → `Rebuild(InputSetChanged)`
  - depfile present, malformed → same as missing
- **`cook-register::unit_api`** — `cook.add_unit` validation:
  - `discovered_inputs = { from = "x.d", format = "make" }` populates `CacheMeta.discovered_inputs`
  - missing `from` → register-phase Lua error
  - unknown `format = "ninja"` → register-phase Lua error with the documented message
  - absolute or `..`-escaping `from` → register-phase Lua error

### 9.2. Integration tests

- **`cook-cache/tests/integration_discovered_inputs_warmup.rs`** (new) — three-run scenario at the cache layer:
  1. Run 1: `needs_rebuild_cook` returns `NoCacheEntry`; simulated execute writes a depfile and stores a `StepEntry` post-augmentation with fat inputs.
  2. Run 2: `needs_rebuild_cook` augments `current_inputs` from the depfile and returns `Skip`.
  3. Header content edit; Run 3: returns `Rebuild(InputChanged)`.
- **`cook-cache/tests/integration_discovered_inputs_restore.rs`** (new) — restore-on-hit with an augmented `StepEntry`:
  1. Build (simulated) and persist; verify the depfile artifact is in the backend.
  2. Delete the depfile and one declared output from disk.
  3. Re-check; verify both files are restored from the backend, return `Skip`.

### 9.3. End-to-end (`examples/cache_benchmarks` and `examples/lua-build`)

- `examples/cache_benchmarks/verify.sh` gains a scenario asserting the warmup collapses from three runs to two:

  ```
  scenario 14 — discovered-inputs warmup
    cook clean
    cook                    # all .o nodes rebuilt; depfiles written
    cook                    # all .o nodes cached (was: rebuilt under prior behaviour)
  ```

- `examples/lua-build` is the natural canary. After the cpp.lua slim-down, all existing recipes (`liblua`, `lua`, `luac`, `build`, `test`, `compile-commands`) MUST continue to pass.

### 9.4. Conformance

`standard/conformance/cookfiles/` gains a small Cookfile that exercises `cook.add_unit({ ..., discovered_inputs = ... })` and asserts the augmented `StepEntry.inputs` shape via the existing cache-state assertion harness. This is a new fixture, not a new harness capability.

## 10. Open questions

None blocking implementation.

1. **Restore depfile mid-check — RESOLVED during Task 13.** During implementation, the integration test for restore-on-hit revealed that without this refinement, a partial wipe of `.cook/deps/` paired with an intact local entry burns an avoidable rebuild. The adopted approach: when the pre-check depfile parse fails AND a `restore_ctx` is available, substitute the stored entry's input set so the outputs walk and restore path can proceed. The refinement is gated on `restore_ctx.is_some()` so the no-backend path retains the original baseline self-heal. See §{exec.cache.discovered-inputs} clause 2 for the normative wording.
2. **Future format strategies.** `format = "rustc-dep-info-json"`, `format = "ninja-deps"`, `format = "fsatrace"`. Each is purely additive; specifying them is left to the spec that introduces the corresponding language module.
3. **Eviction of pre-amendment thin-key artifact uploads.** Any uploads made before this design's commit path landed are correctly addressed under their own `artifact_key`s; they are reachable but orphaned once the unit's entry grows fat. Eviction policy work owns this cleanup.

## Appendix A. Worked example: `lua-build` warmup collapse

Pre-amendment behaviour (observed):

```
$ cook clean && cook
    Finished in 0.45s   (37 nodes, 0 cached)
$ cook
    Finished in 0.41s   (37 nodes, 3 cached)     # only links cached
$ cook
    Finished in 0.02s   (37 nodes, 37 cached)
```

Post-amendment expected behaviour:

```
$ cook clean && cook
    Finished in <unchanged>   (37 nodes, 0 cached)     # full rebuild; depfiles written
$ cook
    Finished in <fast>        (37 nodes, 37 cached)    # all hit on second run
```

The first run is unchanged: there is no prior cache to consult, every node executes, and depfiles are written as a side effect of `gcc -MMD`. The second run augments each compile unit's `current_inputs` from its depfile, the augmented set matches the stored `StepEntry.inputs`, and every node hits — collapsing the prior three-run warmup to two.
