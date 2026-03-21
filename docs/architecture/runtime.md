# Runtime: Lua VM, APIs, Two-Phase Execution

## Overview

The runtime (`src/runtime/mod.rs`, `src/runtime/api.rs`) manages the Lua VM that executes the generated Lua source. It has three responsibilities:

1. **Lua VM lifecycle** — create an `mlua::Lua` instance, register Cook's built-in API namespaces (`cook.*`, `fs.*`, `path.*`), then load and execute the generated source string.
2. **API registration** — expose all build-system primitives to Lua code as native Rust functions. Every shell command, filesystem query, and cache-aware transformation routes through these functions.
3. **Two execution modes** — the same Lua source runs twice for each recipe. The first run (capture mode, `register_recipe()`) suppresses side effects and records what work needs to happen. The second run (execute mode, `execute_recipe()`) actually runs that work. This two-phase design is what makes parallel scheduling possible.

---

## Runtime Struct

Defined at `src/runtime/mod.rs:34`:

```rust
pub struct Runtime {
    working_dir: PathBuf,
    env_vars: HashMap<String, String>,
    no_taste: bool,
    quiet: bool,
}
```

| Field | Purpose |
|---|---|
| `working_dir` | Absolute path to the directory containing the Cookfile. All relative paths in shell commands and filesystem APIs are resolved against this. |
| `env_vars` | The resolved environment variables for this build (from the `[env]` config section). Exposed to Lua as `cook.env` and passed as the environment to all spawned shell processes. |
| `no_taste` | When true, `cook.taste()` is silent. Used by automated/CI invocations. |
| `quiet` | Reserved for suppressing progress output (currently threaded through but not yet used in the API layer). |

`Runtime::new()` (line 139) takes `working_dir` and `env_vars`; `no_taste` and `quiet` default to `false` and are set via `set_no_taste()` / `set_quiet()` (lines 148, 152). The caller in `cmd_run()` (in `src/main.rs`) constructs the runtime, applies config, then calls either `register_recipe()` for the scheduler path or `execute_recipe()` for the legacy direct path.

---

## Lua VM Initialization

Each call to `register_recipe()` or `execute_recipe()` creates a **fresh** `mlua::Lua` instance (lines 157, 245). There is no shared VM across recipe invocations. This means each recipe starts with a clean global table and no state leaks between recipes.

After creating the VM, the runtime registers APIs in a fixed order:

1. `register_cook_api()` or `register_cook_api_capture()` — sets up the `cook` global table with all `cook.*` functions. Returns a `Rc<RefCell<Vec<RegisteredRecipe>>>` that accumulates recipe registrations as the Lua source executes.
2. `register_fs_api()` — sets up the `fs` global table.
3. `register_path_api()` — sets up the `path` global table.
4. `register_layer_api()` or `register_layer_api_capture()` — adds `cook.layer` to the existing `cook` table. This is registered after the others because it needs a `SharedCacheState` that is constructed after the VM is ready.

Then `lua.load(lua_source).exec()` runs the Lua source. This executes the top-level `cook.recipe(...)` calls, registering each recipe into the in-memory registry. The recipe function bodies are stored but not yet called.

Finally, the runtime looks up the target recipe by name, calls `setup_recipe_context()` to populate `recipe.ingredients` and run cache invalidation, then invokes the recipe function.

---

## API Reference

### `cook.*` namespace

Registered by `register_cook_api()` (execute mode, `src/runtime/api.rs:20`) or `register_cook_api_capture()` (capture mode, `src/runtime/api.rs:428`).

---

#### `cook.recipe(name, metadata, fn)`

Registers a recipe. The runtime records `name`, extracts `metadata.ingredients` and `metadata.requires` from the Lua table, stores a registry key for `fn`, and appends the entry to the recipes list. The function body is not called at registration time.

`metadata` fields:
- `ingredients` — array of glob pattern strings. Expanded by `setup_recipe_context()` before the recipe body runs.
- `requires` — array of recipe name strings. Used by the scheduler to build inter-recipe dependencies.

Both fields are optional; missing or empty fields produce empty Vecs.

---

#### `cook.exec(cmd, line)` → string

Execute `cmd` via `/bin/sh -c` in `working_dir` with `env_vars` merged into the process environment. Returns captured stdout as a string (also printed to stdout). Stderr is forwarded directly to the caller's stderr. On failure, raises a Lua error carrying the `COOK_CMD_FAILED:line:code:cmd` sentinel that `execute_recipe()` decodes into a `RuntimeError::CommandFailed`.

