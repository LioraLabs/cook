# Dead Code Removal Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove dead code paths and the taste feature identified during architecture review.

**Architecture:** Pure deletion and cleanup. Remove `execute_recipe()`, `list_recipes()`, `register_cook_api`, `register_layer_api`, the taste feature entirely, the `--no-taste` CLI flag, the `no_taste` Runtime field, and fix the error flow in `cli::run()` to return errors instead of calling `process::exit()`.

**Tech Stack:** Rust

**Spec:** `docs/superpowers/specs/2026-03-15-dead-code-removal-design.md`

---

## File Structure

All changes are modifications to existing files — no new files.

```
src/main.rs                — Fix error handling to use exit_code()
src/cli/mod.rs             — Remove --no-taste flag, fix run() to return errors, remove set_no_taste call
src/runtime/mod.rs         — Remove execute_recipe(), list_recipes(), no_taste field, set_no_taste(), dead tests
src/runtime/api.rs         — Remove register_cook_api(), register_layer_api(), taste registrations, no_taste param
src/parser/lexer.rs        — Remove Taste token variant and tests
src/parser/mod.rs          — Remove taste parsing and test
src/parser/ast.rs          — Remove Step::Taste variant and test usage
src/codegen/mod.rs         — Remove taste skip logic and test
tests/integration.rs       — Remove --no-taste from all test invocations, remove taste-test recipe
```

---

## Chunk 1: Runtime Dead Code Removal

### Task 1: Remove `execute_recipe()`, `list_recipes()`, and dead tests from runtime/mod.rs

**Files:**
- Modify: `src/runtime/mod.rs`

- [ ] **Step 1: Remove `execute_recipe()` function**

Delete lines 156-208 (the `pub fn execute_recipe` method).

- [ ] **Step 2: Remove `list_recipes()` function**

Delete lines 210-234 (the `pub fn list_recipes` method). After this deletion, the next function should be `register_recipe()`.

- [ ] **Step 3: Remove `no_taste` field and `set_no_taste()` method**

In the `Runtime` struct, remove the `no_taste: bool` field (line 37).
Remove the `set_no_taste()` method (lines 148-150).
In `Runtime::new()`, remove `no_taste: false` from the initializer (line 143).

- [ ] **Step 4: Remove dead tests (lines 327-718)**

Delete all tests from `test_runtime_executes_shell` through `test_env_vars_passed_to_shell`. Keep the `// Registration-mode tests` comment and everything after it (line 720+).

Do NOT remove `make_runtime()` or `make_runtime_with_env()` — they're used by the live registration-mode tests.

- [ ] **Step 5: Update comment on `setup_recipe_context`**

Change the comment at line 41-42 from:
```
/// Shared helper for recipe-level cache invalidation, context table setup,
/// ingredient resolution, and glob recording. Used by both execute_recipe and
/// register_recipe.
```
to:
```
/// Helper for recipe-level cache invalidation, context table setup,
/// ingredient resolution, and glob recording. Used by register_recipe.
```

- [ ] **Step 6: Remove dead imports**

Remove `register_cook_api` and `register_layer_api` from the import at line 4 (keep `register_cook_api_capture`, `register_fs_api`, `register_layer_api_capture`, `register_path_api`).

Remove any now-unused imports (e.g., `Rc`, `RefCell` if no longer used — check first since `register_recipe` may still use them via `SharedCacheState`).

- [ ] **Step 7: Run tests**

Run: `cargo test -p cook --lib runtime`
Expected: All remaining registration-mode tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/runtime/mod.rs
git commit -m "refactor(runtime): remove dead execute_recipe, list_recipes, and no_taste"
```

---

### Task 2: Remove `register_cook_api()`, `register_layer_api()`, and taste from runtime/api.rs

**Files:**
- Modify: `src/runtime/api.rs`

- [ ] **Step 1: Remove `register_cook_api()` function**

Delete the entire `pub fn register_cook_api(...)` function (starts at line 20). This is the non-capture-mode API registration that was only used by the now-deleted `execute_recipe()` and `list_recipes()`.

- [ ] **Step 2: Remove `register_layer_api()` function**

Delete the entire `pub fn register_layer_api(...)` function (starts at line 252). This is the non-capture-mode layer API that was only used by `execute_recipe()`.

- [ ] **Step 3: Remove taste registration from `register_cook_api_capture()`**

In the `register_cook_api_capture()` function, remove the taste registration block:
```rust
    // cook.taste — no-op in capture mode
    let taste_fn = lua.create_function(|_, _line: usize| Ok(()))?;
    cook.set("taste", taste_fn)?;
