# Lua-Build cpp Module Integration — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the lua-build example to use the cpp.lua cook_module. The existing `ingredients` line automatically exposes a `local ingredients` Lua variable (a flat table of resolved file paths) inside recipe bodies, bridging Cook's DSL with Lua module APIs.

**Architecture:** Two changes to the codegen+runtime layers (no parser/AST changes). Codegen emits `local ingredients = cook.resolve_ingredients({...}, {...})` at the top of any recipe body that has an `ingredients` line. A new runtime function `cook.resolve_ingredients()` performs glob+exclude resolution and returns a Lua table. The existing `{in}/{out}/{stem}/{all}` template chain is unaffected.

**Tech Stack:** Rust (cook-luagen codegen, cook-register runtime), Lua (cpp.lua module), Cook DSL

---

## Chunk 1: Ingredients as Lua Variable

### Task 1: Runtime — Register `cook.resolve_ingredients()` Lua function

**Files:**
- Modify: `cli/crates/cook-register/src/context.rs`
- Modify: `cli/crates/cook-register/src/engine.rs:37-104`
- Test: `cli/crates/cook-register/src/tests.rs`

The runtime needs a `cook.resolve_ingredients(includes, excludes)` function that:
1. Takes two Lua tables (include patterns, exclude patterns)
2. Resolves globs against the working directory
3. Filters out excludes
4. Returns a flat Lua table of relative file paths

This reuses the existing `resolve_glob` function already in `context.rs:46-68`.

- [ ] **Step 1: Write a failing test for resolve_ingredients**

In `cli/crates/cook-register/src/tests.rs`, add a test. The test needs a temp directory with files to glob against, and must create a `cook` table on the Lua global before calling the registration function.

```rust
#[test]
fn test_resolve_ingredients_api() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.c"), "").unwrap();
    fs::write(dir.path().join("b.c"), "").unwrap();
    fs::write(dir.path().join("skip.c"), "").unwrap();

    let lua = mlua::Lua::new();
    // Must create the cook table first — register_resolve_ingredients adds to it
    let cook = lua.create_table().unwrap();
    lua.globals().set("cook", cook).unwrap();

    crate::context::register_resolve_ingredients(&lua, dir.path()).unwrap();

    let result: Vec<String> = lua
        .load(r#"return cook.resolve_ingredients({"*.c"}, {"skip.c"})"#)
        .eval::<mlua::Table>()
        .unwrap()
        .sequence_values::<String>()
        .filter_map(|v| v.ok())
        .collect();

    assert!(result.contains(&"a.c".to_string()));
    assert!(result.contains(&"b.c".to_string()));
    assert!(!result.contains(&"skip.c".to_string()));
    assert_eq!(result.len(), 2);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p cook-register test_resolve_ingredients 2>&1 | tail -20`
Expected: FAIL — `register_resolve_ingredients` doesn't exist yet.

- [ ] **Step 3: Implement `cook.resolve_ingredients()`**

In `cli/crates/cook-register/src/context.rs`, add a public function that registers `cook.resolve_ingredients` on the existing cook global table. This reuses the `resolve_glob` function at line 46:

```rust
/// Register `cook.resolve_ingredients(includes, excludes)` on the cook global table.
/// Returns a flat Lua table of relative file paths after glob+exclude resolution.
pub fn register_resolve_ingredients(lua: &Lua, working_dir: &Path) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let wd = working_dir.to_path_buf();
    let resolve_fn = lua.create_function(move |lua, (includes, excludes): (LuaTable, LuaTable)| {
        // Collect exclude patterns and resolve them
        let mut excluded: BTreeSet<String> = BTreeSet::new();
        for exc in excludes.sequence_values::<String>() {
            let pattern = exc.map_err(|e| mlua::Error::runtime(format!("bad exclude: {e}")))?;
            excluded.extend(resolve_glob(&wd, &pattern));
        }

        // Resolve include patterns, filtering out excludes
        let mut result: Vec<String> = Vec::new();
        for inc in includes.sequence_values::<String>() {
            let pattern = inc.map_err(|e| mlua::Error::runtime(format!("bad include: {e}")))?;
            let files = resolve_glob(&wd, &pattern);
            for f in files {
                if !excluded.contains(&f) {
                    result.push(f);
                }
            }
        }

        // Build Lua table
        let table = lua.create_table()?;
        for (i, file) in result.iter().enumerate() {
            table.set(i + 1, file.as_str())?;
        }
        Ok(table)
    })?;
    cook.set("resolve_ingredients", resolve_fn)?;
    Ok(())
}
```