**Capture mode behaviour** (`src/runtime/api.rs:474`):
- If `inside_layer` is true: appends `(cmd, line)` to `layer_commands` (the layer body is doing a dry run to discover the command) and returns `""`.
- If `inside_layer` is false: creates a `CapturedUnit { payload: WorkPayload::Shell { cmd, line }, dep_kind: Sequential }` and appends it to `state.units`. Returns `""`.

In both capture-mode cases no process is spawned.

---

#### `cook.interactive(cmd, line)` → string

Like `cook.exec()` but runs with inherited stdio so the child process has a real terminal. Uses `Command::status()` instead of `Command::output()`, so stdout is not captured. Returns an empty string on success.

This is generated for steps prefixed with `@` in the Cookfile.

**Capture mode behaviour** (`src/runtime/api.rs:496`): Always creates a `CapturedUnit { payload: WorkPayload::Interactive { cmd, line }, dep_kind: Sequential }` regardless of `inside_layer`. Interactive steps are always sequential.

---

#### `cook.sh(cmd)` → string

User-facing shell utility. Calls `run_shell_command` with line number `0` (so failures report line 0 rather than a Cookfile line). Intended for use in Lua expressions that need a shell result to drive control flow (e.g., computing a version string).

**Capture mode behaviour** (`src/runtime/api.rs:516`) — this is the critical difference from `cook.exec`:
- If `inside_layer` is true: appends `(cmd, 0)` to `layer_commands` and returns `""`. Behaves the same as `cook.exec` inside a layer.
- If `inside_layer` is false: **executes immediately**, even in capture mode. The rationale is that `cook.sh()` is a utility function whose return value drives Lua control flow. Suppressing it would break recipes that use `cook.sh()` at the top level of the recipe body to decide what to build.

---

#### `cook.taste(line)`

Debugger breakpoint placeholder. In execute mode with `no_taste: false`, prints a message to stderr. Otherwise a no-op. In capture mode, always a no-op (`src/runtime/api.rs:552`). The DAG codegen path does not emit `cook.taste()` calls; this function exists for legacy compatibility.

---

#### `cook.env`

A Lua table populated with all entries from `env_vars` (`src/runtime/api.rs:99`, `556`). Keys and values are strings. Read-only by convention. Generated Lua code accesses environment variables via `cook.env["VAR"]` (from template expansion) or `cook.env.VAR` (from Lua block steps).

---

#### `cook.layer(inputs, output, cmd_hash, fn [, lua_code])`

Cache-aware execution wrapper. The core of Cook's incremental build support.

Arguments:
- `inputs` — string or table of strings: relative paths to input files
- `output` — string or nil: relative path to the output file (nil for plate/run-only steps)
- `cmd_hash` — u64: hash of the command text, computed at codegen time
- `fn` — Lua function: the body to execute if a rebuild is needed
- `lua_code` — optional string: raw Lua source of the body, used only in capture mode for `LuaChunk` work units (passed by codegen for `>{ }` blocks)

**Execute mode** (`src/runtime/api.rs:252`): Checks cache via `needs_rebuild_cook` or `needs_rebuild_plate` (depending on whether `output` is nil). If the cache says skip, returns without calling `fn`. If rebuild is needed, calls `fn`, then writes a new `StepEntry` to the cache recording the input mtimes, hashes, and command hash.

**Capture mode** (`src/runtime/api.rs:568`): Checks cache exactly the same way. If the cache says skip, records a presatisfied `CapturedUnit` with `cache_meta: None` (no work needed). If rebuild is needed:
- If `lua_code` is present: creates a `WorkPayload::LuaChunk` — the body will be re-executed as a Lua chunk in the worker thread.
- If `lua_code` is absent: sets `inside_layer = true`, calls `fn()` as a dry run (which causes any `cook.exec` / `cook.sh` inside it to push into `layer_commands` instead of executing), then pops the last captured command as the `WorkPayload::Shell`.

In both cases the captured unit is assigned a `DepKind` based on whether a step group is currently open (see below).

---

#### `cook.begin_step()` / `cook.end_step()`

Step group boundaries for parallelism.

**Execute mode**: Both are no-ops (`src/runtime/api.rs:105`).

**Capture mode** (`src/runtime/api.rs:532`, `544`):
- `begin_step()` appends a new empty group to `step_groups`, sets `current_group` to its index.
- `end_step()` sets `current_group` to `None`.

Any `cook.layer()` call executed while `current_group` is `Some(i)` gets `dep_kind: DepKind::StepGroup(i)` and its unit index is appended to `step_groups[i]`. Units outside a group get `dep_kind: DepKind::Sequential`.

---

### `fs.*` namespace

Registered by `register_fs_api()` (`src/runtime/api.rs:112`). All path arguments are joined to `working_dir` before the filesystem call, so relative paths work correctly from Lua.

