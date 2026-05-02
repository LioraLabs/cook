# Design: Cache restore-on-hit, per-output artifacts, cross-recipe dep inputs

**Date:** 2026-05-02
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time)
**Linear epic:** SHI-140 follow-up — *Addendum to cache cloud-readiness*
**Predecessor:** [2026-05-01-cache-cloud-readiness-design.md](./2026-05-01-cache-cloud-readiness-design.md)
**Scope:** `cli/crates/cook-cache`, `cli/crates/cook-register`, `cli/crates/cook-engine`, `examples/cache_benchmarks`, and amendments to the 2026-05-01 spec §2 and §3.3.

## 1. Motivation

The 2026-05-01 cache cloud-readiness design ("v3") shipped per-step keying, machine/tool identity in `context_hash`, an env denylist contribution, a `CacheBackend` trait, and a `LocalBackend` that already mirrors uploaded artifacts to a content-addressed sharded store at `~/.cache/cook/cloud/<aa>/<bb…>`. The end-to-end verification fixture (`examples/cache_benchmarks`) caught three observable gaps that v3 did not close:

1. **Cross-recipe dependency outputs are absent from the consuming recipe's `cache_meta.input_paths`.** When the `demo` recipe references `{greet}` and `{util}` in its using-string, codegen substitutes those tokens with `cook.dep_output("greet")` / `cook.dep_output("util")`. Those calls accumulate dep edges into the DAG (`step_group_dep_refs`), but the resolved output paths never land in the consuming unit's `inputs[]`. demo's `StepEntry.inputs` is empty; the cache check has no path to compare hashes against. demo "hits" whenever its (empty) declared input set and its output file (left over from a prior run) match — even if `build/greet.o` content has drifted underneath. This is a correctness bug, not a performance one: **dep output drift is silently ignored.**
2. **Variant toggle on a shared output path triggers `OutputChanged` rebuild even when the prior variant's cache entry exists.** Two configs (`debug`, `release`) routing to the same `build/greet.o` produce two distinct local cache *entries* (per spec §5.1; the `cache_key` embeds `context_hash:env_contribution`) — but on disk the file holds whichever variant ran last. Toggling back to the prior variant finds the cache entry but mismatching on-disk content; the engine rebuilds. The cache cloud-readiness spec §3.3 step 8b already specifies "cloud hit? `Backend::get` + extract artifact bytes; skip" — restore-on-hit is the design intent — but the *local* hit path never consults `Backend::get`, so it can't restore bytes that the local sharded store already holds.
3. **Multi-output recipes upload only the first output's bytes.** v3 keys artifacts by `cloud_key`, with one artifact per cache entry. A recipe that produces `foo.txt` and `bar.txt` uploads `foo.txt`'s bytes; a future cache hit (cloud or local-restore) can recover `foo.txt` but not `bar.txt`. v3 framed this as a non-goal pending "manifest-style multi-output upload." That framing assumes a single artifact-per-entry shape; lifting that assumption — by deriving an independent artifact key per output — dissolves the limitation without introducing a manifest format.

A fourth concern, surfaced during scope review: monorepo Cookfiles imported via the `import` keyword (§{cook.imports}) MUST cache correctly across cookfile boundaries. The cross-cookfile collision tests in commit `ccfc551` cover collision; they do not exercise the **cross-cookfile cache hit** path.

## 2. Non-goals

- **Manifest-format upload.** §5 derives a per-output artifact key; one logical cache entry produces N independent artifacts in the backend. No manifest blob, no archive format. Backends implement the `CacheBackend` trait unchanged.
- **Cloud backend.** SHI-24 still owns the network implementation. This addendum keeps the trait surface and makes it correct.
- **Restoring outputs that the user has explicitly deleted.** A missing output file is `OutputMissing`, not `OutputChanged`. Both fall through to the same restore-attempt path; this is intentional — a user who deleted `build/foo.o` *wants* it back, and the cache can give it back without re-running the command.
- **Restoring outputs across `command_hash`/`context_hash`/`env_contribution` mismatches.** Restore-on-hit applies only when the cache entry matches the current step on all those keys. A mismatch is still a real rebuild.
- **Auto-rewriting user output paths to be variant-scoped.** The recommendation in `examples/cache_benchmarks/README.md` ("use `build/{config_name}/greet.o`") remains a valid authoring practice, not a requirement. With restore-on-hit, the workaround becomes optional rather than load-bearing.

