# Incremental Build Cache Design

## Overview

Cook currently re-runs every step on every build. This design adds step-level incremental caching so that only steps whose inputs have actually changed are re-executed. The goal is to make Cook competitive with modern build systems on rebuild speed.

## Core Concepts

### Layers

Each `cook` or `plate` step in a recipe is a **layer** — the atomic unit of caching. A layer has declared inputs and (optionally) outputs. The runtime decides whether to execute or skip each layer based on whether its inputs have changed since the last successful run.

- **`cook` steps = cacheable layers with output** — declared I/O, enforced 1-to-1 or many-to-1 contract, pure function: inputs → output file
- **`plate` steps = cacheable layers without output** — declared inputs, side-effectful (run tests, rsync, deploy). Cached on input state only — if inputs unchanged, skip the side effect.
- **`shell`/`lua` steps = uncacheable** — always re-run, no I/O contract
- **Declaration-only `cook` steps** (no `using` clause) — not wrapped in `cook.layer()`, they declare an output path without performing a transform

### Two Layer Shapes

1. **1-to-1** (`{in}` → `{out}`): The layer body runs once per input file. Each (input, output) pair is cached independently.
2. **Many-to-1** (`{all}` → `{out}`): All inputs aggregate into one output. Cached as a single entry with multiple inputs.

Both shapes apply to `cook` and `plate` steps. For `plate`, the "output" dimension is absent — caching is purely input-driven.

### Change Detection: Hybrid mtime + xxHash

1. **Stat mtime first** — one syscall per file, extremely fast
2. **If mtime changed, hash content** — xxh3_64 via `xxhash-rust`, ~30 GB/s
3. **If hash unchanged** — update cached mtime, skip the step (false positive from `touch`, editor save, etc.)
4. **If hash changed** — execute the step, update cache

The fast path (nothing changed) costs only stat syscalls. No file reads, no hashing, no subprocess spawning.

## Cache Data Model

### Storage

- Per-recipe binary files at `.cook/cache/<recipe_name>.bin`
- Serialized with `bincode` via `serde`
- Users `.gitignore` the `.cook/` directory
- Missing or corrupted cache files treated as full cache miss — silent, self-healing
- If cache `version` does not match the expected version, treat as full cache miss and rebuild all
- Cache is written incrementally after each successful layer (atomic: write to temp file, rename). If a build is interrupted, only the in-progress layer is lost. If a layer's body fails, its cache entry is NOT updated.

### Rust Types

```rust
#[derive(Serialize, Deserialize)]
pub struct RecipeCache {
    pub version: u32,
    pub globs: HashMap<String, Vec<String>>,  // pattern → sorted matched files
    pub secondary_inputs_hash: u64,           // xxh3 of all secondary ingredient files (headers, etc.)
    pub env_hash: u64,                        // xxh3 of cook.env table contents
    pub steps: HashMap<String, StepEntry>,    // output path as key (unique per layer)
}

#[derive(Serialize, Deserialize)]
pub struct StepEntry {
    pub inputs: Vec<FileRecord>,
    pub output: Option<FileRecord>,  // None for plate steps (side-effect only)
    pub command_hash: u64,
}

#[derive(Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub mtime: u64,
    pub hash: u64,  // xxh3_64
}
```

### Command Hash

The `command_hash` is computed at codegen time:
- For shell template `using` clauses: xxh3_64 of the template string (e.g. `"gcc -c {in} -o {out}"`)
- For Lua block `using >{ }` clauses: xxh3_64 of the Lua source text

This means changing the build recipe (adding `-O2`, modifying a Lua block) invalidates all steps even if input files didn't change. Known limitation: changes to `cook.env` values read at runtime don't invalidate — addressed by the future borrow checker.

### Example Cache State

After building a project with two source files:

```
RecipeCache {
    version: 1,
    globs: {
        "src/*.c": ["src/main.c", "src/util.c"]
    },
    steps: {
        "build/main.o": StepEntry {
            inputs: [FileRecord { path: "src/main.c", mtime: 1710234567, hash: 0xa1b2c3d4e5f6 }],
            output: Some(FileRecord { path: "build/main.o", mtime: 1710234568, hash: 0xf6e5d4c3b2a1 }),
            command_hash: 0x9f8e7d6c,
        },
        "build/util.o": StepEntry {
            inputs: [FileRecord { path: "src/util.c", mtime: 1710234567, hash: 0x1a2b3c4d5e6f }],
            output: Some(FileRecord { path: "build/util.o", mtime: 1710234568, hash: 0x6f5e4d3c2b1a }),
            command_hash: 0x9f8e7d6c,
        },
        "build/lib.a": StepEntry {
            inputs: [
                FileRecord { path: "build/main.o", mtime: 1710234568, hash: 0xf6e5d4c3b2a1 },
                FileRecord { path: "build/util.o", mtime: 1710234568, hash: 0x6f5e4d3c2b1a },
            ],
            output: Some(FileRecord { path: "build/lib.a", mtime: 1710234569, hash: 0xabcdef012345 }),
            command_hash: 0x3c4d5e6f,
        },
    },
}
```

## Rebuild-Check Algorithm

### For `cook` layers (1-to-1, per input/output pair):

1. Cache entry exists for this output? → NO → **rebuild**
2. Command hash changed? → YES → **rebuild**
3. Output file exists on disk? → NO → **rebuild**
4. Output mtime changed from cached? → YES → hash output → output hash changed? → YES → **rebuild**
5. Input mtime changed from cached? → NO → **skip** (fast path)
6. Input content hash changed? → NO → update cached mtime, **skip**
7. → **rebuild**

### For `cook` layers (many-to-1):

1. Cache entry exists for this output? → NO → **rebuild**
2. Command hash changed? → YES → **rebuild**
3. Output file exists on disk? → NO → **rebuild**
4. Output mtime changed from cached? → YES → hash output → output hash changed? → YES → **rebuild**
5. Input file set changed from cached? (compare current inputs vs cached inputs) → YES → **rebuild**
6. Any input mtime changed? → NO → **skip** (fast path)
7. For changed inputs, content hash changed? → NO → update mtimes, **skip**
8. → **rebuild**

### For `plate` layers (1-to-1 or many-to-1):

1. Cache entry exists? → NO → **run**
2. Command hash changed? → YES → **run**
3. Input mtime changed from cached? → NO → **skip**
4. Input content hash changed? → NO → update cached mtime, **skip**
5. → **run**

No output checks — plate steps are side-effectful with no output file to verify.

### New file detection

Glob results are stored per recipe in the cache. On each run:

1. Expand globs fresh
2. Compare against cached glob results
3. New files → cache miss for 1-to-1 (natural), invalidation for many-to-1
4. Removed files → prune stale cache entries

## Codegen Changes

### Before (current):

```lua
cook.recipe("build", {ingredients = {"src/*.c"}}, function()
    for _, _cook_in in ipairs(recipe.ingredients[1]) do
        local _cook_stem = path.stem(_cook_in)
        local _cook_out = "build/" .. _cook_stem .. ".o"
        cook.exec("gcc -c " .. _cook_in .. " -o " .. _cook_out, 3)
    end

    local _cook_all = table.concat(_cook_outputs_1, " ")
    local _cook_out = "build/lib.a"
    cook.exec("ar rcs " .. _cook_out .. " " .. _cook_all, 4)
end)
```

### After (with layer wrapping):

