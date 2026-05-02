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
| 5 | Config CFLAGS toggle | `cook demo release` after a debug build | greet+util rebuild (env_contribution differs); demo rebuilds (input content differs) |
| 6 | Toggle back to prior variant | `cook demo debug` after release | greet+util **re-hit prior cache entries** (variant-keyed local cache); demo also rebuilds-then-hits |
| 7 | Sister recipe insulation | `cook sister` with various configs | sister hits across all configs (no CFLAGS in its consulted_env) |
| 8 | Denylisted env (HOME) | `HOME=/elsewhere cook demo debug` | All 3 hit (HOME is in D1 baseline denylist) |
| 9 | Multi-output pair | `cook pair` then `cook pair` | First execute, second hit; `~/.cache/cook/cloud/` shows **only the first output's bytes** uploaded — documented v3 limitation |

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

### Documented limitations (out of SHI-140 scope; follow-ups)

**Cross-recipe dependency outputs aren't recorded in the consuming recipe's
local `inputs[]`.** The `demo` recipe references `{greet}` and `{util}` (the
output paths of those recipes) in its using-string, but the resulting
`StepEntry.inputs` doesn't include `build/greet.o` and `build/util.o`. This
means demo's cache check has no way to detect when greet/util's outputs
have changed content. demo "hits" whenever its declared inputs (empty) +
its output file (`build/demo` left over from a prior run) match the cached
record. Sister recipes consuming via cross-recipe references will see the
same behavior.

This is a Cook DAG/cache integration issue separate from the SHI-140 cache
cloud-readiness work. The verify.sh expectations account for it. Follow-up:
have `cook.add_unit` (or its caller in cook-luagen) include cross-recipe
referenced outputs in the unit's inputs[] array.

**Multi-output cloud upload uploads only the first output's bytes.** The
`pair` recipe is the canonical reproducer. See spec §2 non-goals — a
manifest-style multi-output upload format is the right solution; left as a
follow-up. Verify scenario [14] confirms the bytes uploaded equal `foo.txt`
size, not `foo.txt + bar.txt`.

**Output stomping on variant toggle.** When two configs share an output
path (e.g., `build/greet.o` for both debug and release), toggling causes
on-disk content drift, which triggers `OutputChanged` rebuild on the next
toggle. The local cache *entries* coexist (per spec §5.1), but the on-disk
output can hold only one variant at a time. For true variant-isolation
without rebuild churn, use config-driven output paths
(e.g., `build/{config_name}/greet.o`).

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