## 3. Architecture

### 3.1. Modules touched

```
cli/crates/
├── cook-cache/
│   ├── backend.rs       artifact_key() helper added
│   ├── check.rs         needs_rebuild_cook gains restore-attempt path
│   └── store.rs         StepEntry unchanged on disk; recipe_namespace borrowed
├── cook-register/
│   ├── dep_output_api.rs   cook.dep_output / cook.dep_output_list unchanged in shape
│   ├── unit_api.rs         add_unit appends resolved dep paths to cache_meta.input_paths
│   └── engine.rs           threads SharedTerminalOutputs into register_unit_api
└── cook-engine/
    └── executor.rs      commit path uploads N artifacts (one per output)
```

No Cookfile grammar change. No Cook Lua API change. No on-disk schema bump (the `StepEntry` shape is unchanged; only the meaning of `inputs[]` for steps with cross-recipe deps grows to include the resolved paths).

### 3.2. Architectural invariants preserved

- **Local-correct ⊕ cloud-correct.** A `StepEntry` whose `inputs[]` accurately reflects every file the command consumed is correct to upload as-is. Folding cross-recipe dep outputs into `inputs[]` brings the local `StepEntry` into the same correctness shape that cloud restore needs.
- **Cache miss > cache poison.** Restore-on-hit fails closed: a `Backend::get` miss for any of the N output artifacts falls through to a real rebuild. The engine never proceeds with a partial restore.
- **Backend errors never fail the build.** `Backend::get` and `Backend::put` errors are logged and the build continues with the rebuild path.

### 3.3. Build-time flow (amends 2026-05-01 §3.3)

```
8. For each unit:
     a. local hit (RecipeCache + on-disk outputs match)?      skip
     b. local entry matches but on-disk drift OR missing?     try Backend::get for each output;
                                                              if all hit, write to disk; skip
     c. else, cloud hit?                                       try Backend::get for each output;
                                                              if all hit, write to disk; skip
     d. else execute, then:
          i.   record_completion()                             local index update
          ii.  for each output, Backend::put()                 upload, fire-and-forget
```

Step 8b is new. Step 8d-ii now uploads N artifacts, one per output (was: first output only).

## 4. Data structures

### 4.1. Per-output artifact key

`cli/crates/cook-cache/src/backend.rs`:

```rust
/// Derive an output-scoped artifact key from a cache entry's cloud_key.
///
/// A logical cache entry (one (recipe, command, context, env, inputs) tuple)
/// MAY produce multiple output artifacts. Each artifact MUST be addressable
/// independently in the backend so a cache hit can restore all of them.
///
/// Composition: SHA-256(cloud_key || u32_le(output_index) || output_path_bytes)
///
/// The 0x01 delimiter (after cloud_key) is omitted because cloud_key is a
/// fixed 32-byte value and the index/path tail follows it directly.
pub fn artifact_key(
    cloud_key: &CloudKey,
    output_index: u32,
    output_path: &str,
) -> CloudKey {
    let mut h = Sha256::new();
    h.update(cloud_key);
    h.update(output_index.to_le_bytes());
    h.update(output_path.as_bytes());
    h.finalize().into()
}
```

The `output_path` is included so that two outputs at indices `0` and `1` with paths swapped produce different artifact keys — defense-in-depth against a recipe re-ordering its declared outputs without otherwise changing.

### 4.2. `ArtifactMeta` extension

```rust
pub struct ArtifactMeta {
    // existing fields ...
    pub recipe_namespace: String,
    pub command_hash: u64,
    pub context_hash: u64,
    pub env_contribution: u64,
    pub schema_version: u32,
    pub size_bytes: u64,
    pub tags: BTreeSet<String>,
    pub consulted_env_keys: BTreeSet<String>,

    // NEW: which output this artifact is for.
    pub output_index: u32,
    pub output_path: String,
}
```

`output_index` and `output_path` are written into the sidecar `.meta.json` so a future tool inspecting the local store can see which on-disk file each artifact corresponds to. They do **not** participate in any equality comparison the engine performs — the artifact key is the identity; these fields are diagnostic.

