# Cache: Hash-Based Caching and Invalidation

## Overview

Cook uses content-hash-based caching to skip rebuilding steps whose inputs and
outputs have not changed since the last run. The cache is per-recipe, per-step,
and stored on disk in `.cook/cache/`. When a step's inputs, output, and build
command are all identical to the last successful run, the step is skipped
entirely — no subprocess is launched.

The invalidation logic short-circuits on the first reason to rebuild, so the
common "nothing changed" case is fast: mtime comparison is tried before the
more expensive content-hash comparison.

---

## Data Structures (`src/cache/mod.rs`, `src/cache/store.rs`)

All persistent types live in `src/cache/store.rs` and are serialized with
bincode.

### `RecipeCache` (store.rs:8)

The top-level cache object for one recipe file.

| Field | Type | Purpose |
|---|---|---|
| `version` | `u32` | Schema version; mismatches cause the cache to be discarded on load |
| `globs` | `HashMap<String, Vec<String>>` | Maps each ingredient glob pattern to the list of paths it expanded to on the last run |
| `secondary_inputs_hash` | `u64` | xxh3_64 digest of all secondary ingredient files (indices > 0) |
| `env_hash` | `u64` | xxh3_64 digest of the environment variables in effect during the last run |
| `steps` | `HashMap<String, StepEntry>` | Per-step cache entries, keyed by cache key (see Cache Keys below) |

### `StepEntry` (store.rs:17)

Snapshot of one step's inputs, output, and command at the time it last ran.

| Field | Type | Purpose |
|---|---|---|
| `inputs` | `Vec<FileRecord>` | Ordered list of input file snapshots |
| `output` | `Option<FileRecord>` | Output file snapshot; `None` for plate steps |
| `command_hash` | `u64` | xxh3_64 of the rendered build command string |

### `FileRecord` (store.rs:23)

Snapshot of a single file at a point in time.

| Field | Type | Purpose |
|---|---|---|
| `path` | `String` | Relative path from the recipe's working directory |
| `mtime` | `u64` | Last-modified time in milliseconds since UNIX epoch |
| `hash` | `u64` | xxh3_64 of the file's contents |

Millisecond mtime resolution is intentional (see `check.rs:10`): it catches
rapid modifications that second-resolution timestamps would miss.

---

## Cache Keys

Cache entries in `RecipeCache::steps` are keyed differently depending on step
type:

- **Cook steps** (produce an output file): keyed by the output path. The output
  path is unique within a recipe, so it serves as a stable identifier across
  runs.
- **Plate steps** (no output file): keyed by a combination of the input paths
  and the command hash. Because plate steps have no output to serve as an
  anchor, the key must encode both what files went in and what command was run.

---

## Invalidation Logic (`src/cache/check.rs`)

The two public entry points are `needs_rebuild_cook` (check.rs:112) for steps
that produce output files, and `needs_rebuild_plate` (check.rs:159) for steps
that do not. Both return `(RebuildResult, Option<StepEntry>)`.

`RebuildResult` is either `Skip` or `Rebuild(RebuildReason)`. The `Option<StepEntry>` returned on `Skip` contains an updated entry with refreshed mtime
values (see mtime fast-path below) ready to be written back to the cache.

### Invalidation cascade

Checks run in this order and short-circuit on the first rebuild trigger:

1. **No cache entry** (`RebuildReason::NoCacheEntry`) — the step has never run,
   or its entry was evicted by a recipe-level invalidation. Rebuild.

2. **Command hash changed** (`RebuildReason::CommandHashChanged`) — the build
   command template has been edited since the last run. The current
   `command_hash` is compared against `entry.command_hash`. Rebuild.

3. **Output file missing** (`RebuildReason::OutputMissing`) — cook steps only.
   The output path does not exist on disk. Rebuild.

4. **Output file content changed** (`RebuildReason::OutputChanged`) — cook
   steps only. The output file's mtime differs from the cached mtime, and the
   content hash also differs. (Mtime change alone without a hash change is not
   treated as tampering.) Rebuild.

5. **Input set changed** (`RebuildReason::InputSetChanged`) — the ordered list
   of input paths is different from what was cached (a file was added, removed,
   or reordered). Rebuild.

6. **Input file content changed** (`RebuildReason::InputChanged(path)`) — for
   each input file in order, if its mtime differs from the cached mtime *and*
   its content hash also differs, the file has been modified. Rebuild.

7. **Mtime fast-path — update and skip** — if an input file's mtime differs
   from the cached value but its content hash is *identical*, the file was
   touched without being modified (e.g., `touch`, a VCS checkout that preserves
   content, or a build tool that rewrites a file with identical bytes). Cook
   updates `updated[i].mtime` in the returned `StepEntry` (check.rs:104) so
   the next check will not re-hash the file, then continues to the next input.
   No rebuild is triggered. The caller writes this updated entry back to the
   cache so the fast path is effective on subsequent runs as well.

8. **All checks pass → Skip** — returns `RebuildResult::Skip` with the
   (possibly mtime-refreshed) `StepEntry`.

