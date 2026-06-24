# cache-trust

The **Cache-trust v3** single-key model on one runnable Cookfile
(COOK-158 §4–§6). Every cacheable unit has exactly ONE content-addressed key
over its declared determinants; sharing is on by default; dispositions are
policy + determinant declarations, never a separate "strict" key. Host identity
is a declared probe, not engine-baked.

Four recipes, one per disposition:

| recipe     | disposition   | behaviour |
|------------|---------------|-----------|
| `portable` | *(none)*      | shares fleet-wide — its key is machine-independent, so it HITS across a host change |
| `hostdep`  | `seal host`   | the `host` probe value folds into the key; a host change re-keys and rebuilds |
| `scratch`  | `local`       | cached locally, never published to / fetched from the shared store |
| `generate` | `record`      | non-reproducible output; a warm hit reuses the recording instead of re-generating |

The `host` probe reads `$SIMHOST` (`produce as env { SIMHOST }`) so the demo can
simulate moving to a different machine by changing one environment variable —
in a real Cookfile it would be `cc -dumpmachine` or `uname -srm`.

The fifth disposition, `pinned` (fetch-only: a cache miss is a hard error, never
a rebuild), needs a pre-populated shared store and so can't be shown in a
self-contained single-machine demo — see the cross-machine E2E test
`cli/crates/cook-engine/tests/cache_trust_cross_machine_e2e.rs`.

Run: `bash verify.sh`.
