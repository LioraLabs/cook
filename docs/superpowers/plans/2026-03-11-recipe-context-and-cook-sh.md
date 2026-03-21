# Recipe Context & cook.sh Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose resolved recipe metadata as `recipe.*` in Lua bodies and add `cook.sh(cmd)` as the user-facing shell API.

**Architecture:** Two changes to the runtime layer. (1) `RegisteredRecipe` stores raw metadata at registration time; `execute_recipe` resolves ingredient globs and sets a `recipe` Lua global before invoking the function. (2) A new `cook.sh(cmd)` Lua function wraps `cook.exec` without requiring a line number.

**Tech Stack:** Rust, mlua (Lua 5.4), glob crate

**Spec:** `docs/superpowers/specs/2026-03-11-recipe-context-and-cook-sh.md`

---

## Chunk 1: Store metadata in RegisteredRecipe and add cook.sh

### Task 1: Store metadata in RegisteredRecipe

**Files:**
- Modify: `src/runtime/api.rs:8-11` (RegisteredRecipe struct)
- Modify: `src/runtime/api.rs:24-31` (recipe registration closure)
- Test: `src/runtime/mod.rs` (existing tests, must still pass)

- [ ] **Step 1: Add metadata field to RegisteredRecipe**

In `src/runtime/api.rs`, change the struct:

```rust
pub struct RegisteredRecipe {
    pub name: String,
    pub function: LuaRegistryKey,
    pub metadata: RegisteredMetadata,
}

/// Raw metadata stored at registration time, resolved at execution time.
pub struct RegisteredMetadata {
    pub ingredients: Vec<String>,
    pub serves: Option<ServedValue>,
}

/// Serves can be a single path or multiple paths.
pub enum ServedValue {
    Single(String),
    Multiple(Vec<String>),
}
```

- [ ] **Step 2: Update the recipe registration closure to store metadata**

In `src/runtime/api.rs`, change the `recipe_fn` closure (line 24-31). Replace `_meta: LuaValue` with `meta: LuaTable`:

```rust
let recipe_fn =
    lua.create_function(move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
        let key = lua.create_registry_value(func)?;

        // Extract ingredients: table of strings, or empty
        let mut ingredients = Vec::new();
        if let Ok(ing_table) = meta.get::<LuaTable>("ingredients") {
            for pair in ing_table.sequence_values::<String>() {
                if let Ok(s) = pair {
                    ingredients.push(s);
                }
            }
        }

        // Extract serves: string or table of strings, or None
        let serves = if let Ok(s) = meta.get::<String>("serves") {
            Some(ServedValue::Single(s))
        } else if let Ok(t) = meta.get::<LuaTable>("serves") {
            let mut items = Vec::new();
            for pair in t.sequence_values::<String>() {
                if let Ok(s) = pair {
                    items.push(s);
                }
            }
            Some(ServedValue::Multiple(items))
        } else {
            None
        };

        recipes_clone.borrow_mut().push(RegisteredRecipe {
            name,
            function: key,
            metadata: RegisteredMetadata { ingredients, serves },
        });
        Ok(())
    })?;
```

- [ ] **Step 3: Run existing tests to verify nothing broke**

Run: `cargo test`
Expected: All 97 tests pass. The metadata is now stored but not yet used.

- [ ] **Step 4: Commit**

```bash
git add src/runtime/api.rs
git commit -m "feat: store recipe metadata in RegisteredRecipe for later use"
```

---

### Task 2: Add cook.sh function