| Function | Return | Notes |
|---|---|---|
| `fs.exists(path)` | bool | True if the path exists (file or directory) |
| `fs.size(path)` | u64 | File size in bytes; errors if path does not exist |
| `fs.read(path)` | string | Full file contents as a UTF-8 string; errors on read failure |
| `fs.glob(pattern)` | table | Array of absolute path strings matching the glob pattern |
| `fs.mtime(path)` | f64 | Modification time as seconds since UNIX epoch |

Note: `fs.glob()` returns absolute paths (the full `working_dir + pattern` expansion). This differs from `recipe.ingredients`, which returns relative paths. Callers that need relative paths from `fs.glob()` must strip the working directory prefix themselves.

---

### `path.*` namespace

Registered by `register_path_api()` (`src/runtime/api.rs:184`). Pure string manipulation using Rust's `std::path`. No filesystem access.

| Function | Return | Notes |
|---|---|---|
| `path.stem(p)` | string | Filename without extension (`"lib/matrix.c"` → `"matrix"`); dotfiles return full name (`".gitignore"` → `".gitignore"`) |
| `path.name(p)` | string | Basename with extension (`"lib/matrix.c"` → `"matrix.c"`) |
| `path.ext(p)` | string | Extension including dot (`"matrix.c"` → `".c"`); returns `""` if none |
| `path.dir(p)` | string | Parent directory (`"lib/matrix.c"` → `"lib"`); returns `""` for bare filenames |
| `path.replace_ext(p, ext)` | string | Replaces extension; leading dot on `ext` is optional (`"matrix.c", ".o"` and `"matrix.c", "o"` both give `"matrix.o"`) |
| `path.join(a, b)` | string | Concatenates path components (`"build/obj", "matrix.o"` → `"build/obj/matrix.o"`) |

---

## Two Execution Modes

### Capture Mode (`register_recipe()`)

Entry point: `src/runtime/mod.rs:239`.

The VM is initialized with the capture-mode API set: `register_cook_api_capture()` + `register_layer_api_capture()`. When the recipe function body executes:

- `cook.exec()` — records a `CapturedUnit` (or buffers into `layer_commands` if inside a layer). No process is spawned.
- `cook.interactive()` — records a `CapturedUnit` with `WorkPayload::Interactive`. No process is spawned.
- `cook.sh()` — inside a layer: buffers into `layer_commands`. Outside a layer: **executes immediately** (needed for control-flow).
- `cook.layer()` — checks cache; either records a presatisfied unit or dry-runs the body to extract the shell command, then records a unit that needs execution.
- `cook.begin_step()` / `cook.end_step()` — open and close step groups.

After the recipe function returns, `register_recipe()` assembles a `RecipeUnits` value from the `CaptureState` and returns it to the scheduler (line 290–296).

Purpose: discover what work exists and whether it is cached, without performing any work. The resulting `RecipeUnits` feeds into the DAG builder.

### Execute Mode (`execute_recipe()`)

Entry point: `src/runtime/mod.rs:156`.

The VM is initialized with the real API set: `register_cook_api()` + `register_layer_api()`. When the recipe function body executes:

- `cook.exec()` / `cook.sh()` — spawn a subprocess via `/bin/sh -c`, capture stdout, return it.
- `cook.interactive()` — spawn a subprocess with inherited stdio.
- `cook.layer()` — checks cache; if rebuild needed, calls the Lua body function which in turn calls `cook.exec()` / `cook.sh()` for real.
- `cook.begin_step()` / `cook.end_step()` — no-ops.

On success the cache is flushed. On failure, `execute_recipe()` walks the `mlua::Error` chain looking for the `COOK_CMD_FAILED:line:code:cmd` sentinel (the `find_cook_cmd_failed()` helper at line 303) and converts it into a structured `RuntimeError::CommandFailed`.

This is the **legacy single-threaded path**. The parallel scheduler does not call `execute_recipe()` — it calls `register_recipe()` to get the DAG, then dispatches work units to the thread pool individually. Each worker receives a `WorkPayload` and executes it directly (bypassing the Lua VM entirely for `Shell` payloads, or spinning up a fresh VM for `LuaChunk` payloads).

---

## Capture-Mode Data Structures

### `CaptureState` (`src/runtime/api.rs:404`)

Mutable state accumulated during capture-mode execution. Wrapped in `Rc<RefCell<_>>` and shared between all the capture-mode API closures via clone:

| Field | Type | Purpose |
|---|---|---|
| `inside_layer` | bool | True while `cook.layer()`'s dry-run body is executing |
| `layer_commands` | `Vec<(String, usize)>` | Commands buffered during the dry-run |
| `units` | `Vec<CapturedUnit>` | All captured work units, in execution order |
| `current_group` | `Option<usize>` | Index of the currently open step group, or None |
| `step_groups` | `Vec<Vec<usize>>` | Parallel groups: each inner vec contains unit indices |

### `CapturedUnit` (`src/runtime/api.rs:389`)

One unit of work extracted from the recipe:

```rust
pub struct CapturedUnit {
    pub payload: WorkPayload,
    pub cache_meta: Option<CacheMeta>,
    pub dep_kind: DepKind,
}
```

- `payload` — the work to execute: `WorkPayload::Shell { cmd, line }`, `WorkPayload::Interactive { cmd, line }`, or `WorkPayload::LuaChunk { code, input, output, ingredient_groups }`.
- `cache_meta` — `Some(CacheMeta)` if the unit needs to be executed; `None` if the cache check determined it is already up to date (presatisfied).
- `dep_kind` — how this unit relates to others in the recipe.

### `DepKind` (`src/runtime/api.rs:380`)

```rust
pub enum DepKind {
    StepGroup(usize),
    Sequential,
}
```

- `DepKind::StepGroup(i)` — this unit belongs to step group `i`. It can run in parallel with other units in the same group because they share no file dependencies (each processes a distinct input file).
- `DepKind::Sequential` — this unit is a sequential barrier. The DAG builder places it after all units from the previous step group, and before any units in the next group. Bare `cook.exec()` calls outside of `cook.begin_step()` / `cook.end_step()` always get `Sequential`.

### `RecipeUnits` (`src/runtime/api.rs:396`)

The complete output of `register_recipe()`:

```rust
pub struct RecipeUnits {
    pub recipe_name: String,
    pub deps: Vec<String>,
    pub units: Vec<CapturedUnit>,
    pub step_groups: Vec<Vec<usize>>,
}
```

- `recipe_name` — name of the recipe.
- `deps` — recipe names from `metadata.requires`. Used by the DAG builder to add inter-recipe edges.
- `units` — all captured units in order.
- `step_groups` — parallel tiers: `step_groups[i]` contains the indices into `units` of all units in step group `i`.

### How Step Groups are Built

1. `cook.begin_step()` fires → `step_groups.push(Vec::new())`, `current_group = Some(len - 1)`.
2. Each `cook.layer()` call during the group → unit appended to `units`, its index appended to `step_groups[current_group]`, unit gets `dep_kind: StepGroup(current_group)`.
3. `cook.end_step()` fires → `current_group = None`.

After capture, `RecipeUnits.step_groups[0]` contains the indices of all units in the first `cook`/`plate` step, `step_groups[1]` the second, and so on. The DAG builder uses these to assign all units within a group to the same parallelism tier.

---

## Recipe Context Setup (`setup_recipe_context()`)

`src/runtime/mod.rs:44`. Called by both `execute_recipe()` and `register_recipe()` immediately before invoking the recipe function. Sets up the Lua `recipe` global and handles recipe-level cache invalidation.

### Cache Invalidation

Computes two hashes over the current build state:

- `current_env_hash` — hash of all `env_vars` key-value pairs.
- `current_secondary_hash` — hash of all files matched by each `ingredients` glob pattern (file contents or mtimes).

If either hash differs from what is stored in the on-disk `RecipeCache`, the entire cache for this recipe is discarded (`state.cache = RecipeCache::new()`, `state.dirty = true`). This ensures that a change to the environment or to any ingredient file causes all steps to be treated as stale.

The new hash values are then written back to the cache state (line 65–67).

### Ingredient Glob Resolution

For each pattern in `recipe.metadata.ingredients` (the list from the `ingredients` declaration in the Cookfile), the runtime:

1. Joins the pattern to `working_dir` to form a full glob pattern.
2. Runs `glob::glob()` to expand it.
3. Strips the `working_dir` prefix from each result to get relative paths.
4. Stores the relative paths in a Lua table.

The resulting nested table is written to `recipe_table["ingredients"]` with 1-based indexing: `recipe.ingredients[1]` is the array of paths matching the first glob pattern, `recipe.ingredients[2]` the second, and so on. This matches the `recipe.ingredients[N]` access pattern that codegen emits.

### Glob Recording for New-File Detection

After building the Lua table, the runtime also records the glob results in `cache_state.cache.globs` (line 98–132). On each run, it compares the current glob result against the stored one. If any file that was previously in the glob result is no longer present, all `StepEntry` records that reference that file as an input are evicted from the cache. This ensures that deleting a source file correctly invalidates any steps that depended on it.
