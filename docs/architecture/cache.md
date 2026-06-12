# Cache: Fingerprinting, Persistence, and Cloud Sharing

## Overview

Cook decides whether a step can be skipped by comparing a fingerprint of its
inputs, outputs, command, machine context, and consulted env against a record
kept from the previous run. When every component still matches, the step is
skipped and the cached `StepEntry` is refreshed; otherwise one specific
`RebuildReason` short-circuits the check and the engine reruns the step.

The same fingerprint inputs also derive a 32-byte SHA-256 **cloud key** that
addresses the resulting artifacts in any `CacheBackend` implementation —
local filesystem today, Cook Cloud's R2/D1 store when configured.

The cache layer is split across two crates:

- **`cli/crates/cook-fingerprint/`** — pure logic. Hashing, machine and tool
  identity, env-denylist filtering, rebuild decisions, and the `CacheBackend`
  trait + cloud-key composition. No I/O outside reading the files it
  fingerprints.
- **`cli/crates/cook-cache/`** — persistence. The on-disk `RecipeCache` file
  format, the `LocalBackend` content-addressed artifact store, the optional
  `CloudBackend` HTTP client, the thread-safe manager that batches writes,
  and the separate `TestCache` for `cook test` results.

`cook-cache` depends on `cook-fingerprint` and re-exports the most-used items
for back-compat with older `cook_cache::*` import sites; new code should
import from the crate that owns the type.

---

## Crate split: pure logic vs. persistence

### `cook-fingerprint` — pure

| Module | Defines |
|---|---|
| `lib.rs` | `hash_str`, `compute_test_fingerprint`, `FingerprintInputs`, `resolve_glob` |
| `check.rs` | `needs_rebuild_cook`, `needs_rebuild_plate`, `hash_env`, `hash_file`, `stat_mtime`, `RebuildResult`, `RebuildReason`, `RestoreCtx`, `install_depfile_parser` |
| `record.rs` | `FileRecord`, `StepEntry`, `CACHE_VERSION` (currently `4`) |
| `context.rs` | `ExecutionContext`, `MachineIdentity`, `ToolHash`, `step_context_hash` |
| `envkey.rs` | `EnvDenylist`, `env_contribution` |
| `backend.rs` | `CacheBackend` trait, `ArtifactMeta`, `BackendError`, `BackendConfig`, `CloudKey`, `CloudKeyInputs`, `cloud_key`, `artifact_key` |

The crate compiles without `std::fs` outside the hashing/mtime helpers and
the post-execution depfile parse it dispatches through a function pointer.
The function pointer (`install_depfile_parser` at
`cli/crates/cook-fingerprint/src/check.rs:129`) is the seam that lets the
engine install `cook-cache::parse_make_depfile` at startup without
`cook-fingerprint` taking a dependency on `cook-cache`.

### `cook-cache` — persistence

| Module | Defines |
|---|---|
| `lib.rs` | Re-exports + crate docs |
| `backend.rs` | `LocalBackend` (v3 filesystem CAS), `VerifyingReader`, `get_bytes`/`put_bytes` helpers |
| `store.rs` | `RecipeCache` on-disk format (TOML, hex-string hashes), `load`/`save`, atomic rename |
| `manager.rs` | `ThreadSafeCacheManager`, `CacheState`, `SharedCacheState`, `record_completion` |
| `cache_ctx.rs` | `CacheContext` (per-build aggregate of exec ctx, denylist, backend, cloud config) |
| `cloud_backend.rs` | `CloudBackend` HTTP client implementing `CacheBackend` |
| `cloud_config.rs` | `CloudConfig` deserialised from `.cook/cloud.toml` |
| `depfile.rs` | `parse_make_depfile` (Make-style `.d` parser for discovered inputs) |
| `test_cache.rs` | `TestCache`, `TestCacheEntry`, `TestCacheOutcome` (separate from recipe-step cache) |

---

## Data types

### `StepEntry` (cli/crates/cook-fingerprint/src/record.rs:15)

The fingerprint snapshot of a single unit. One entry per step per recipe.