```

Also remove the `no_taste` parameter from `register_cook_api_capture`'s signature if it has one (check — it may not).

- [ ] **Step 4: Run tests**

Run: `cargo test -p cook --lib`
Expected: All tests pass. Some may fail due to removed imports — fix compilation errors.

- [ ] **Step 5: Commit**

```bash
git add src/runtime/api.rs
git commit -m "refactor(api): remove dead register_cook_api, register_layer_api, and taste"
```

---

## Chunk 2: Parser and Codegen Cleanup

### Task 3: Remove taste from parser and codegen

**Files:**
- Modify: `src/parser/lexer.rs`
- Modify: `src/parser/mod.rs`
- Modify: `src/parser/ast.rs`
- Modify: `src/codegen/mod.rs`

- [ ] **Step 1: Remove `Token::Taste` from lexer**

In `src/parser/lexer.rs`:
- Remove `Taste` from the `Token` enum (line 12)
- Remove the `taste` keyword recognition (lines 88-89: `} else if trimmed == "taste" { Token::Taste }`) — this should fall through to `Content` instead
- Remove `"taste"` from the keyword list in the `is_keyword()` check (line 54)
- Remove test `test_taste()` (lines 238-242)
- Update test `test_taste_with_args_is_shell` (lines 245-248) — rename to something like `test_taste_is_shell_command` since taste is no longer a keyword. The test should now verify that `taste test` is `Content("taste test")` — but also add a test that bare `taste` is also `Content("taste")`.

- [ ] **Step 2: Remove `Step::Taste` from AST**

In `src/parser/ast.rs`:
- Remove `Taste { line: usize }` from the `Step` enum (line 39)
- Remove the `Step::Taste { line: 5 }` usage in the test (line 64) — replace with another step or remove that line from the test

- [ ] **Step 3: Remove taste parsing from parser**

In `src/parser/mod.rs`:
- Remove the `Token::Taste` arm from the match in global scope (line 77 — remove `Token::Taste` from the pattern `Token::LuaLine(_) | Token::LuaBlockOpen | Token::Taste`)
- Remove the `Token::Taste` match arm inside `parse_recipe()` (lines 337-338)
- Remove test `test_taste_step` (lines 699-702)

- [ ] **Step 4: Remove taste skip from codegen**

In `src/codegen/mod.rs`:
- Remove the `Step::Taste { .. }` match arm (lines 45-47)
- Remove test `test_taste_skipped` (lines 614-623)

- [ ] **Step 5: Run tests**

Run: `cargo test -p cook --lib`
Expected: All tests pass. Fix any compilation errors from removed variants.

- [ ] **Step 6: Commit**

```bash
git add src/parser/lexer.rs src/parser/mod.rs src/parser/ast.rs src/codegen/mod.rs
git commit -m "refactor(parser,codegen): remove taste feature entirely"
```

---

## Chunk 3: CLI Error Flow Fix and Integration Test Cleanup

### Task 4: Fix error flow in `cli::run()` and `main.rs`

**Files:**
- Modify: `src/cli/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Add `std::error::Error` impl for `CookError`**

In `src/cli/mod.rs`, after the `impl std::fmt::Display for CookError` block, add:
```rust
impl std::error::Error for CookError {}
```

- [ ] **Step 2: Change `run()` return type and remove `process::exit()`**

Change `run()` to return `Result<(), CookError>` instead of `Result<(), Box<dyn std::error::Error>>`.

Replace the error match arm (lines 112-118):
```rust
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("cook: {e}");
            std::process::exit(e.exit_code());
        }
    }
```
with:
```rust
    result
```

Make `CookError` and `exit_code()` public so `main.rs` can use them:
- Change `enum CookError` to `pub enum CookError`
- Change `fn exit_code` to `pub fn exit_code`

- [ ] **Step 3: Remove `--no-taste` flag from CLI**

Remove the `no_taste` field from the `Cli` struct (lines 33-35).
Remove `rt.set_no_taste(cli.no_taste);` from `cmd_run()` (line 216).

- [ ] **Step 4: Update `main.rs` to handle `CookError`**

Replace the contents of `src/main.rs` with:
```rust
use cook::cli;

fn main() {
    if let Err(e) = cli::run() {
        eprintln!("cook: {e}");
        std::process::exit(e.exit_code());
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p cook --lib`
Expected: All library tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/cli/mod.rs src/main.rs
git commit -m "refactor(cli): fix error flow, remove --no-taste flag"
```

---

### Task 5: Remove taste references from integration tests

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: Remove `--no-taste` from all test invocations**

Search for `"--no-taste"` in `tests/integration.rs` and remove it from all `.args()` calls. There are approximately 6 occurrences (lines 522, 823, 857, 870, 925, 1055).

For example, change:
```rust
.args(["--no-taste", "all"])
```
to:
```rust
.args(["all"])
```

And change:
```rust
.args(["--no-taste", "--set", "MSG=overridden", "build"])
```
to:
```rust
.args(["--set", "MSG=overridden", "build"])
```

- [ ] **Step 2: Remove taste-test recipe and assertions**

In the stress test (`test_cook_stress_all_features`):
- Remove the `taste-test` recipe from the Cookfile string (lines ~505-509)
- Remove `"taste-test"` from the `"all"` recipe dependency list (line ~513)
- Remove the taste assertion block (lines ~572-579)

- [ ] **Step 3: Run all tests**

Run: `cargo test`
Expected: All 195+ unit tests and 33 integration tests pass (count will decrease slightly due to removed tests).

- [ ] **Step 4: Commit**

```bash
git add tests/integration.rs
git commit -m "test: remove taste references and --no-taste flag from integration tests"
```
