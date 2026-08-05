# cook-engine

`cook-engine` walks one work-unit DAG, deciding for every unit on it whether
the cache already holds the answer before spending anything to produce it.

That is the one thing. It is not quite the only thing in this directory; see
[What else lives here](#what-else-lives-here), which is the honest part of
this file.

## How it does that well

- **One DAG, one walk.** Cross-recipe edges, both the coarse `deps` barrier
  and the fine-grained per-unit `dep_edges`, sit on the same work-unit DAG as
  within-recipe ones. There is no per-wave register / build / execute loop
  (SHI-222 Phase 4), and progress events went with it: `RecipeStarted` fires
  when a recipe's first unit leaves `Waiting`, not when a wave opens, so the
  events describe unit motion rather than a scheduler's internal rhythm.
- **One cache judge for every unit.** A compile, a test, a probe, and a unit
  that declares no outputs at all all reach `needs_rebuild_cook` through
  `check_node_cache`. CS-0186 folded the separate test-result store into the
  step index at a single `Cacheability::ResultOnly` arm; while a test's
  verdict was cached by its own mechanism, four stale-hit bugs found one path
  and not the other.
- **A test is a unit with a name (CS-0191).** `WorkNode.test_name` is the only
  thing left that distinguishes a test at execute time. Test-ness is not a
  payload shape, so the classifier asks the node, and COOK-395 then deleted 61
  of the roughly 82 lines the test and generic dispatch arms held byte-identical
  between them: a change to dispatch bookkeeping is no longer N=2.
- **The two test-result accumulators are uncompilable to swap.** `process_ready`
  threads both through ten call sites and they used to be the same type, so
  transposing them anywhere compiled clean and quietly filed cache-hit rows as
  blocked. `CachedTestResults` and `BlockedTestResults` are newtypes for that
  reason and no other (COOK-395).
- **One publish path.** `publish_completion` is the single source of
  completion, record, `cloud_key`, artifact and depfile upload, and determinant
  manifest. Both completion sites call it: the restored/interactive one and the
  freshly-executed worker one. They were two roughly 210-line blocks before
  COOK-91.
- **A hit is byte-identical to a fresh build, not merely "the file is there."**
  CS-0119 reconciles a directory output to exactly the recorded set on a hit, so
  strays dropped in between runs are swept. COOK-278 extends the same guarantee
  to a cold fetch: the previous build's content-named outputs that the restore
  did not rewrite are removed first, which is what a warm revert over stale
  bundler chunks needs.
- **Sharing disposition decides which stores are consulted, in one place**
  (COOK-162). A `local` unit has the `RestoreCtx` withheld from it rather than
  being asked to remember not to use one, so the backend is unreachable even for
  a drift restore. A `pinned` unit missing from both stores is a hard failure,
  never a rebuild.
- **Probe evaluation is delegated, not copied.** The executor routes through
  `cook-probe` (COOK-359). The register-phase pre-pass and the executor's G4
  path had been two implementations of one lifecycle, and the register copy's
  cache block turned out never to have executed at all.
- **The plan is rejected rather than repaired.** A literal input equal to
  another recipe's literal output with no ordering path from reader to writer
  fails before any work is dispatched (§16.1.2). Inferring the edge would
  reverse §10.6, so the diagnostic names the three ways to declare the ordering
  instead of guessing one. The predicate is directed, unlike the write-write
  collision check: either ordering serialises two writes, but only
  consumer-requires-producer orders a read after a write.
- **Progress records carry an identity that survives the run.** `NodeStarted`
  and `NodeCompleted` stamp the recipe-local cache key (CS-0171, CS-0174),
  because the per-run DAG index dies with the run and the display name collides
  across distinct units. CS-0167 was an entire defect class built on basename
  collisions; `cook why` joins recorded observations on the key for that reason.
- **The warm case is the one that was measured.** `lookup_step` copies the one
  keyed entry instead of the whole recipe index: at DuckDB scale, 1,687 nodes
  behind a single 648k-record index, that was 95% of the process's allocation
  traffic (COOK-306). The same lookup computes `env_moved_key`, so the dominant
  warm re-run, an env value flipping and moving the key, is attributed to env
  instead of presenting as an unattributable cold build (COOK-276).

## What it does not do

It does not plan. Turning an invocation (a cwd, a target, some flags) into
the `RegisteredWorkspace` and recipe edge map this crate consumes is
`cook-plan` (COOK-419), and this crate does not depend on it — the handoff
type lives in `cook-register`, the stratum both share. Consequently it never
parses a Cookfile or generates its Lua: `cook-lang` and `cook-luagen` are not
in its dependency set at all. It does not own a worker VM (`cook-luaotp`), a
cache backend or store layout (`cook-cache`), or a process spawn
(`cook-shell`). It defines no contracts; `CacheMeta`, `WorkPayload`,
`Sharing`, the cacheability classification, and the fingerprint and key law
are `cook-contracts` (with the store-side half in `cook-cache` since
COOK-418 dissolved `cook-fingerprint`).

It does not render. It emits `EngineEvent` and the CLI translates. `NodeKind`
and `RecipeKind` are deliberate engine-side mirrors of the `cook-progress`
enums so that this crate does not depend on the renderer: a progress bar is one
possible consumer of the event stream, not the consumer.

It does not decide what the user asked for. Target selection, exit codes, and
diagnostics wording belong to `cook-cli`.

## What else lives here

COOK-419 cut this section down. The plan (`pipeline/*`, `analyzer`) is
`cook-plan` now, the dead `recipe_dag` wave scheduler is deleted (COOK-423),
and what remains beyond the walk is deliberate:

| Piece | Modules | Its one thing |
|---|---|---|
| The query | `why`, `observations` | answer what a build *would* do without being the build |
| The selection | `affected/*` | intersect a recipe closure with what `git` says changed |

`verify` is not on that list because it is not a query: `cook cache verify`
re-runs a unit in a sandbox to check the record reproduces. It spends work;
it is an experiment, and it stays with the walk.

The query stays in this crate on purpose. Since COOK-401 both the walk and
`cook why` reach the cache verdict through one shared observation query, but
`why` still re-derives adjacent plumbing around it; a crate boundary between
the two implementations of one decision would freeze the remaining
duplication instead of exposing it. When that residue is gone the query can
move (likely to `cook-graph`), and the boundary will be real rather than
declared. The selection (`affected/*`, ~280 lines) is a module, not a crate;
it stays until it earns an edge of its own.