| Field | Type | Purpose |
|---|---|---|
| `inputs` | `Vec<FileRecord>` | Ordered fingerprints of every declared (and discovered) input |
| `outputs` | `Vec<FileRecord>` | Fingerprints of every declared output (and the implicit depfile output, if any) |
| `command_hash` | `u64` | xxh3_64 of the rendered command text |
| `context_hash` | `u64` | xxh3_64 of `MachineIdentity` + declared-tool hashes + per-step tool resolutions |
| `env_contribution` | `u64` | xxh3_64 of the post-denylist consulted env |

### `FileRecord` (cli/crates/cook-fingerprint/src/record.rs:24)

| Field | Type | Purpose |
|---|---|---|
| `path` | `String` | Path relative to the recipe's working directory |
| `mtime` | `u64` | Last-modified time, epoch milliseconds (catches sub-second rewrites) |
| `hash` | `u64` | xxh3_64 of the file's bytes |

### `CacheMeta` (cli/crates/cook-contracts/src/lib.rs:141)

The per-unit metadata the engine hands to the cache layer for each executed
step. Carries everything needed to recompose the cloud key on the next run.

| Field | Type | Purpose |
|---|---|---|
| `recipe_name` | `String` | Recipe owning the unit |
| `project_id` | `String` | From `[cloud] project` in `.cook/cloud.toml`; namespace component |
| `cookfile_path` | `String` | Source Cookfile relative to project root, forward-slashed |
| `cache_key` | `String` | Per-step key within the `RecipeCache.steps` map |
| `input_paths` | `Vec<String>` | Declared input paths (post-glob, in deterministic order) |
| `output_paths` | `Vec<String>` | Declared output paths |
| `command_hash` | `u64` | Rendered-command hash |
| `context_hash` | `u64` | Machine + tool identity hash |
| `env_contribution` | `u64` | Post-denylist env hash |
| `consulted_env` | `BTreeMap<String, String>` | Pairs the command consulted (Layer-2 inference) |
| `discovered_inputs` | `Option<DiscoveredInputs>` | Declarative depfile binding, if any |

### `DiscoveredInputs` (cli/crates/cook-contracts/src/lib.rs:133)

`from` is the workspace-relative depfile path, `format` is currently always
`"make"`.

### `RecipeCache` (cli/crates/cook-cache/src/store.rs:42)

The top-level on-disk object, one per recipe under `.cook/cache/`.

| Field | Type | Purpose |
|---|---|---|
| `schema_version` | `u32` | CS-0048 wire-format tag; sourced from `CACHE_VERSION` |
| `globs` | `BTreeMap<String, BTreeSet<String>>` | Per-glob resolved-path sets from the last run |
| `steps` | `BTreeMap<String, StepEntry>` | Per-step entries keyed by `cache_key` |

`BTreeMap`/`BTreeSet` are mandatory — they keep the serialised bytes
deterministic across rebuilds. The legacy `secondary_inputs_hash` (SHI-145)
and `env_hash` (SHI-142) fields are gone: secondary inputs were a dead path,
and env state is now folded into the per-step `env_contribution`.

### `ArtifactMeta` (cli/crates/cook-fingerprint/src/backend.rs:98)

The sidecar persisted next to each artifact byte file. Carries the namespace,
the three fingerprint hashes, the schema version, `size_bytes`, `tags`,
`consulted_env_keys` (**keys only — values never persisted**), the
`output_index`/`output_path`, and the SHA-256 `content_hash` stamped by
`CacheBackend::put` at write time and verified streaming-style by `get`.

---

## Cache-key composition

Cook addresses every cached artifact by a 32-byte SHA-256 **cloud key**,
composed in `cook_fingerprint::backend::cloud_key`
(`cli/crates/cook-fingerprint/src/backend.rs:271`). The input struct
`CloudKeyInputs` carries:

1. `schema_version` (currently `CACHE_VERSION = 4`)
2. `recipe_namespace` — `"{project_id}/{cookfile_path}::{recipe_name}"`
3. `command_hash`
4. `context_hash`
5. `env_contribution`
6. `sorted_input_content_hashes` — input `FileRecord.hash` values **the caller
   must sort by path before passing** (spec §5.3)

A `0x00` delimiter separates the namespace bytes from the hash bytes to
prevent string-injection collisions. Each output of one logical entry gets
its own artifact-scoped key via `artifact_key(&cloud_key, output_index,
output_path)` (`cli/crates/cook-fingerprint/src/backend.rs:256`), so one
multi-output step uploads independently addressable artifacts.