Then call this from `cli/crates/cook-register/src/engine.rs` in the `register_recipe` method, after the test API registration (line 65) and before `lua.load(lua_source).exec()` (line 67):

```rust
crate::context::register_resolve_ingredients(&lua, &self.working_dir)?;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cook-register 2>&1 | tail -20`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-register/src/context.rs cli/crates/cook-register/src/engine.rs cli/crates/cook-register/src/tests.rs
git commit -m "feat(cook-register): add cook.resolve_ingredients() runtime function"
```

---

### Task 2: Codegen — Emit `local ingredients = cook.resolve_ingredients(...)` for recipes with ingredients

**Files:**
- Modify: `cli/crates/cook-luagen/src/recipe.rs:8-101`
- Test: `cli/crates/cook-luagen/src/tests.rs`

When a recipe has `ingredients` (i.e., `recipe.ingredients` is non-empty), the codegen should emit a `local ingredients = cook.resolve_ingredients({...}, {...})` line at the top of the recipe function body. This makes the resolved file list available as a Lua variable alongside the existing template chain.

- [ ] **Step 1: Write a failing codegen test**

In `cli/crates/cook-luagen/src/tests.rs`, there's an existing `make_recipe` helper at line 17. Add a test that verifies the `ingredients` local is emitted:

```rust
#[test]
fn test_ingredients_lua_variable_emitted() {
    let recipe = make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Lua {
            code: "print(ingredients)".to_string(),
            line: 3,
        }],
    );
    let cookfile = make_cookfile(vec![recipe]);
    let output = generate(&cookfile);
    assert!(
        output.contains("local ingredients = cook.resolve_ingredients("),
        "should emit ingredients variable, got:\n{}",
        output
    );
    assert!(output.contains("\"src/*.c\""));
}

#[test]
fn test_ingredients_lua_variable_with_excludes() {
    let mut recipe = make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Lua {
            code: "print(ingredients)".to_string(),
            line: 3,
        }],
    );
    recipe.excludes = vec!["src/skip.c".to_string()];
    let cookfile = make_cookfile(vec![recipe]);
    let output = generate(&cookfile);
    assert!(output.contains("\"src/*.c\""));
    assert!(output.contains("\"src/skip.c\""));
}

#[test]
fn test_no_ingredients_no_variable() {
    let recipe = make_recipe(
        "clean",
        vec![],
        vec![],
        vec![Step::Shell {
            command: "rm -rf build".to_string(),
            line: 2,
            interactive: false,
        }],
    );
    let cookfile = make_cookfile(vec![recipe]);
    let output = generate(&cookfile);
    assert!(
        !output.contains("cook.resolve_ingredients"),
        "should NOT emit ingredients variable for recipe without ingredients"
    );
}
```

Note: The existing `make_recipe` helper doesn't set excludes. The test for excludes manually sets `recipe.excludes` after construction. Also check if a `make_cookfile` helper exists — if not, inline the `Cookfile` construction.

- [ ] **Step 2: Run to verify tests fail**

Run: `cargo test -p cook-luagen test_ingredients_lua_variable 2>&1 | tail -20`
Expected: FAIL — codegen doesn't emit the `ingredients` variable yet.

- [ ] **Step 3: Implement codegen for ingredients variable**

In `cli/crates/cook-luagen/src/recipe.rs`, in the `generate` function, add the `local ingredients` emission right after the `cook.recipe(...)` opening line (after line 29) and before the step loop (line 31):

```rust
// Emit local ingredients variable when recipe has ingredients
if !recipe.ingredients.is_empty() {
    let includes: Vec<String> = recipe.ingredients.iter()
        .map(|s| format!("\"{}\"", escape_lua_string(s)))
        .collect();
    let excludes: Vec<String> = recipe.excludes.iter()
        .map(|s| format!("\"{}\"", escape_lua_string(s)))
        .collect();
    out.push_str(&format!(
        "    local ingredients = cook.resolve_ingredients({{{}}}, {{{}}})\n",
        includes.join(", "),
        excludes.join(", "),
    ));
}
```

This goes between the `cook.recipe(...)` line and the `let mut prev_cook_index` line. It emits nothing when the recipe has no ingredients, preserving backward compatibility.

- [ ] **Step 4: Run codegen tests to verify they pass**

Run: `cargo test -p cook-luagen 2>&1 | tail -20`
Expected: All tests pass, including existing tests (no regressions).

- [ ] **Step 5: Run the full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: All tests pass across all crates.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-luagen/src/recipe.rs cli/crates/cook-luagen/src/tests.rs
git commit -m "feat(cook-luagen): emit local ingredients variable for recipes with ingredients"
```

