# Dead Code Removal Design

**Date:** 2026-03-15
**Purpose:** Remove dead code paths identified during architecture review.

## Removals

### 1. `execute_recipe()` — runtime/mod.rs (~lines 156-208)

Old single-threaded execution path. Zero production callers — only called from tests. The live path is `register_recipe()` which captures work units for the DAG scheduler.

### 2. `list_recipes()` — runtime/mod.rs (~lines 210-234)

Registers a no-op `cook.layer` and returns recipe names. Zero production callers — only its own test calls it.

### 3. Tests and helpers — runtime/mod.rs (~lines 319-718)

23 tests that exercise `execute_recipe()` or `list_recipes()`, plus 2 helper functions (`make_runtime`, `make_runtime_with_env`). All dead once the functions above are removed.

Tests to remove: `test_runtime_executes_shell`, `test_runtime_recipe_not_found`, `test_runtime_command_failure`, `test_runtime_env_vars`, `test_cook_exec_returns_stdout`, `test_fs_exists`, `test_fs_size`, `test_fs_read`, `test_fs_glob`, `test_fs_mtime`, `test_recipe_context_name`, `test_recipe_context_ingredients`, `test_recipe_context_no_metadata`, `test_list_recipes`, `test_cook_sh_executes_and_returns_stdout`, `test_cook_sh_failure_reports_line_zero`, `test_path_stem`, `test_path_name`, `test_path_ext`, `test_path_dir`, `test_path_replace_ext`, `test_path_join`, `test_env_vars_passed_to_shell`.

Registration-mode tests (starting ~line 724) are KEPT — they test the live `register_recipe()` path.

### 4. `cook.taste` API registration — runtime/api.rs

Remove taste registration from both `register_cook_api()` (~lines 89-96) and `register_cook_api_capture()` (~lines 551-553). The parser continues to recognize the `taste` keyword (removing it would be a user-facing breaking change), and the `Step::Taste` AST variant and codegen skip logic are kept.

## Fixes

### 5. Error flow in `cli::run()` and `main.rs`

**Current (broken):** `cli::run()` calls `process::exit()` directly on error (line 116). The `main.rs` error handler never fires.

**Fixed:** `cli::run()` returns `Err(e)` instead of calling `process::exit()`. `main.rs` handles the error: prints it, calls `e.exit_code()`. This requires `CookError` to be exposed or the return type adjusted so `main()` can call `exit_code()`.

## Updates

### 6. Comment in runtime/mod.rs (~line 41-42)

Update comment referencing `execute_recipe` to only mention `register_recipe`.

## Not Removed

- **`CacheState`/`SharedCacheState`** — still used by `register_recipe()` via `setup_recipe_context()`
- **`Step::Taste` AST variant** — parser still recognizes it; removing would break existing Cookfiles
- **Taste skip in codegen** — still needed as long as the parser recognizes taste