Changing any of the six inputs changes the cloud key, which is what makes
shared cloud caches sound: two builds on different machines or with different
env will never read each other's bytes by accident.

---

## Env handling: the denylist

`env_contribution` is **not** a hash of the process's full environment.
Cook only hashes the env vars the command actually consulted (Layer-2
inference, captured in `cook-luagen`/`cook-register`), then runs that map
through an `EnvDenylist` to strip vars that are machine-specific rather than
build-affecting.

The denylist (`cli/crates/cook-fingerprint/src/envkey.rs:9`) has two layers:

- **D1 baseline (shipped)**: exact names like `HOME`, `USER`, `PATH`, `PWD`,
  `SSH_AUTH_SOCK`, `DBUS_*`, `TMPDIR`; plus glob patterns `XDG_*`,
  `GITHUB_*`, `RUNNER_*`, `GITLAB_CI_*`, `BUILDKITE_*`, `CIRCLE_*`,
  `TRAVIS_*`, `JENKINS_*`, `TEAMCITY_*`, `DRONE_*`.
- **D2 project additions**: extra names or globs from
  `.cook/cloud.toml [cache] ignore_env`, applied via `extend_with` and
  idempotent on overlap with D1.

`env_contribution` (`cli/crates/cook-fingerprint/src/envkey.rs:80`) iterates
the consulted map (already sorted via `BTreeMap`) and folds each surviving
`KEY=VALUE\n` into an `xxh3_64` digest. The map iteration order is
deterministic so the hash is order-independent at the call site.

---

## Machine and tool identity

Two fingerprint components keep the cache machine-aware so a Linux artifact
doesn't get installed onto a macOS workspace through a key collision.

### `MachineIdentity` (cli/crates/cook-fingerprint/src/context.rs:14)

Build-wide, probed once per `cook build`:

- `target_triple` — from `env!("COOK_TARGET_TRIPLE")` at compile time.
- `libc_version` — best-effort glibc version on Linux, `None` on
  musl/macOS/BSD/RISC-V (target_triple still distinguishes them).
- `locale_baseline` — sorted map of `LANG`, `TZ`, `SOURCE_DATE_EPOCH`, and
  all `LC_*` env vars.

`encode()` (context.rs:31) produces a stable byte sequence with `0x1F` /
`0x1E` field separators so hashing is deterministic.

### `ToolHash` and `step_context_hash`

`step_context_hash` (`cli/crates/cook-fingerprint/src/context.rs:227`)
combines the machine identity with the SHA-256 content hash of every
executable resolved from the command text:

- Command text is split on newlines; for each statement, leading `VAR=value`
  env-prefix tokens are skipped, the next token is treated as an executable,
  resolved via `which::which` + `canonicalize`, then SHA-256-hashed.
- Tool hashes are cached per realpath in `ExecutionContext.tool_cache`.
- The build-wide `declared_tools_hash` (from `.cook/cloud.toml [cache] tools`,
  CS-0052) folds in too — a misdeclared name aborts the build at startup.

**Coverage limit**: only the first executable on each newline-separated
statement is fingerprinted. Tools downstream of `;`, `&&`, `||`, pipes,
command substitution, subshells, or `xargs`/`find -exec` are not. Pin those
via `cache.tools` in `.cook/cloud.toml` or split them onto separate lines.

---

## Invalidation cascade

`needs_rebuild_cook` (`cli/crates/cook-fingerprint/src/check.rs:154`) is the
entry point for steps that produce output files; `needs_rebuild_plate`
(check.rs:393) handles output-less steps. Both return
`(RebuildResult, Option<StepEntry>)`. A `Skip` carries an updated entry with
refreshed mtime values; a `Rebuild(reason)` carries `None`.

Checks short-circuit on the first failing predicate:

| # | Check | `RebuildReason` |
|---|---|---|
| 1 | No prior entry | `NoCacheEntry` |
| 2 | `command_hash` differs | `CommandHashChanged` |
| 3 | `context_hash` differs (machine or tool changed) | `ContextChanged` |
| 4 | `env_contribution` differs | `EnvChanged` |
| 5 | Output count differs / cook-only: an output is missing on disk | `OutputMissing` |
| 6 | Cook-only: an output's mtime differs **and** its content hash differs | `OutputChanged` |
| 7 | Input path set (ordered) differs from the cached set | `InputSetChanged` |
| 8 | An input's mtime differs **and** its content hash differs | `InputChanged(path)` |
| — | **mtime fast-path**: input mtime differs but content hash matches | refresh mtime, continue |
| — | All checks pass | `Skip(StepEntry)` |