**Files:**
- Modify: `src/runtime/api.rs:34-68` (add cook.sh next to cook.exec)
- Test: `src/runtime/mod.rs` (new test)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/runtime/mod.rs`:

```rust
#[test]
fn test_cook_sh_executes_and_returns_stdout() {
    let dir = TempDir::new().unwrap();
    let rt = make_runtime(dir.path());
    let lua_src = r#"
cook.recipe("check", {}, function()
    local out = cook.sh("echo hello_sh")
    assert(out == "hello_sh\n", "expected 'hello_sh\\n' but got: '" .. out .. "'")
end)
"#;
    let result = rt.execute_recipe(lua_src, "check");
    assert!(result.is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_cook_sh_executes_and_returns_stdout`
Expected: FAIL — `cook.sh` is not defined yet.

- [ ] **Step 3: Write the cook.sh implementation**

In `src/runtime/api.rs`, after the `cook.exec` registration (after line 69), add:

```rust
// cook.sh(cmd) -> stdout string (user-facing, no line number)
let wd2 = working_dir.clone();
let env2 = env_vars.clone();
let sh_fn = lua.create_function(move |_, cmd: String| {
    let mut child_env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in &env2 {
        child_env.insert(k.clone(), v.clone());
    }

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(&cmd)
        .current_dir(&wd2)
        .envs(&child_env)
        .output()
        .map_err(|e| mlua::Error::runtime(format!("failed to execute: {e}")))?;

    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        return Err(mlua::Error::runtime(format!(
            "COOK_CMD_FAILED:0:{}:{}",
            code, cmd
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    Ok(stdout)
})?;
cook.set("sh", sh_fn)?;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test test_cook_sh_executes_and_returns_stdout`
Expected: PASS

- [ ] **Step 5: Write test for cook.sh failure with line-0**

Add to `src/runtime/mod.rs` tests:

```rust
#[test]
fn test_cook_sh_failure_reports_line_zero() {
    let dir = TempDir::new().unwrap();
    let rt = make_runtime(dir.path());
    let lua_src = r#"
cook.recipe("fail", {}, function()
    cook.sh("false")
end)
"#;
    let result = rt.execute_recipe(lua_src, "fail");
    assert!(result.is_err());
    match result.unwrap_err() {
        RuntimeError::CommandFailed { command, line, code } => {
            assert_eq!(command, "false");
            assert_eq!(line, 0);
            assert_eq!(code, 1);
        }
        other => panic!("expected CommandFailed, got: {other}"),
    }
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test test_cook_sh_failure_reports_line_zero`
Expected: PASS (line-0 is already parsed correctly by the existing sentinel parser)

- [ ] **Step 7: Update CLI to handle line-0 errors**

In `src/cli/mod.rs`, find the `CommandFailed` formatting (line 160-166) and add a line-0 check:

```rust
crate::runtime::RuntimeError::CommandFailed {
    command,
    line,
    code,
} => {
    if line == 0 {
        CookError::CommandFailed(format!(
            "command failed (exit {code}): {command}"
        ))
    } else {
        CookError::CommandFailed(format!(
            "Cookfile:{line}: command failed (exit {code}): {command}"
        ))
    }
}
```

- [ ] **Step 8: Run all tests**

Run: `cargo test`
Expected: All tests pass (97 existing + 2 new = 99)

- [ ] **Step 9: Commit**

```bash
git add src/runtime/api.rs src/runtime/mod.rs src/cli/mod.rs
git commit -m "feat: add cook.sh() user-facing shell API with line-0 error handling"
```

---

### Task 3: Build recipe context table and set as global

**Files:**
- Modify: `src/runtime/mod.rs:48-84` (execute_recipe)
- Test: `src/runtime/mod.rs` (new tests)

- [ ] **Step 1: Write the failing test for recipe.name**

Add to `src/runtime/mod.rs` tests:

```rust
#[test]
fn test_recipe_context_name() {
    let dir = TempDir::new().unwrap();
    let rt = make_runtime(dir.path());
    let lua_src = r#"
cook.recipe("mybuild", {}, function()
    assert(recipe.name == "mybuild", "expected 'mybuild', got: " .. tostring(recipe.name))
end)
"#;
    let result = rt.execute_recipe(lua_src, "mybuild");
    assert!(result.is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_recipe_context_name`
Expected: FAIL — `recipe` global doesn't exist yet.

- [ ] **Step 3: Implement recipe context in execute_recipe**

In `src/runtime/mod.rs`, modify `execute_recipe` to build and set the `recipe` global before calling the function. After finding the recipe in the registry (line 57-60), before `let func` (line 62):

```rust
// Build recipe context table
let recipe_table = lua.create_table()?;
recipe_table.set("name", recipe.name.as_str())?;

// Resolve ingredient globs relative to working_dir
let ingredients_table = lua.create_table()?;
for (i, pattern) in recipe.metadata.ingredients.iter().enumerate() {
    let full_pattern = self.working_dir.join(pattern).to_string_lossy().to_string();
    let prefix = self.working_dir.to_string_lossy().to_string();
    let files_table = lua.create_table()?;
    let mut file_idx = 1;
    if let Ok(paths) = glob::glob(&full_pattern) {
        for entry in paths.filter_map(|p| p.ok()) {
            let path_str = entry.to_string_lossy().to_string();
            // Strip working_dir prefix to get Cookfile-relative path
            let relative = path_str
                .strip_prefix(&prefix)
                .unwrap_or(&path_str)
                .trim_start_matches('/')
                .to_string();
            files_table.set(file_idx, relative)?;
            file_idx += 1;
        }
    }
    ingredients_table.set(i + 1, files_table)?;
}
recipe_table.set("ingredients", ingredients_table)?;

// Set serves
match &recipe.metadata.serves {
    Some(api::ServedValue::Single(s)) => {
        recipe_table.set("serves", s.as_str())?;
    }
    Some(api::ServedValue::Multiple(items)) => {
        let serves_table = lua.create_table()?;
        for (i, s) in items.iter().enumerate() {
            serves_table.set(i + 1, s.as_str())?;
        }
        recipe_table.set("serves", serves_table)?;
    }
    None => {} // recipe.serves stays nil
}

lua.globals().set("recipe", recipe_table)?;
```

Note: add `use glob;` is not needed — glob is already a dependency and we use the full path `glob::glob`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test test_recipe_context_name`
Expected: PASS

- [ ] **Step 5: Write test for recipe.ingredients resolution**

Add to `src/runtime/mod.rs` tests:

```rust
#[test]
fn test_recipe_context_ingredients() {
    let dir = TempDir::new().unwrap();
    // Create test files
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.c"), "").unwrap();
    fs::write(dir.path().join("src/b.c"), "").unwrap();
    fs::write(dir.path().join("src/c.h"), "").unwrap();

    let rt = make_runtime(dir.path());
    let lua_src = r#"
cook.recipe("check", {ingredients = {"src/*.c", "src/*.h"}}, function()
    assert(#recipe.ingredients == 2, "expected 2 ingredient groups, got " .. #recipe.ingredients)
    assert(#recipe.ingredients[1] == 2, "expected 2 .c files, got " .. #recipe.ingredients[1])
    assert(#recipe.ingredients[2] == 1, "expected 1 .h file, got " .. #recipe.ingredients[2])
    -- Check paths are relative
    for _, f in ipairs(recipe.ingredients[1]) do
        assert(f:match("^src/"), "expected relative path starting with src/, got: " .. f)
    end
end)
"#;
    let result = rt.execute_recipe(lua_src, "check");
    assert!(result.is_ok(), "failed: {:?}", result.unwrap_err());
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test test_recipe_context_ingredients`
Expected: PASS

- [ ] **Step 7: Write test for recipe.serves**

Add to `src/runtime/mod.rs` tests:

```rust
#[test]
fn test_recipe_context_serves_single() {
    let dir = TempDir::new().unwrap();
    let rt = make_runtime(dir.path());
    let lua_src = r#"
cook.recipe("check", {serves = "bin/app"}, function()
    assert(recipe.serves == "bin/app", "expected 'bin/app', got: " .. tostring(recipe.serves))
end)
"#;
    let result = rt.execute_recipe(lua_src, "check");
    assert!(result.is_ok());
}

#[test]
fn test_recipe_context_serves_multiple() {
    let dir = TempDir::new().unwrap();
    let rt = make_runtime(dir.path());
    let lua_src = r#"
cook.recipe("check", {serves = {"bin/app", "bin/helper"}}, function()
    assert(type(recipe.serves) == "table", "expected table, got: " .. type(recipe.serves))
    assert(#recipe.serves == 2, "expected 2, got: " .. #recipe.serves)
    assert(recipe.serves[1] == "bin/app", "bad serves[1]")
    assert(recipe.serves[2] == "bin/helper", "bad serves[2]")
end)
"#;
    let result = rt.execute_recipe(lua_src, "check");
    assert!(result.is_ok());
}

#[test]
fn test_recipe_context_no_metadata() {
    let dir = TempDir::new().unwrap();
    let rt = make_runtime(dir.path());
    let lua_src = r#"
cook.recipe("check", {}, function()
    assert(recipe.name == "check", "bad name")
    assert(#recipe.ingredients == 0, "expected empty ingredients")
    assert(recipe.serves == nil, "expected nil serves")
end)
"#;
    let result = rt.execute_recipe(lua_src, "check");
    assert!(result.is_ok());
}
```

- [ ] **Step 8: Run all new tests**

Run: `cargo test test_recipe_context`
Expected: All 4 recipe context tests pass.

- [ ] **Step 9: Run full test suite**

Run: `cargo test`
Expected: All tests pass (99 existing + 4 new = 103)

- [ ] **Step 10: Commit**

```bash
git add src/runtime/mod.rs
git commit -m "feat: build recipe context table with resolved ingredients and serves"
```

---

### Task 4: Update example Cookfile and verify end-to-end

**Files:**
- Modify: `examples/Cookfile`
- Test: manual + `cargo test`

- [ ] **Step 1: Update examples/Cookfile to use recipe.ingredients and cook.sh**

```
# Cookfile for a C math library project

recipe "setup"
    mkdir -p build/obj bin
end

recipe "lib"
    ingredients = {"lib/*.c", "include/*.h"}
    serves = "build/libmath.a"
    requires = {"setup"}

    >{
        local objects = {}
        for _, src in ipairs(recipe.ingredients[1]) do
            local name = src:match("([^/]+)%.c$")
            local obj = "build/obj/" .. name .. ".o"
            print("  compiling " .. src)
            cook.sh("gcc -c " .. src .. " -Iinclude -Wall -Wextra -O2 -o " .. obj)
            table.insert(objects, obj)
        end
        cook.sh("ar rcs " .. recipe.serves .. " " .. table.concat(objects, " "))
        print("  archived " .. tostring(fs.size(recipe.serves)) .. " bytes")
    }
end

recipe "build"
    ingredients = {"src/*.c"}
    serves = "bin/app"
    requires = {"lib"}

    gcc src/main.c -Iinclude -Lbuild -lmath -lm -Wall -Wextra -O2 -o bin/app
    >{
        local size = fs.size(recipe.serves)
        if size > 1024 * 1024 then
            print("  warning: binary is over 1MB (" .. size .. " bytes)")
        else
            print("  binary: " .. size .. " bytes")
        end
    }
end

recipe "test"
    requires = {"lib"}

    >{
        local tests = fs.glob("tests/test_*.c")
        for _, src in ipairs(tests) do
            local name = src:match("([^/]+)%.c$")
            local bin = "build/" .. name
            print("  compiling " .. src)
            cook.sh("gcc " .. src .. " -Iinclude -Lbuild -lmath -lm -Wall -Wextra -o " .. bin)
            print("  running " .. name)
            cook.sh("./" .. bin)
        end
    }
end

recipe "run"
    requires = {"build"}
    ./bin/app
end

recipe "clean"
    rm -rf build bin
end
```

- [ ] **Step 2: Run end-to-end test from examples directory**

```bash
cd examples
../target/debug/cook clean
../target/debug/cook test
../target/debug/cook run
../target/debug/cook menu
```

Expected:
- `cook clean` — removes build/bin dirs
- `cook test` — compiles lib via recipe.ingredients[1], runs test binaries, all pass
- `cook run` — builds and runs the demo app with mathlib output
- `cook menu` — lists all recipes with metadata

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: All 103 tests pass.

- [ ] **Step 4: Commit**

```bash
git add examples/Cookfile
git commit -m "feat: update example Cookfile to use recipe.ingredients and cook.sh"
```