The shared input-checking logic is factored into the private `check_inputs`
function (check.rs:75), which is called by both `needs_rebuild_cook` and
`needs_rebuild_plate`.

### Hashing utilities (check.rs:7–56)

| Function | Purpose |
|---|---|
| `stat_mtime` | Returns file mtime as epoch milliseconds; `None` if the file is missing |
| `hash_file` | Returns xxh3_64 of file contents; `None` if the file cannot be read |
| `hash_str` | Returns xxh3_64 of a string (used for command templates) |
| `hash_env` | Sorts env var pairs deterministically, joins as `KEY=VALUE` separated by newlines, then hashes |
| `hash_secondary_inputs` | Hashes all files matching secondary ingredient globs (index > 0); returns 0 when there are no secondary ingredients |

---

## Recipe-Level Invalidation

Before the per-step checks run, the scheduler performs three recipe-level
checks that can clear the entire cache for a recipe or prune individual entries.

### Environment hash

`RecipeCache::env_hash` stores the hash of the environment that was in effect
during the previous build. If the current env hash differs, the entire
`RecipeCache` is discarded and replaced with a fresh empty cache, forcing all
steps in the recipe to rebuild.

### Secondary inputs hash

`RecipeCache::secondary_inputs_hash` stores the hash of all files matched by
secondary ingredient globs (ingredient patterns at index 1 and beyond, computed
by `hash_secondary_inputs` at check.rs:34). Header files, shared libraries, and
other non-primary dependencies are modeled this way. If the hash changes, the
entire recipe cache is cleared.

### New files in ingredient globs

`RecipeCache::globs` maps each glob pattern to the set of paths it resolved to
on the last run. Before per-step checks, Cook re-expands the globs and compares
the results. If a glob now matches a file that was not in the previous
expansion, step entries that depended on the old glob result are removed from
the cache, forcing those steps to rebuild with the new file set. Entries for
files that have since been deleted are also pruned.

---

## Persistent Storage (`src/cache/store.rs`)

### Location

Cache files live at `.cook/cache/{recipe_name}.bin` relative to the project
root. Each recipe gets its own file; recipes do not share a cache.

### Format

`RecipeCache` is serialized to bytes with `bincode` (store.rs:61). The binary
format is compact and fast to deserialize, but is not human-readable. The
`version` field (currently `CACHE_VERSION = 1`, store.rs:5) allows Cook to
detect stale on-disk caches after a breaking schema change: if the deserialized
version does not match `CACHE_VERSION`, `load` returns `None` (store.rs:51–53).

### Atomic writes (store.rs:57–66)

To avoid leaving a corrupt cache if the process is interrupted mid-write:

1. Serialize the `RecipeCache` to bytes.
2. Write to `{recipe_name}.bin.tmp`.
3. Call `fs::rename` to atomically replace the final `.bin` file.

`rename` is atomic on POSIX filesystems: a reader will see either the old file
or the new one, never a partially-written file.

### Load

`RecipeCache::load` (store.rs:47) reads the `.bin` file, deserializes it, and
checks the version field. If the file is missing, contains corrupted bytes, or
has a version mismatch, `None` is returned and the caller falls back to a fresh
empty `RecipeCache::default()`.

---

## Thread-Safe Manager (`src/cache/mod.rs`)

When the parallel scheduler runs multiple steps concurrently, multiple threads
may need to read and write cache entries simultaneously. `ThreadSafeCacheManager`
(mod.rs:39) provides a thread-safe wrapper.

### Structure

```
ThreadSafeCacheManager {
    caches: Mutex<HashMap<String, RecipeCache>>,   // in-memory cache by recipe name
    cache_dir: PathBuf,                            // where .bin files are written
    dirty:  Mutex<HashSet<String>>,                // recipes with unsaved changes
}
```

### Key methods

**`load_recipe(recipe_name)`** (mod.rs:54) — loads a recipe's cache from disk
into memory. Called once per recipe before execution begins.

**`get_or_load(recipe_name)`** (mod.rs:91) — returns a clone of the cached
`RecipeCache`, loading it from disk first if it has not been loaded yet. Used
when the scheduler needs to read cache state for a step check.

**`update_step(recipe_name, cache_key, entry)`** (mod.rs:61) — atomically
inserts or replaces the `StepEntry` for `cache_key` in the named recipe's
in-memory cache, then records that recipe as dirty. There is no separate
`mark_dirty()` call; dirtying is an internal consequence of `update_step`.

**`flush_all()`** (mod.rs:72) — at the end of a build, iterates over all dirty
recipe names, writes each `RecipeCache` to disk via `RecipeCache::save`, then
clears the dirty set. This is the only point at which in-memory state reaches
disk.

### Single-threaded path

For non-parallel execution Cook uses `CacheState` (mod.rs:11) and
`SharedCacheState` (an `Rc<RefCell<CacheState>>`) instead of
`ThreadSafeCacheManager`. `CacheState::flush` (mod.rs:28) writes the cache
immediately when `dirty` is true and resets the flag.
