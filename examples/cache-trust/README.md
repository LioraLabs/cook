# cache-trust

The **Cache-trust v3** single-key model on one runnable Cookfile
(COOK-158 §4–§6). Every cacheable unit has exactly ONE content-addressed key
over its declared determinants; sharing is on by default; dispositions are
policy + determinant declarations, never a separate "strict" key. Host identity
is a declared probe, not engine-baked.

Five recipes, one per disposition:

| recipe     | disposition   | behaviour |
|------------|---------------|-----------|
| `portable` | *(none)*      | shares fleet-wide — its key is machine-independent, so it HITS across a host change |
| `hostdep`  | `seal host`   | the `host` probe value folds into the key; a host change re-keys and rebuilds |
| `scratch`  | `local`       | cached locally, never published to / fetched from the shared store |
| `generate` | `nondet`      | non-reproducible output; a warm hit reuses the recording instead of re-generating |
| `pin`      | `pinned`      | fetch-only: served from the cache, never rebuilt; a cold miss is a HARD ERROR |

The `host` probe folds `$SIMHOST` (`envs { SIMHOST }`) so the demos can
simulate moving to a different machine by changing one environment variable —
in a real Cookfile it would be `cc -dumpmachine` or `uname -srm`.

## Running it

| script | what it shows |
|--------|---------------|
| `bash verify.sh`        | single machine: the four cacheable dispositions on one host (cold build → warm hits, `nondet` value stability, host-change re-key) |
| `bash share-local.sh`   | **cross-machine sharing** on one host: two checkouts with independent local caches share one store. `portable` reuses by key across a host change; `hostdep` (`seal host`) and `scratch` (`local`) miss; `generate` (`nondet`) reuses the exact recording; `pin` (`pinned`) cold-miss hard-errors. Proof is `cook why`'s per-unit HIT/MISS classification. |
| `bash share-docker.sh`  | the same, made concrete across **two real containers** sharing a store: a `builder` container publishes `portable` by key, a `consumer` container fetches the exact bytes by key without re-running the command. Skips cleanly if Docker is unavailable (override the base image with `COOK_DEMO_IMAGE`). |

All three locate the `cook` binary at `../../cli/target/debug/cook` by default
(override with `COOK=/path/to/cook`); build it first with
`(cd ../../cli && cargo build --bin cook)`.

The cross-machine behaviours are also pinned by the E2E test
`cli/crates/cook-engine/tests/cache_trust_cross_machine_e2e.rs`.
