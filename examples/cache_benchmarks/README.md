# Cache Benchmarks

Verification fixture for the SHI-140 cache cloud-readiness work. Each recipe
exercises one cache invariant from `standard/specs/2026-05-01-cache-cloud-readiness-design.md`.
Run `verify.sh` to exercise the matrix.

## Files

```
.cook/cloud.toml      project_id + cache.ignore_env (extends D1 baseline)
Cookfile              recipes (greet, util, demo, sister, pair, clean)
src/
  greet.c             prints a greeting; consumed by greet recipe
  util.c              main() — links against greet; consumed by util recipe
  sister.txt          input for the sister recipe (no CFLAGS dependence)
tools/
  genpair.sh          helper for the multi-output pair recipe
verify.sh             scenario matrix; prints PASS/FAIL per scenario
README.md             this file
```

## Scenario matrix

| # | Scenario | Cookfile shape | Expected behavior |
|---|---|---|---|
| 1 | Fresh build | `cook clean && cook demo debug` | All 3 recipes execute (no cache hits) |
| 2 | No-op rebuild | `cook demo debug` again | All 3 recipes hit (no rebuild) |
| 3 | Source touch | `touch src/greet.c && cook demo debug` | All 3 still hit (mtime fast-path detects content unchanged) |
| 4 | Source content change | `echo " " >> src/greet.c && cook demo debug` | greet rebuilds; util hits; demo rebuilds (input content changed) |
| 5 | Config CFLAGS toggle | `cook demo release` after a debug build | greet+util rebuild (env_contribution differs); demo rebuilds (its inputs — greet.o/util.o — drifted, addendum §4.3) |
| 6 | Toggle back to prior variant | `cook demo debug` after release | greet+util **restore from artifact store** (addendum §5.2); demo rebuilds (its single cache entry tracks the most-recent inputs only) |
| 7 | Sister recipe insulation | `cook sister` with various configs | sister hits across all configs (no CFLAGS in its consulted_env) |
| 8 | Denylisted env (HOME) | `HOME=/elsewhere cook demo debug` | All 3 hit (HOME is in D1 baseline denylist) |
| 9 | Multi-output pair | `cook pair` then `cook pair` | First execute, second hit; `~/.cache/cook/cloud/` now contains **both** outputs as separate artifacts (addendum §5.1) |
| 10 | Imported lib round-trip | `cook clean && cook mono` then `cook mono` | First run executes 5 recipes (greet, util, lib.lib_build, demo, mono); second run hits the 4 cacheable ones |
| 11 | Cross-cookfile dep drift | `echo // > lib/src/lib.c && cook mono` | lib.lib_build invalidates; demo still hits (cross-cookfile body refs are future work — see pipeline.rs:526) |
| 12 | Variant-toggle restore | debug → release → debug | Final greet.o bytes match the original debug bytes (restore-on-hit, addendum §5.2) |
| 13 | Multi-output restore | `cook pair && rm -rf build/pair && cook pair` | Both `foo.txt` and `bar.txt` restored from artifact store with no command execution (addendum §5.1) |

## Recipes

### `greet`, `util` — cacheable C compile steps

Both consult `{CFLAGS}` and `{CC}` via Layer 2 inference. Their
`consulted_env_keys` includes both names, so a config overlay that changes
either causes a cache miss for these steps.

### `demo` — link step

Does **not** consult `{CFLAGS}` (the linker doesn't take CFLAGS through the
template). It does invoke the `gcc` binary, so its `context_hash` captures
the linker's resolved binary content. demo's cache entry is keyed by command
+ context, with no env-discriminator: a config toggle does not split demo
into two cache entries, but it still rebuilds because the input `.o` files
have new content.

### `sister` — recipe with no CFLAGS dependency

Cats `src/sister.txt` to `build/sister.out`. No env vars consulted by the
command; its `consulted_env_keys` is empty, so its `env_contribution = 0`.
Used to verify env-key isolation: toggling CFLAGS doesn't perturb this
recipe's cache, demonstrating that per-step env keying (SHI-142) is
properly scoped.

### `pair` — multi-output recipe (DOCUMENTED v3 LIMITATION)

Produces `build/pair/foo.txt` and `build/pair/bar.txt`. The local cache
tracks both outputs in the `StepEntry`. **However**, the executor's
`Backend::put` upload logic uploads **only the first output's bytes** —
this is the spec §2 non-goal "multi-output steps upload only the first
output bytes; manifest-style multi-output upload is a follow-up."