The fast-path lives at the same call site as the input-content check
(check.rs:84–97): when `disk_mtime != cached.mtime` but `disk_hash ==
cached.hash`, the file was touched without modification (vcs checkout, `touch`,
build tool that rewrites identical bytes). Cook updates `updated[i].mtime` in
the returned `StepEntry` so the next check short-circuits without re-hashing.
**Empty files** (`hash == empty_hash()`) are treated as signals: any mtime
change forces a rebuild even when contents match, so `touch
.cook/sentinel` works as a manual invalidator.

### Restore-on-hit (spec §5.2)

When `needs_rebuild_cook` is called with a `RestoreCtx`
(check.rs:142), an entry whose command/context/env hashes match but whose
on-disk outputs are missing or have drifted will first attempt to fetch the
bytes from the backend before falling back to rebuild. `try_restore`
(check.rs:328) recomposes the `cloud_key` from the cached inputs, fetches
each missing/drifted output by `artifact_key`, verifies the streaming bytes
against `entry.outputs[idx].hash`, writes atomically via `.cook.tmp` +
rename, and on any miss returns `false` so the caller falls through to a
normal rebuild.

---

## Recipe-level invalidation

Per-recipe environment and secondary-inputs hashes are gone (SHI-142,
SHI-145). Every invalidation signal that used to live at the recipe level
now flows through per-step `env_contribution` and explicit `input_paths`.

The one recipe-level structure that survives is `RecipeCache.globs`: a
`BTreeMap<glob, BTreeSet<path>>` of glob expansions from the last run. The
engine re-expands ingredient globs at the start of each build; entries
referencing now-deleted files are pruned, and added files force the
dependent steps to rebuild via `InputSetChanged`.

---

## Persistent format

### Recipe cache file layout

`RecipeCache::save` (cli/crates/cook-cache/src/store.rs) writes
`.cook/cache/{recipe_name}.toml` as a human-readable TOML file:

1. Serialise to a TOML string. u64 hash fields (`command_hash`,
   `context_hash`, `env_contribution`, `FileRecord.hash`) are emitted as
   zero-padded 16-digit lowercase hex strings via
   `cook_fingerprint::record::hex_u64`; `mtime` stays a TOML integer.
2. Write to `{recipe_name}.toml.tmp`.
3. `fs::rename` to the final `.toml` path.

`rename` is atomic on POSIX, so a reader sees either the prior cache or the
new cache, never a torn write. `RecipeCache::load` reads the `.toml` file,
deserialises with `toml::from_str`, and refuses any cache whose
`schema_version != CACHE_VERSION` — both downgrades and upgrades surface as
`None` (the cache is regeneratable; no error is needed).

Pre-v4 bincode `.bin` and `.bin.tmp` files written directly inside
`.cook/cache/` are not read by this loader (it only opens `.toml`). Orphaned
`.bin`/`.bin.tmp` files in `.cook/cache/` are deleted once by
`ThreadSafeCacheManager::new` — non-recursive, so the `tests/` JSON cache
under `.cook/cache/tests/` is untouched. Running `cook clean` (`rm -rf
.cook`) removes them along with everything else.

Per the CS-0048 read policy in `store.rs` crate docs: today's check is
exact equality (pre-v1.0); the forward-compatible `<= CACHE_VERSION` form
takes effect once the additive-only contract starts at v1.0.

### Artifact CAS layout

`LocalBackend` (cli/crates/cook-cache/src/backend.rs:130) is a content-
addressed store rooted at a project-configured directory. For a 32-byte
cloud key `K`, the artifact path is
`{root}/{hex(K[0])}/{hex(K[1..32])}` — first byte fans out into 256
subdirectories to bound per-directory entry count.

Each artifact is two files:

- `{path}` — the artifact bytes.
- `{path}.meta.json` — `ArtifactMeta` sidecar with the recorded
  `content_hash`, namespace, fingerprint components, etc.