---

## Chunk 2: Lua-Build Example Refactoring

### Task 3: Copy cpp.lua module into lua-build example

**Files:**
- Create: `examples/lua-build/cook_modules/cpp.lua` (copy from `examples/cpp-project/cook_modules/cpp.lua`)

- [ ] **Step 1: Create cook_modules directory and copy cpp.lua**

```bash
mkdir -p examples/lua-build/cook_modules
cp examples/cpp-project/cook_modules/cpp.lua examples/lua-build/cook_modules/cpp.lua
```

- [ ] **Step 2: Commit**

```bash
git add examples/lua-build/cook_modules/cpp.lua
git commit -m "feat(examples): add cpp.lua module to lua-build example"
```

---

### Task 4: Rewrite the lua-build Cookfile

**Files:**
- Modify: `examples/lua-build/Cookfile`

- [ ] **Step 1: Rewrite the Cookfile**

Replace the entire content of `examples/lua-build/Cookfile` with:

```
use cpp

recipe liblua
    ingredients "lua-5.4.7/src/*.c" !"lua-5.4.7/src/lua.c" !"lua-5.4.7/src/luac.c"
    cpp.static_library("liblua", {
        sources = ingredients,
        defines = cook.platform.os == "linux" and { "LUA_USE_LINUX" } or {},
        system_libs = { "m", "dl" },
    })
end

recipe lua: liblua
    cpp.executable("lua", {
        sources = { "lua-5.4.7/src/lua.c" },
        links = { "liblua" },
        defines = cook.platform.os == "linux" and { "LUA_USE_LINUX" } or {},
        system_libs = { "m", "dl", "readline" },
    })
end

recipe luac: liblua
    cpp.executable("luac", {
        sources = { "lua-5.4.7/src/luac.c" },
        links = { "liblua" },
        defines = cook.platform.os == "linux" and { "LUA_USE_LINUX" } or {},
        system_libs = { "m", "dl" },
    })
end

recipe build: lua luac
end

recipe test: lua
    test "build/bin/lua -e 'print(\"hello from lua built by cook\")'"
end

recipe compile-commands: liblua lua luac
    cpp.compile_commands()
end

recipe clean
    rm -rf build .cook
end
```

- [ ] **Step 2: Commit**

```bash
git add examples/lua-build/Cookfile
git commit -m "feat(examples): rewrite lua-build to use cpp module with ingredients variable"
```

---

### Task 5: End-to-end verification

**Files:** None (verification only)

- [ ] **Step 1: Clean and rebuild lua-build from scratch**

```bash
cd examples/lua-build
rm -rf build .cook
cargo run --manifest-path ../../cli/Cargo.toml -- build
```

Expected: Compiles all 32 .c files, archives liblua.a, links lua and luac binaries. No errors.

- [ ] **Step 2: Run the test recipe**

```bash
cargo run --manifest-path ../../cli/Cargo.toml -- test
```

Expected: Prints "hello from lua built by cook".

- [ ] **Step 3: Verify incremental rebuild skips unchanged files**

```bash
cargo run --manifest-path ../../cli/Cargo.toml -- build
```

Expected: Second run should be faster (cache hits on unchanged files).

- [ ] **Step 4: Verify header dependency tracking**

```bash
touch lua-5.4.7/src/luaconf.h
cargo run --manifest-path ../../cli/Cargo.toml -- build
```

Expected: Files that include `luaconf.h` should recompile, others should be skipped.

- [ ] **Step 5: Generate compile_commands.json**

```bash
cargo run --manifest-path ../../cli/Cargo.toml -- compile-commands
```

Expected: `compile_commands.json` is generated in the lua-build directory with entries for all source files.

- [ ] **Step 6: Verify the cpp-project example still works**

```bash
cd ../cpp-project
rm -rf build .cook
cargo run --manifest-path ../../cli/Cargo.toml -- build
cargo run --manifest-path ../../cli/Cargo.toml -- run-tests
```

Expected: No regressions — cpp-project builds and tests pass.

- [ ] **Step 7: Run the full test suite one final time**

```bash
cargo test
```

Expected: All unit and integration tests pass.

- [ ] **Step 8: Final commit if any fixups needed, otherwise done**

If any fixes were needed during verification, commit them:

```bash
git add -A
git commit -m "fix: address issues found during lua-build integration testing"
```