Verify with:
```sh
ls $HOME/.cache/cook/cloud/    # LocalBackend artifact store
cat $HOME/.cache/cook/cloud/<prefix>/<rest>.meta.json | jq .size_bytes
```

For the pair recipe, `size_bytes` will be the size of `foo.txt` only.
A teammate cache-hitting this entry from a future `CloudBackend` would
receive `foo.txt`'s bytes but not `bar.txt`. Until the manifest format
ships, multi-output recipes should be local-only or split into
single-output sub-recipes.

## What the verification proves

When `verify.sh` passes:

- **AC-141.1** Switching `CC` between compilers produces distinct cache entries. *(Tested via config CC change if local toolchain has both; otherwise asserted by inspection.)*
- **AC-142.1/2** `CFLAGS=-O0 -g` ↔ `-O3` get distinct local cache entries; toggling back rehits prior.
- **AC-142.4** Non-`{CFLAGS}`-consulting steps (`sister`) keep their cache across config swaps.
- **AC-Env.3** Denylisted env (`HOME`) does not contribute to `env_contribution` — toggling does not invalidate.
- **AC-B2.3** `LocalBackend::put` is idempotent — re-running an already-cached recipe doesn't fail.
- **Cloud-key uniqueness in practice** — `~/.cache/cook/cloud/` accumulates one artifact per `(recipe × variant)` tuple.
- **Multi-output limitation** — pair recipe uploads only `foo.txt`; documented.

## Findings caught by this fixture

### Fixed during verification

**Phase 5's `consulted_env_keys` lookup used `std::env::var`** (process env)
instead of `cook.env` (the merged Cookfile-config + process env that the
command actually consulted). Caught while authoring this benchmark — fixed
in commit `2a2a4b3`. Scenarios 5–7 now correctly observe `env_contribution`
changes when config overlays change `CFLAGS`.

**Cross-recipe dep outputs land in `cache_meta.input_paths` (addendum §4.3).**
Originally, `demo` referencing `{greet}` and `{util}` recorded a DAG edge but
left `cache_meta.input_paths` empty, so demo silently hit even when
`build/greet.o` content drifted. After the addendum, `cook.dep_output(name)`
accumulates the dep ref into `step_group_dep_refs`, and `add_unit` resolves
those refs against `SharedTerminalOutputs` to append the resolved paths to
`cache_meta.input_paths` (and the cache key composition). `WorkPayload.inputs`
remains scoped to the recipe's own iteration source.

**Multi-output upload writes one artifact per output (addendum §5.1).**
Each output gets its own `artifact_key = SHA-256(cloud_key || u32_le(idx) ||
path_bytes)`, so a recipe with N outputs uploads N artifacts. The `pair`
recipe now produces both `foo.txt` and `bar.txt` artifacts in the
LocalBackend store, both restorable on cache hit. Verify scenario [14]
confirms both meta sidecars are present with sizes matching the on-disk
files.

**Output drift triggers restore-on-hit, not rebuild (addendum §5.2).** When
a cache entry's command/context/env hashes still match but the on-disk
output content has drifted (e.g., a variant toggle stomped the file), the
local cache check now consults `Backend::get` to restore the original
bytes from the LocalBackend store before falling back to `OutputChanged`
rebuild. Scenario [18] verifies that a debug → release → debug toggle
returns greet.o to its original debug bytes via restore.

### Monorepo / `import` keyword cache verification (addendum §6)

Scenarios 10–11 exercise the imported `lib/Cookfile`:
- `lib.lib_build` registers under the namespaced name and is reachable from
  the parent's `mono` recipe via `: lib.lib_build`.
- Touching `lib/src/lib.c` invalidates `lib.lib_build` correctly across
  the cookfile boundary.

Cross-cookfile body refs (`{lib.lib_build}` inside a parent body template)
are not currently supported by the language — they degrade to `cook.env[]`
lookups. Same-Cookfile body refs in a workspace context now work after
fixing `workspace.rs` to thread `recipe_names` through the codegen call.

## Running

Prerequisites: `gcc` on `$PATH`. Build the cook binary in the workspace
first:

```sh
cd ../../cli
cargo build --bin cook
```

Then from this directory:

```sh
./verify.sh
```

The script cleans build artifacts and the `~/.cache/cook/cloud/` LocalBackend
store before each scenario, so it's idempotent.