### 4.3. Cross-recipe dep input recording

`cli/crates/cook-register/src/unit_api.rs`:

`register_unit_api` gains a `terminal_outputs: SharedTerminalOutputs` parameter. Inside the `cook.add_unit` closure, after the existing `inputs` collection from the Lua table:

```rust
// Resolve cross-recipe dep refs accumulated by cook.dep_output / dep_output_list.
// These are paths the command consumed via {NAME} substitution; they MUST
// participate in the cache key and the input-content check, but MUST NOT
// land in WorkPayload.inputs (which drives _cook_in iteration / Lua-visible
// inputs and is scoped to the recipe's own iteration source).
let dep_input_paths: Vec<String> = {
    let state = cs.borrow();
    let to = terminal_outputs.borrow();
    state
        .step_group_dep_refs
        .iter()
        .filter_map(|name| to.get(name))
        .flat_map(|paths| paths.iter().cloned())
        .collect()
};

// Append to cache_meta.input_paths only.
let cache_input_paths: Vec<String> = inputs
    .iter()
    .cloned()
    .chain(dep_input_paths.iter().cloned())
    .collect();
```

`cache_input_paths` feeds `CacheMeta.input_paths`; the original `inputs` continues to feed `WorkPayload`. Determinism: `step_group_dep_refs` is already a `Vec<String>` with insertion-order dedup; `terminal_outputs` returns `Vec<String>` per recipe in the order they were registered. The resulting `cache_input_paths` is therefore stable for a given Cookfile evaluation.

## 5. Algorithms

### 5.1. Commit path (cache miss → execute → upload)

`cli/crates/cook-engine/src/executor.rs` (both call sites at lines 490-540 and 632-677):

```rust
// After record_completion() succeeds and step_entry is built:
let cloud_k = cook_cache::backend::cloud_key(&key_inputs);

for (i, output_path) in meta.output_paths.iter().enumerate() {
    let abs_output = working_dir.join(output_path);
    let bytes = match std::fs::read(&abs_output) {
        Ok(b) => b,
        Err(_) => continue, // missing output already logged elsewhere
    };
    let artifact_k = cook_cache::backend::artifact_key(&cloud_k, i as u32, output_path);
    let artifact_meta = ArtifactMeta {
        recipe_namespace: recipe_namespace.clone(),
        command_hash: meta.command_hash,
        context_hash: meta.context_hash,
        env_contribution: meta.env_contribution,
        schema_version: cook_cache::store::CACHE_VERSION,
        size_bytes: bytes.len() as u64,
        tags: BTreeSet::new(),
        consulted_env_keys: meta.consulted_env.keys().cloned().collect(),
        output_index: i as u32,
        output_path: output_path.clone(),
    };
    if let Err(e) = cache_ctx.backend.put(&artifact_k, &bytes, &artifact_meta) {
        tracing::warn!("cache backend put failed for {output_path}: {e}");
    }
}
```

Failure of any individual upload is logged; the build proceeds. Idempotency of `Backend::put` (per the trait contract) means re-running the same command produces identical bytes for identical artifact keys — no observable difference.

### 5.2. Restore path (local entry matches, on-disk output drifted or missing)

`cli/crates/cook-cache/src/check.rs` `needs_rebuild_cook`:

The current function returns `Rebuild(OutputChanged)` or `Rebuild(OutputMissing)` directly when output content/existence checks fail. The new behavior wraps those returns in a restore attempt:

```rust
// Existing flow walks entry.outputs and returns OutputChanged/OutputMissing
// on any mismatch. New behavior collects the mismatched indices instead
// and routes them through restore_outputs() before falling back to rebuild.

let mut needs_restore: Vec<usize> = Vec::new();
for (i, (cached_out, rel_path)) in entry.outputs.iter().zip(current_outputs.iter()).enumerate() {
    let abs = working_dir.join(rel_path);
    if !abs.exists() {
        needs_restore.push(i);
        continue;
    }
    if let (Some(disk_mtime), Some(disk_hash)) = (stat_mtime(&abs), hash_file(&abs)) {
        if disk_mtime != cached_out.mtime && disk_hash != cached_out.hash {
            needs_restore.push(i);
        }
    }
}

if !needs_restore.is_empty() {
    match try_restore(restore_ctx, entry, current_outputs, &needs_restore, working_dir) {
        RestoreResult::Restored => {} // continue to input-content check below
        RestoreResult::PartialMiss => return (Rebuild(OutputChanged), None),
        RestoreResult::OutputMissing => return (Rebuild(OutputMissing), None),
    }
}
```

