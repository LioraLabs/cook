# Module Boundaries Refactor — Human Execution Plan

**Spec:** `docs/superpowers/specs/2026-03-15-module-boundaries-design.md`

This is ordered so each step compiles and tests pass before moving to the next. Do one step per commit.

---

## Phase 1: Foundation (no existing code changes)

### Step 1: Create `contracts/` module

Start here because nothing depends on it yet — pure additive work.

1. Create `src/contracts/mod.rs`, `work.rs`, `capture.rs`, `hash.rs`
2. **Copy** (don't move yet) `WorkPayload` and `CacheMeta` from `src/scheduler/dag.rs` into `contracts/work.rs`
3. **Copy** `CapturedUnit`, `DepKind`, `RecipeUnits`, `CaptureState`, `SharedCaptureState` from `src/runtime/api.rs` into `contracts/capture.rs`
4. Move `hash_str` into `contracts/hash.rs` as a standalone pure function (it wraps `xxh3_64` — one line of logic). This is simpler than a trait and matches "pure types + zero logic."
5. Add `pub mod contracts;` to `lib.rs`
6. `cargo check` — should compile with no consumers yet

### Step 2: Add cache public API methods

Add to `ThreadSafeCacheManager` in `src/cache/mod.rs` (it'll move to `manager.rs` later, but do it here first so you can test against existing code):

1. Add `record_completion()` method to **`ThreadSafeCacheManager`** — extract the logic from `src/scheduler/mod.rs` lines ~183-215 and ~243-275 (they're identical). The method takes `(recipe_name, cache_key, command_hash, input_paths, output_path, working_dir)` and handles `FileRecord` construction + `stat_mtime` + `hash_file` internally.
2. Add `invalidate_recipe()` method to **`CacheState`** (not `ThreadSafeCacheManager`) — this runs during the single-threaded registration phase which uses `Rc<RefCell<CacheState>>`. Extract the cache invalidation logic from `src/runtime/mod.rs`'s `setup_recipe_context()` function. Look for where it checks env hash, secondary inputs hash, and glob snapshots against the cache. Signature: `invalidate_recipe(env_hash, secondary_inputs_hash, glob_snapshots, working_dir) -> bool`.
3. `cargo test` — everything should still pass since you haven't changed callers yet

---

## Phase 2: Migrate consumers to contracts + cache API

**Important: Steps 3 and 4 must be done together in one commit.** Rust uses nominal typing — if you delete types from `runtime/api.rs` and `scheduler/dag.rs` separately, you'll temporarily have two distinct Rust types with the same name (one in contracts, one in the original location), and they won't be interchangeable. The safe approach: in a single commit, make both modules re-export from contracts.

### Step 3: Migrate `scheduler/` + `runtime/` to contracts (do together)

1. In `src/scheduler/dag.rs`: delete `WorkPayload` and `CacheMeta` definitions, replace with `pub use crate::contracts::{WorkPayload, CacheMeta};` — this re-export keeps all existing import paths working
2. In `src/runtime/api.rs`: delete `CapturedUnit`, `DepKind`, `RecipeUnits`, `CaptureState`, `SharedCaptureState` definitions. Replace with `pub use crate::contracts::{CapturedUnit, DepKind, RecipeUnits, CaptureState, SharedCaptureState};`
3. In `src/runtime/api.rs`: change `WorkPayload`/`CacheMeta` imports from `crate::scheduler::dag` to `crate::contracts`
4. In `src/scheduler/builder.rs`: change imports of `CapturedUnit`, `DepKind`, `RecipeUnits` from `crate::runtime::api` to `crate::contracts`
5. In `src/scheduler/pool.rs`: update `WorkPayload` import to `crate::contracts` (pool.rs also imports this)
6. In `src/scheduler/mod.rs`: replace the two 30-line cache-update blocks (lines ~183-215 and ~243-275) with calls to `cache_manager.record_completion(...)`
7. In `src/runtime/mod.rs`: replace cache invalidation logic in `setup_recipe_context()` with call to `cache_state.invalidate_recipe(...)`
8. `cargo test`

### Step 4: Migrate `codegen/` to contracts

1. In `src/codegen/mod.rs`: replace `use crate::cache::check::hash_str` with `use crate::contracts::hash_str`
2. `cargo test`

### Step 5: Clean up re-exports

At this point, `scheduler/dag.rs` and `runtime/api.rs` are re-exporting types from contracts. Check all `use` statements project-wide and make everything import from `contracts` directly. Then remove the re-exports from `dag.rs` and `api.rs`.

Also update `src/scheduler/pool.rs` — it imports `register_fs_api` and `register_path_api` from `crate::runtime::api`. These functions stay in runtime, but make sure the import path still works (it will for now; it'll need updating again in Step 9 when `api.rs` gets decomposed).

`cargo test`

---

## Phase 3: Decompose mod.rs files

Each step: move functions into new files, update `mod.rs` to `pub mod` + re-export, `cargo test`. The mechanical process is always the same:

1. Create the new file (e.g., `parser/recipe.rs`)
2. Cut the functions from `mod.rs`, paste into the new file
3. Add `use super::*;` or specific imports at the top of the new file
4. Add `pub mod recipe;` and any needed re-exports to `mod.rs`
5. Fix visibility — functions called from other modules need `pub`, internal helpers can stay `pub(crate)` or `pub(super)`
6. `cargo check`, fix any import errors, `cargo test`

### Step 6: Decompose `cache/mod.rs`

Easiest one — just move manager types to `manager.rs`. Good warm-up.

- `mod.rs` -> `manager.rs`: `CacheState`, `SharedCacheState`, `ThreadSafeCacheManager` (including the new methods)
- `mod.rs` becomes: `pub mod manager; pub mod check; pub mod store;` + re-exports

### Step 7: Decompose `parser/mod.rs`

The big one by line count, but the splits are natural:

1. `recipe.rs` <- `parse_recipe()`, `parse_config_block()`
2. `cook_line.rs` <- `parse_cook_line()`, `parse_quoted_strings_parser()`, `parse_single_quoted_string()`, `strip_keyword()`
3. `lua_block.rs` <- `collect_lua_block()`, `count_brace_delta()`
4. `tests.rs` <- the entire `#[cfg(test)] mod tests { ... }` block
5. `mod.rs` keeps only `parse()` entry point + `pub mod` declarations + `ParseError`

**Tip:** Start with `tests.rs` — it's the largest chunk and a clean cut. Then `lua_block.rs` (self-contained state machine). Then `cook_line.rs`. Then `recipe.rs`.

**Visibility gotcha:** Many functions in `parser/mod.rs` are currently private (`fn`, not `pub fn`) — e.g., `parse_recipe`, `parse_cook_line`, `strip_keyword`, `collect_lua_block`, `count_brace_delta`. When you move them to submodule files, they need to become at least `pub(super)` so that `mod.rs`'s `parse()` function can still call them.

### Step 8: Decompose `codegen/mod.rs`

1. `recipe.rs` <- `generate()` walk logic, `generate_metadata()`
2. `cook_step.rs` <- `generate_cook_step()`, `cook_step_mode()`
3. `plate_step.rs` <- `generate_plate_step()`, `expand_plate_cmd()`
4. `template.rs` <- `expand_output_pattern()`, `expand_template_to_lua()`, `expand_template_with_env_fallback()`
5. `lua_string.rs` <- `escape_lua_string()`, `wrap_lua_string()`
6. `tests.rs` <- all unit tests

### Step 9: Decompose `runtime/`

Split the already-large `api.rs` and slim down `mod.rs`. **Do this in sub-steps — order matters:**

1. `fs_api.rs` <- `register_fs_api()` — self-contained, no shared state. Easy first cut.
2. `path_api.rs` <- `register_path_api()` — same, self-contained.
3. `cargo test` after each of the above.
4. `capture.rs` <- `register_cook_api_capture()`, `register_layer_api_capture()` — **this is the hard one.** `register_layer_api_capture` is a ~180-line function containing closures that capture `Rc<RefCell<...>>` state. Make sure all imports and closure captures come along.
5. `engine.rs` <- `Runtime` struct, `new()`, `set_quiet()`, `register_recipe()`
6. `context.rs` <- `setup_recipe_context()`
7. `tests.rs` <- unit tests
8. Delete `api.rs` (everything has been moved out)
9. **Update `scheduler/pool.rs`** — it imports `register_fs_api` and `register_path_api` from `crate::runtime::api`. Update to the new paths (e.g., `crate::runtime::fs_api::register_fs_api` or re-export from `runtime/mod.rs`).

### Step 10: Decompose `scheduler/mod.rs`

1. `executor.rs` <- `execute_dag()`, `process_ready()`, `cancel_subtree()`, `run_interactive_on_main()`
2. `tests.rs` <- all unit tests

### Step 11: Split `cli/` into `cli/` + `engine/`

Do this last — it touches the top of the call stack, so everything below should be stable first.

1. Create `src/engine/mod.rs`, `pipeline.rs`, `commands.rs`, `error.rs`
2. Move `CookError` -> `engine/error.rs`
3. Move `read_and_parse()`, `resolve_env()`, `cmd_run()` -> `engine/pipeline.rs`
4. Move `cmd_menu()`, `cmd_init()`, `cmd_serve()` -> `engine/commands.rs`
5. `cli/mod.rs` keeps only `Cli`, `Command`, `run()` (which now calls `engine::*`)
6. Add `pub mod engine;` to `lib.rs`
7. `cargo test`

---

## Phase 4: Documentation

### Step 12: Update `CLAUDE.md`

Add module structure rules:
- `mod.rs` is a facade: `pub mod` declarations, re-exports, and at most a small entry-point function
- All logic lives in named submodules
- Cross-module imports go through public APIs, never reach into submodule internals
- Shared types between modules live in `contracts/`

### Step 13: Update `docs/architecture/`

- Update `README.md` module map to include `contracts/` and `engine/`
- Update the dependency diagram
- Update individual module docs to reflect new file structure
- Update `execution-flow.md` code paths if line references changed

---

## Tips

- **Run `cargo test` after every file move.** Rust's module system will tell you immediately if you broke a visibility or import. Don't batch moves.
- **`use super::*`** is your friend when extracting to submodules — it imports everything from the parent module, so most code just works after the move. You can tighten imports later.
- **Watch for `pub(crate)` vs `pub`** — functions only called within the module can stay `pub(super)` or private. Functions called from other modules need `pub` or `pub(crate)`.
- **Git tip:** Use `git add -p` to stage moves carefully. If a step gets messy, `git stash` and try a smaller cut.
- **The order matters:** Phase 1 and 2 establish the new contracts and cache API while everything still works. Phase 3 is pure mechanical refactoring — no logic changes, just file moves. Phase 4 is documentation. Don't skip phases.

---

## Verification Checklist (after all steps)

- [ ] `cargo test` passes — same number of tests as before the refactor
- [ ] `cargo check` has no warnings (no unused imports, no dead code)
- [ ] Every `mod.rs` file is under ~50 lines (facades only)
- [ ] `grep -r "crate::cache::check::hash_str" src/` returns nothing outside `cache/` and `contracts/`
- [ ] `grep -r "crate::runtime::api::" src/` returns nothing (api.rs is gone)
- [ ] `grep -r "crate::scheduler::dag::WorkPayload" src/` returns nothing outside `scheduler/` (consumers import from contracts)
- [ ] No module imports implementation details from another module's subfiles