`put` streams bytes through a SHA-256 hasher into `{path}.tmp`, enforces
`max_artifact_bytes` mid-stream, then runs CS-0055 idempotency checks
against any existing sidecar (identical bytes → no-op success; conflicting
bytes → `BackendError::Other` naming the key in hex; missing/malformed
sidecar → partial-write recovery, write through). After commit, both the
bytes and the sidecar reach disk via tmp + rename.

`get` returns a `VerifyingReader` (backend.rs:31): a streaming wrapper that
tees bytes through SHA-256 and raises `io::Error(InvalidData)` at EOF on
mismatch. A missing sidecar, malformed JSON, the zero-sentinel
`content_hash` (pre-CS-0054 entries), or sidecar-without-bytes all surface
as `Ok(None)` — fail-closed.

---

## Local vs. cloud backends

`CacheBackend` (cli/crates/cook-fingerprint/src/backend.rs:134) is the seam.
Both `LocalBackend` and `CloudBackend` implement the same five methods —
`batch_query`, `get`, `put`, `delete`, `health`.

### `LocalBackend`

Always present. Configured via `.cook/cache/` (default) or
`[cache] cache_dir` in `.cook/cloud.toml`. Honours
`BackendConfig::max_artifact_bytes` at `put` time; the network knobs
(`timeout`, `max_retries`, `backoff_*`) are no-ops for disk I/O but are
threaded through anyway so the same `BackendConfig` struct works for both
backends.

### `CloudBackend`

Constructed only when `.cook/cloud.toml` has `[cloud] enabled = true` plus
both `project` and `endpoint`, and the `COOK_CLOUD_API_KEY` env var is set
(CS-0059: the legacy `[cloud] api_key` TOML field was removed to close the
secret-in-checked-in-config foot-gun). Talks v1 HTTP against
`{endpoint}/v1/artifacts/...`, with bearer-token auth. See
`cli/crates/cook-cache/src/cloud_backend.rs` and design CS-0058 for the
full wire protocol, status-code mapping, and retry policy (jittered
exponential backoff between `backoff_initial` and `backoff_max`, bounded by
`max_retries`, only `BackendError::Transient` retried).

### Read-write flow

When both backends are configured, the engine reads local first and falls
back to cloud on miss; writes go to both. The composition lives at the
engine level (the trait surface is uniform, so a "tiered backend" wrapper
is straightforward). Either backend's failure is non-fatal: the engine
treats it as a miss and proceeds — `Unauthorized` disables the backend for
the rest of the build, `QuotaExceeded` honours a `Retry-After` hint if
present, `Transient` retries, `Other` is logged and treated as a miss.

---

## Discovered inputs

Some toolchains (notably C/C++ compilers) discover their real dependency
set during execution — `#include` resolution can't be known up front.
Cook handles this through `DiscoveredInputs`
(`cli/crates/cook-contracts/src/lib.rs:133`): a unit declares it will write
a Make-format depfile (`from = ".cook/deps/foo.d", format = "make"`), and
the engine:

1. **Before the check**: if a prior depfile is on disk, parse it via
   `cook_cache::parse_make_depfile`
   (`cli/crates/cook-cache/src/depfile.rs:46`) and augment `current_inputs`
   with the discovered paths so the input set matches the previous run's
   fat entry.
2. **After successful execution**: re-parse the depfile and append its
   entries to the recorded `StepEntry.inputs`. The depfile itself is also
   appended to `StepEntry.outputs` as an implicit restorable artifact
   (`manager.rs:149`).
3. **On restore**: the implicit depfile output is fetched by its own
   `artifact_key`, so a partial workspace wipe that removes only the `.d`
   recovers without rebuilding.

`parse_make_depfile` strips the target text up to the first `:`, joins
continuation lines, then skips absolute paths, the source itself, and any
path that doesn't exist relative to the working directory. Order is
preserved; duplicates are deduped on first occurrence.

The fingerprint crate doesn't link `cook-cache`, so the parser is wired in
via the `install_depfile_parser` function pointer at engine startup
(`cli/crates/cook-fingerprint/src/check.rs:129`).

### §10 refinement: missing-depfile fallback