`try_restore` recomposes `cloud_key` from `(StepEntry hashes, recipe_namespace, sorted input content hashes)`, then for each index in `needs_restore`:

1. Compute `artifact_key_i = artifact_key(&cloud_key, i, current_outputs[i])`.
2. Call `restore_ctx.backend.get(&artifact_key_i)`.
3. On `Some(bytes)` → write atomically (tmp + rename) to `working_dir.join(current_outputs[i])`.
4. On `None` or error → return `PartialMiss`.

After all required restores succeed, the function continues to the existing input-content check; the entry then returns `Skip` if inputs also match.

`RestoreCtx` carries borrows of `&dyn CacheBackend` and `&str` (recipe_namespace) and is the only signature change to `needs_rebuild_cook`. Plate steps (`needs_rebuild_plate`) are unaffected — they have no outputs to restore.

### 5.3. `cloud_key` recomposition at check time

The `recipe_namespace` is **not** stored on disk in `StepEntry`; it is recomputed from `CacheMeta` at every check site using the same formula as commit:

```
recipe_namespace = format!("{}/{}::{}",
    meta.project_id, meta.cookfile_path, meta.recipe_name)
```

`CacheMeta` already carries all three fields (added in v3). The check site has access to `CacheMeta` because it owns the cache lookup. `sorted_input_content_hashes` is built from `entry.inputs[].hash` after the input-content check confirms they match disk — but for restore we need this *before* the input-content check. The input-content check that already runs (`check_inputs`) populates an `updated` `Vec<FileRecord>`; we move its execution above the output check so its hashes are available, then proceed with output check + restore.

This is a code-ordering change inside `needs_rebuild_cook`, not a semantic one. `InputChanged`/`InputSetChanged` still short-circuit before any restore work happens, so we never restore against stale inputs.

## 6. Monorepo / `import` keyword cache verification

The `import` keyword (lexer.rs:178, workspace.rs) loads a sibling Cookfile under a namespace prefix. A parent recipe consuming `lib.build` (where `lib` is the import name) gets the imported recipe's outputs through the same `cook.dep_output(name)` pathway used for same-Cookfile deps — the registered name is the namespaced form `lib.build`.

The fix in §4.3 is therefore agnostic to whether the dep is in-Cookfile or imported, *provided* `terminal_outputs` is keyed by the same name the dep ref uses. The current `engine.rs` registration MUST verify this. If imported recipes register under their bare names (`build`) while consumers reference them as `lib.build`, the lookup misses and the fix degrades silently. **The implementation MUST add a regression test that fails if this naming alignment breaks.**

The cache_benchmarks fixture is extended with a subdirectory:

```
examples/cache_benchmarks/
├── Cookfile                # imports lib; demo links lib.lib_build outputs
├── lib/
│   └── Cookfile            # recipe lib_build → produces lib/build/lib.o
└── verify.sh
```

New scenarios verify:

| # | Scenario | Expected |
|---|---|---|
| 10 | `cook demo` with imported lib | first run executes both `lib.lib_build` and `demo`; second run hits both. |
| 11 | Touch `lib/src/lib.c` content | `lib.lib_build` rebuilds; `demo` rebuilds with `InputChanged("lib/build/lib.o")` — proves §4.3 cross-recipe dep wiring works across imports. |
| 12 | Variant toggle on imported recipe | toggling debug↔release for a parent that consumes `lib.lib_build` re-hits the prior cache entry AND restores `lib/build/lib.o`'s bytes — proves §5.2 restore-on-hit works across imports. |
| 13 | `pair` recipe round-trip | first run uploads two artifacts (`foo.txt` + `bar.txt`); `rm -rf build/pair && cook pair` restores both files from the local store with no command execution. |

## 7. Test plan

### 7.1. Unit tests