```lua
cook.recipe("build", {ingredients = {"src/*.c"}}, function()
    for _, _cook_in in ipairs(recipe.ingredients[1]) do
        local _cook_stem = path.stem(_cook_in)
        local _cook_out = "build/" .. _cook_stem .. ".o"
        cook.layer(_cook_in, _cook_out, 0x9f8e7d6c, function()
            cook.exec("gcc -c " .. _cook_in .. " -o " .. _cook_out, 3)
        end)
    end

    local _cook_all = table.concat(_cook_outputs_1, " ")
    local _cook_out = "build/lib.a"
    cook.layer(_cook_outputs_1, _cook_out, 0x3c4d5e6f, function()
        cook.exec("ar rcs " .. _cook_out .. " " .. _cook_all, 4)
    end)
end)
```

For `plate` steps, the output argument is `nil`:

```lua
cook.layer(_cook_in, nil, 0xaabb1234, function()
    cook.exec("./" .. _cook_in, 8)
end)
```

`cook.layer(inputs, output, command_hash, body_fn)` is a Rust-side API registered in the Lua runtime. It runs the rebuild-check algorithm and either calls `body_fn()` or skips it. Shell and lua steps are not wrapped — they always execute.

The `command_hash` is a numeric literal computed at codegen time (xxh3_64 of the template string or Lua block source text).

## Module Structure

```
src/
  cache/
    mod.rs      — cook.layer() API, glue between runtime and cache
    store.rs    — RecipeCache struct, bincode load/save
    check.rs    — needs_rebuild() algorithm
```

## New Dependencies

- `bincode` — binary serialization for cache files
- `xxhash-rust` — xxh3_64 content hashing (~30 GB/s)

## Design Principles

- **`cook` steps are a pure function contract**: given these inputs, produce this output. This is enforced by design now, verified by a borrow checker later.
- **`plate` steps are cacheable side effects**: same input tracking as `cook`, but no output verification. If inputs haven't changed, the side effect is skipped.
- **Cache is invisible by default**: no output about cache hits/misses unless `--verbose` is set.
- **Cache is disposable**: deleting `.cook/` is always safe, equivalent to a clean build.
- **Self-healing**: corrupted or version-mismatched cache files are treated as cache misses.
- **Incremental persistence**: cache is flushed after each successful layer via atomic write (temp + rename). Interrupted builds lose only the in-progress layer.

## Recipe-Level Invalidation

Some inputs affect all steps in a recipe but don't map to specific outputs. These trigger **full recipe cache invalidation** — the entire recipe re-runs:

- **Secondary ingredients** (e.g. `include/*.h` in `ingredients "src/*.c" "include/*.h"`): The cache stores `secondary_inputs_hash` — an xxh3 hash of all secondary ingredient files' content hashes concatenated. If any header changes, the hash changes, and all steps in the recipe invalidate.
- **Environment variables**: The cache stores `env_hash` — an xxh3 hash of the full `cook.env` table (sorted key-value pairs). If any env var changes, all steps invalidate.

On each run, before checking individual layers:
1. Compute current `secondary_inputs_hash` and `env_hash`
2. Compare against cached values
3. If either changed → discard entire recipe cache, rebuild all

This is conservative but correct. Fine-grained dependency tracking (knowing which header affects which source file) is deferred to the Lua module system.

## Known Limitations

- **No fine-grained header tracking**: changing one header invalidates all steps in the recipe, even those that don't depend on it. This is correct but wasteful for large projects.
- **No fine-grained env tracking**: changing one env var invalidates all steps, even those that don't read it.

## Future Work

- **`--verbose` flag**: show cache hit/miss/skip decisions per layer
- **`cook cache status` command**: inspect cache state for debugging
- **Lua module system for fine-grained deps**: language-specific modules (e.g. a cmake module) can emit dependency info to `.cook/deps/` — like `.d` files but generalized as a Cook concept. The cache layer reads these to enable per-step invalidation for headers, includes, imports, etc.
- **Borrow checker**: runtime verification that `cook`/`plate` steps only read declared inputs and only write declared outputs
- **SQLite backend**: optional feature flag for large projects where per-recipe files become unwieldy
- **Parallelism**: with layers as the unit of work and declared I/O, parallel execution of independent layers becomes possible