When a depfile is missing or malformed but a `RestoreCtx` is available,
the check uses the stored entry's fat input list instead of failing with
`InputSetChanged`. The depfile is an implicit output, so the subsequent
output walk hits the missing-output path and `try_restore` fetches the
`.d` back from the backend alongside the real artifacts. Without a
restore context, the missing depfile falls through to `InputSetChanged`
and a normal rebuild self-heals.

---

## Test cache

`cook test` results have their own cache, separate from the recipe-step
cache, because test outcomes carry pass/fail state and rerun policy that
the recipe-step cache deliberately doesn't model.

### Fingerprint

`compute_test_fingerprint` (`cli/crates/cook-fingerprint/src/lib.rs:92`)
implements CS-0061 §3.3. It folds (in this stable order):

1. `cmd` (substituted command text)
2. `timeout` (big-endian u64)
3. `should_fail` flag
4. `cook_outputs` (sorted `(path, fingerprint)` pairs)
5. `dep_outputs` (sorted)
6. `env_keys` (sorted `(key, value)` pairs)
7. `tool_hashes` (sorted)

Output is `"sha256:<hex>"`. Critically, **`suite_name` and `test_name` are
excluded** — renaming a test via `as STRING` MUST NOT bust its fingerprint.

### Storage

`TestCache` (`cli/crates/cook-cache/src/test_cache.rs:64`) writes JSON
files under `{local_root}/cache/tests/{prefix}/{full}.json`, where
`prefix` is the first two hex chars of the fingerprint after stripping
the `sha256:` scheme. Only `TestCacheOutcome::Passed` entries are
persisted — failed, timed-out, and blocked outcomes are never cached so a
subsequent `cook test` always reruns them. Writes are atomic (tmp +
rename). `lookup` rejects entries whose `schema_version != 1` or whose
stored fingerprint doesn't match the lookup key (tamper/rename guard).

`TestCacheEntry` carries stdout, stderr, observed duration, and
`should_fail_observed` so the reporter can annotate a cached hit with
realistic durations and the original pass/fail polarity.

---

## Thread-safe manager

The parallel scheduler runs many units concurrently. `ThreadSafeCacheManager`
(`cli/crates/cook-cache/src/manager.rs:76`) is the write-side wrapper.

```text
ThreadSafeCacheManager {
    caches:    Mutex<HashMap<String, RecipeCache>>,  // recipe_name -> in-memory cache
    cache_dir: PathBuf,                              // root for .toml index files
    dirty:     Mutex<HashSet<String>>,               // recipes with unsaved changes
}
```

Key methods (`cli/crates/cook-cache/src/manager.rs`):

| Method | Behaviour |
|---|---|
| `load_recipe(name)` (manager.rs:91) | Load the named recipe's `.toml` index (or insert empty default) |
| `get_or_load(name)` (manager.rs:127) | Return a clone of the in-memory cache, loading on demand |
| `update_step(name, key, entry)` (manager.rs:97) | Insert/replace a step entry and mark the recipe dirty |
| `record_completion(name, key, meta, wd)` (manager.rs:137) | Build a `StepEntry` from `CacheMeta` (fingerprinting inputs/outputs via `collect_records`), append the depfile to outputs when `discovered_inputs` is set, then `update_step` |
| `flush_all()` (manager.rs:108) | For every dirty recipe, call `RecipeCache::save` (atomic write); clear the dirty set |

`flush_all` is the only point where in-memory step entries reach disk.

### Single-threaded path

For non-parallel execution Cook uses `CacheState` (manager.rs:43) wrapped
in `SharedCacheState = Rc<RefCell<CacheState>>`. `CacheState::flush`
(manager.rs:60) writes when `dirty == true` and clears the flag — the same
atomic tmp+rename underneath, just without the mutex hierarchy.

### Per-build aggregate: `CacheContext`

`CacheContext` (`cli/crates/cook-cache/src/cache_ctx.rs:13`) bundles
everything the cache layer needs into one struct constructed in
`cook-engine`'s `run.rs` and threaded down through execution:

- `exec_ctx: Arc<ExecutionContext>` — machine + tool identity, declared tools.
- `denylist: Arc<EnvDenylist>` — D1 baseline + D2 project additions.
- `backend: Arc<dyn CacheBackend>` — local, cloud, or a tiered composition.
- `cloud_config: Arc<CloudConfig>` — parsed `.cook/cloud.toml`.
- `project_root`, `project_id` — namespace components for the cloud key.