- `cook-cache/src/backend.rs` — `artifact_key` is deterministic; differs on `(cloud_key, index, path)` permutations.
- `cook-cache/src/check.rs` — `needs_rebuild_cook` with a populated backend restores on-disk content and returns `Skip`; with an empty backend returns the existing `Rebuild(OutputChanged)`.
- `cook-register/src/unit_api.rs` — `add_unit` with a populated `SharedTerminalOutputs` and one accumulated dep ref appends the dep's outputs to `cache_meta.input_paths` but not to `WorkPayload.inputs`.

### 7.2. Integration tests

- `cook-cache/tests/integration_restore_on_hit.rs` — full commit → mutate disk → check → assert `Skip` and disk content restored.
- `cook-cache/tests/integration_multi_output_restore.rs` — pair-style recipe; both files restored.
- `cook-engine` or `cook-cli` test that loads a workspace with one import, executes a parent recipe consuming `lib.foo`, then mutates `lib/build/foo.o` and asserts the parent rebuilds with `InputChanged("lib/build/foo.o")`.

### 7.3. End-to-end (cache_benchmarks)

`verify.sh` gains scenarios 10–13 above. The script MUST exit non-zero if any scenario fails. The README.md "Findings caught by this fixture" section is updated: the cross-recipe-dep finding moves from "Documented limitations" to "Fixed during verification"; the multi-output finding moves to "Fixed during verification"; the output-stomping finding moves to "Fixed during verification (restore-on-hit)."

## 8. Spec amendments

### 8.1. `2026-05-01-cache-cloud-readiness-design.md` §2

The 2026-05-01 spec §2 carries no "multi-output uploads first output only" non-goal; that language lives in `examples/cache_benchmarks/Cookfile` and the executor source. Those comments are removed alongside the §5.1 implementation. No edit to spec §2 required.

### 8.2. `2026-05-01-cache-cloud-readiness-design.md` §3.3

Replace the build-time flow (lines 78-87) with the version in §3.3 of this addendum: step 8 splits into 8a (local hit), 8b (local entry + on-disk drift → restore from backend), 8c (cloud hit → restore from backend), 8d (execute + record + per-output upload).

## 9. Backwards compatibility

- **On-disk cache:** `StepEntry` shape is unchanged; v3 caches written before this addendum remain readable. Steps with cross-recipe deps will see their `inputs[]` grow on the next rebuild (the existing entry's `inputs` will mismatch `current_inputs`, triggering `InputSetChanged`, which is a one-shot rebuild — desired).
- **Artifact store:** the `LocalBackend` storage layout (`<aa>/<bb…>`) is unchanged. Old single-artifact-per-entry uploads coexist with new per-output uploads; their keys differ (the old code uploaded under `cloud_key` directly; the new code uploads under `artifact_key(cloud_key, i, path)`). Old artifacts become orphaned and reclaimable by future eviction.
- **Backend trait:** unchanged. Cloud backend implementations in flight (SHI-24) need no API churn — they pick up multi-artifact-per-entry as soon as their `Backend::put` is wired in.

## 10. Open questions

None blocking implementation. Two notes for future work:

1. **Eviction of orphaned single-artifact uploads** — left to the future eviction policy work; pre-amendment artifacts are correctly addressed and harmless.
2. **`output_index` stability** — if a recipe author re-orders the outputs of a multi-output step, the artifact keys change. This is the desired behavior (the cache_key already captures everything else; reordering is a content-affecting change in any system that addresses outputs by position). No additional handling required.

## Appendix A. Naming-alignment regression test

The §6 monorepo test for cross-cookfile dep wiring is load-bearing for the §4.3 fix. The test asserts that after `cook demo` succeeds in a workspace where `demo` consumes `lib.lib_build`:

```
StepEntry { recipe: "demo", inputs: [
    FileRecord { path: "lib/build/lib.o", ... },   // resolved cross-cookfile
    ...
], ... }
```

If `terminal_outputs` is keyed by `"lib_build"` instead of `"lib.lib_build"`, the lookup returns `None`, the dep paths are not appended, and the test catches the regression at the cache-input level. The test MUST inspect `StepEntry.inputs` directly, not infer correctness from rebuild observation alone.
