# Build Lua 5.4 with Cook — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add ingredient exclusion (`!"pattern"`) to Cook's DSL and use it to build Lua 5.4.7 from source — the first real-world project built by Cook.

**Architecture:** The `!` exclusion feature threads through four crates: cook-lang (parser + AST), cook-luagen (codegen), cook-register (runtime glob resolution), and cook-cli (watcher). Each crate change is isolated and testable independently. After the feature lands, we download Lua 5.4.7 source and write a Cookfile that builds it.

**Tech Stack:** Rust, Lua 5.4.7 (C source), glob crate

**Spec:** `docs/superpowers/specs/2026-04-04-lua-dogfood-design.md`

---

## Chunk 1: Ingredient Exclusion Feature

### Task 1: Add `excludes` field to AST

**Files:**
- Modify: `cli/crates/cook-lang/src/ast.rs:24-29` (Recipe struct)
- Modify: `cli/crates/cook-lang/src/ast.rs:66-187` (tests)

- [ ] **Step 1: Add `excludes` field to Recipe struct**

In `cli/crates/cook-lang/src/ast.rs`, add `excludes: Vec<String>` to the `Recipe` struct:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub name: String,
    pub deps: Vec<String>,
    pub ingredients: Vec<String>,
    pub excludes: Vec<String>,
    pub steps: Vec<Step>,
    pub line: usize,
}
```

- [ ] **Step 2: Fix all existing Recipe construction sites in ast.rs tests**

Every `Recipe { ... }` literal in the test module needs `excludes: vec![],`. Update these tests:

`test_recipe_construction` (line ~72):
```rust
let recipe = Recipe {
    name: "build".to_string(),
    deps: vec!["setup".to_string()],
    ingredients: vec!["src/*.c".to_string()],
    excludes: vec![],
    steps: vec![
        Step::Cook {
            step: CookStep {
                output_pattern: "build/obj/{stem}.o".to_string(),
                using_clause: Some(UsingClause::Shell(
                    "gcc -c {in} -o {out}".to_string(),
                )),
            },
            line: 4,
        },
    ],
    line: 1,
};
```

`test_recipe_no_metadata` (line ~96):
```rust
let recipe = Recipe {
    name: "clean".to_string(),
    deps: vec![],
    ingredients: vec![],
    excludes: vec![],
    steps: vec![Step::Shell {
        command: "rm -rf build".to_string(),
        line: 2,
        interactive: false,
    }],
    line: 1,
};
```

- [ ] **Step 3: Fix Recipe construction in recipe.rs parser**

In `cli/crates/cook-lang/src/recipe.rs` line ~131, add the `excludes` field to the returned Recipe:

```rust
return Ok((
    Recipe {
        name,
        deps,
        ingredients,
        excludes: vec![],
        steps,
        line: recipe_line,
    },
    pos,
));
```

We initialize it as empty for now — the parser will populate it in Task 2.

- [ ] **Step 4: Fix Recipe construction in luagen tests**

In `cli/crates/cook-luagen/src/tests.rs`, update the `make_recipe` helper (line ~17):

```rust
fn make_recipe(
    name: &str,
    deps: Vec<&str>,
    ingredients: Vec<&str>,
    steps: Vec<Step>,
) -> Recipe {
    Recipe {
        name: name.to_string(),
        deps: deps.into_iter().map(String::from).collect(),
        ingredients: ingredients.into_iter().map(String::from).collect(),
        excludes: vec![],
        steps,
        line: 1,
    }
}
```

- [ ] **Step 5: Compile and run all tests**

Run: `cd /home/alex/dev/cook/cli && cargo test`
Expected: All 312 tests pass. The new `excludes` field is empty everywhere, so behavior is unchanged.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-lang/src/ast.rs cli/crates/cook-lang/src/recipe.rs cli/crates/cook-luagen/src/tests.rs
git commit -m "feat(cook-lang): add excludes field to Recipe AST"
```

---

### Task 2: Parse `!"pattern"` in ingredients line

**Files:**
- Modify: `cli/crates/cook-lang/src/cook_line.rs:19-38` (parse_quoted_strings_parser)
- Modify: `cli/crates/cook-lang/src/recipe.rs:114-277` (parse_recipe)
- Modify: `cli/crates/cook-lang/src/tests.rs` (add new tests)

- [ ] **Step 1: Write failing tests for exclusion parsing**

Add these tests at the end of `cli/crates/cook-lang/src/tests.rs`:

```rust
// ── Ingredient exclusion ──────────────────────────────────────────

#[test]
fn test_ingredients_with_excludes() {
    let source = r#"recipe build
    ingredients "src/*.c" !"src/lua.c" !"src/luac.c"
    echo compiling
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.ingredients, vec!["src/*.c"]);
    assert_eq!(recipe.excludes, vec!["src/lua.c", "src/luac.c"]);
}

#[test]
fn test_ingredients_excludes_only() {
    let source = r#"recipe build
    ingredients !"src/test.c"
    echo compiling
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert!(recipe.ingredients.is_empty());
    assert_eq!(recipe.excludes, vec!["src/test.c"]);
}

#[test]
fn test_ingredients_no_excludes() {
    let source = r#"recipe build
    ingredients "src/*.c" "include/*.h"
    echo compiling
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.ingredients, vec!["src/*.c", "include/*.h"]);
    assert!(recipe.excludes.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /home/alex/dev/cook/cli && cargo test --lib -p cook-lang -- test_ingredients_with_excludes test_ingredients_excludes_only`
Expected: FAIL — `!` prefix is not recognized by the parser, it will error on `!"src/lua.c"`.

- [ ] **Step 3: Create parse_ingredients_with_excludes function**

Replace `parse_quoted_strings_parser` in `cli/crates/cook-lang/src/cook_line.rs` with a new function that handles both includes and excludes. Keep the old function for backwards compatibility (it's used elsewhere), and add the new one:

```rust
/// Parse an ingredients line into (includes, excludes).
/// Includes are bare `"pattern"`, excludes are `!"pattern"`.
pub(crate) fn parse_ingredients_line(text: &str, line: usize) -> Result<(Vec<String>, Vec<String>), ParseError> {
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    let mut remaining = text.trim();
    while !remaining.is_empty() {
        let is_exclude = remaining.starts_with('!');
        if is_exclude {
            remaining = &remaining[1..];
        }
        if !remaining.starts_with('"') {
            return Err(ParseError::Parse {
                line,
                message: format!("expected '\"', found: {}", remaining),
            });
        }
        let rest = &remaining[1..];
        let end = rest.find('"').ok_or(ParseError::Parse {
            line,
            message: "unterminated string".to_string(),
        })?;
        let value = rest[..end].to_string();
        if is_exclude {
            excludes.push(value);
        } else {
            includes.push(value);
        }
        remaining = rest[end + 1..].trim();
    }
    Ok((includes, excludes))
}
```

- [ ] **Step 4: Update parse_recipe to use new function and populate excludes**

In `cli/crates/cook-lang/src/recipe.rs`, change the ingredients parsing block. First, add `excludes` to the local variables at the top of `parse_recipe`:

```rust
let mut pos = start;
let mut ingredients = Vec::new();
let mut excludes = Vec::new();
let mut steps = Vec::new();
```

Then change the ingredients parsing branch (the `strip_keyword(text, "ingredients")` arm around line ~146):

```rust
if let Some(rest) = strip_keyword(text, "ingredients") {
    if !ingredients.is_empty() || !excludes.is_empty() {
        return Err(ParseError::Parse {
            line: tok.line,
            message: "duplicate 'ingredients' line".to_string(),
        });
    }
    let (inc, exc) = parse_ingredients_line(rest, tok.line)?;
    ingredients = inc;
    excludes = exc;
}
```

And update the returned Recipe to include excludes:

```rust
return Ok((
    Recipe {
        name,
        deps,
        ingredients,
        excludes,
        steps,
        line: recipe_line,
    },
    pos,
));
```

Add the import at the top of recipe.rs — change:
```rust
use crate::cook_line::*;
```
This already imports everything, so `parse_ingredients_line` will be available automatically.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd /home/alex/dev/cook/cli && cargo test --lib -p cook-lang`
Expected: All cook-lang tests pass, including the 3 new exclusion tests.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-lang/src/cook_line.rs cli/crates/cook-lang/src/recipe.rs cli/crates/cook-lang/src/tests.rs
git commit -m "feat(cook-lang): parse ingredient exclusion patterns (!\\"pattern\\")"
```

---

### Task 3: Emit excludes in Lua codegen

**Files:**
- Modify: `cli/crates/cook-luagen/src/recipe.rs:103-126` (generate_metadata)
- Modify: `cli/crates/cook-luagen/src/tests.rs` (add tests)

- [ ] **Step 1: Write failing test for excludes in codegen**

Add to `cli/crates/cook-luagen/src/tests.rs`:

```rust
#[test]
fn test_recipe_with_excludes() {
    let cookfile = make_cookfile(vec![Recipe {
        name: "lib".to_string(),
        deps: vec![],
        ingredients: vec!["src/*.c".to_string()],
        excludes: vec!["src/lua.c".to_string(), "src/luac.c".to_string()],
        steps: vec![],
        line: 1,
    }]);
    let output = generate(&cookfile);
    assert!(
        output.contains(r#"excludes = {"src/lua.c", "src/luac.c"}"#),
        "expected excludes in metadata, got:\n{output}"
    );
    assert!(output.contains(r#"ingredients = {"src/*.c"}"#));
}

#[test]
fn test_recipe_without_excludes() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![],
    )]);
    let output = generate(&cookfile);
    assert!(!output.contains("excludes"), "should not emit excludes when empty");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /home/alex/dev/cook/cli && cargo test --lib -p cook-luagen -- test_recipe_with_excludes`
Expected: FAIL — `generate_metadata` doesn't emit excludes yet.

- [ ] **Step 3: Update generate_metadata to emit excludes**

In `cli/crates/cook-luagen/src/recipe.rs`, update the `generate_metadata` function:

```rust
fn generate_metadata(recipe: &Recipe) -> String {
    let mut fields = Vec::new();
    if !recipe.ingredients.is_empty() {
        let items: Vec<String> = recipe
            .ingredients
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect();
        fields.push(format!("ingredients = {{{}}}", items.join(", ")));
    }
    if !recipe.excludes.is_empty() {
        let items: Vec<String> = recipe
            .excludes
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect();
        fields.push(format!("excludes = {{{}}}", items.join(", ")));
    }
    if !recipe.deps.is_empty() {
        let items: Vec<String> = recipe
            .deps
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect();
        fields.push(format!("requires = {{{}}}", items.join(", ")));
    }
    if fields.is_empty() {
        "{}".to_string()
    } else {
        format!("{{{}}}", fields.join(", "))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /home/alex/dev/cook/cli && cargo test --lib -p cook-luagen`
Expected: All cook-luagen tests pass.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-luagen/src/recipe.rs cli/crates/cook-luagen/src/tests.rs
git commit -m "feat(cook-luagen): emit excludes in recipe metadata"
```

---

### Task 4: Filter excludes during runtime glob resolution

**Files:**
- Modify: `cli/crates/cook-register/src/capture.rs:38-68` (register_cook_api_capture, recipe_fn)
- Modify: `cli/crates/cook-register/src/context.rs` (setup_recipe_context)

- [ ] **Step 1: Store excludes in RegisteredMetadata**

In `cli/crates/cook-register/src/capture.rs`, add `excludes` to `RegisteredMetadata`:

```rust
#[derive(Debug)]
pub struct RegisteredMetadata {
    pub ingredients: Vec<String>,
    pub excludes: Vec<String>,
    pub requires: Vec<String>,
}
```

Then update the `recipe_fn` closure (around line ~38) to read excludes from the Lua meta table:

```rust
let recipe_fn =
    lua.create_function(move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
        let key = lua.create_registry_value(func)?;

        let mut ingredients = Vec::new();
        if let Ok(ing_table) = meta.get::<LuaTable>("ingredients") {
            for pair in ing_table.sequence_values::<String>() {
                if let Ok(s) = pair {
                    ingredients.push(s);
                }
            }
        }

        let mut excludes = Vec::new();
        if let Ok(exc_table) = meta.get::<LuaTable>("excludes") {
            for pair in exc_table.sequence_values::<String>() {
                if let Ok(s) = pair {
                    excludes.push(s);
                }
            }
        }

        let mut requires = Vec::new();
        if let Ok(req_table) = meta.get::<LuaTable>("requires") {
            for pair in req_table.sequence_values::<String>() {
                if let Ok(s) = pair {
                    requires.push(s);
                }
            }
        }

        recipes_clone.borrow_mut().push(RegisteredRecipe {
            name,
            function: key,
            metadata: RegisteredMetadata {
                ingredients,
                excludes,
                requires,
            },
        });
        Ok(())
    })?;
```

- [ ] **Step 2: Filter excludes in setup_recipe_context**

In `cli/crates/cook-register/src/context.rs`, update `setup_recipe_context` to resolve exclude patterns and subtract them:

```rust
pub fn setup_recipe_context(
    lua: &Lua,
    recipe: &RegisteredRecipe,
    working_dir: &Path,
) -> Result<(), RegisterError> {
    // Build recipe context table
    let recipe_table = lua.create_table()?;
    recipe_table.set("name", recipe.name.as_str())?;

    // Resolve exclude patterns into a set for fast lookup
    let mut excluded: BTreeSet<String> = BTreeSet::new();
    for pattern in &recipe.metadata.excludes {
        excluded.extend(resolve_glob(working_dir, pattern));
    }

    // Build ingredients table by resolving glob patterns, minus excludes
    let ingredients_table = lua.create_table()?;
    for (i, pattern) in recipe.metadata.ingredients.iter().enumerate() {
        let files = resolve_glob(working_dir, pattern);
        let filtered: BTreeSet<String> = files
            .into_iter()
            .filter(|f| !excluded.contains(f))
            .collect();
        let files_table = lua.create_table()?;
        for (idx, file) in filtered.iter().enumerate() {
            files_table.set(idx + 1, file.as_str())?;
        }
        ingredients_table.set(i + 1, files_table)?;
    }
    recipe_table.set("ingredients", ingredients_table)?;

    lua.globals().set("recipe", recipe_table)?;
    Ok(())
}
```

- [ ] **Step 3: Compile and run all tests**

Run: `cd /home/alex/dev/cook/cli && cargo test`
Expected: All tests pass. No existing tests use excludes, so this is purely additive.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-register/src/capture.rs cli/crates/cook-register/src/context.rs
git commit -m "feat(cook-register): filter excluded ingredients during glob resolution"
```

---

### Task 5: Ensure watcher excludes don't trigger rebuilds

**Files:**
- Modify: `cli/crates/cook-cli/src/watcher.rs:21-32` (collect_globs_for_recipes)

- [ ] **Step 1: Verify watcher only collects include globs**

Read `cli/crates/cook-cli/src/watcher.rs:21-32`. The `collect_globs_for_recipes` function currently collects `recipe.ingredients` — it does NOT collect `recipe.excludes`. Since `excludes` is a new field on the AST, and this function only reads `ingredients`, **no change is needed**. The watcher already ignores excludes by not referencing them.

Verify by reading the code — the function is:
```rust
pub fn collect_globs_for_recipes(
    cookfile: &cook_lang::ast::Cookfile,
    recipe_names: &[String],
) -> Vec<String> {
    let mut globs = Vec::new();
    for recipe in &cookfile.recipes {
        if recipe_names.contains(&recipe.name) {
            globs.extend(recipe.ingredients.clone());
        }
    }
    globs
}
```

This only accesses `recipe.ingredients`, not `recipe.excludes`. No change required.

- [ ] **Step 2: Run full test suite**

Run: `cd /home/alex/dev/cook/cli && cargo test`
Expected: All tests pass.

---

## Chunk 2: Build Lua 5.4 with Cook

### Task 6: Download and set up Lua 5.4.7 source

**Files:**
- Create: `examples/lua-build/Cookfile`
- Create: `examples/lua-build/.gitignore`
- Download: Lua 5.4.7 source tarball

- [ ] **Step 1: Create examples/lua-build directory and download Lua**

```bash
mkdir -p /home/alex/dev/cook/examples/lua-build
cd /home/alex/dev/cook/examples/lua-build
curl -L -o lua-5.4.7.tar.gz https://www.lua.org/ftp/lua-5.4.7.tar.gz
tar xzf lua-5.4.7.tar.gz
rm lua-5.4.7.tar.gz
```

Verify the source is there:
```bash
ls lua-5.4.7/src/*.c | head -5
```
Expected: Should show `lapi.c`, `lauxlib.c`, etc.

- [ ] **Step 2: Create .gitignore for build artifacts**

Create `examples/lua-build/.gitignore`:

```
build/
.cook/
```

- [ ] **Step 3: Write the Cookfile**

Create `examples/lua-build/Cookfile`:

```
CC "gcc"
AR "ar"
CFLAGS "-O2 -Wall -Wextra -DLUA_USE_LINUX"

recipe setup
    mkdir -p build/obj build/bin
end

recipe lib: setup
    ingredients "lua-5.4.7/src/*.c" !"lua-5.4.7/src/lua.c" !"lua-5.4.7/src/luac.c"
    cook "build/obj/{stem}.o" using "{CC} {CFLAGS} -c {in} -o {out}"
    cook "build/liblua.a" using "{AR} rcs {out} {all}"
end

recipe lua: lib
    ingredients "lua-5.4.7/src/lua.c"
    cook "build/bin/lua" using "{CC} {CFLAGS} {in} -Lbuild -llua -lm -ldl -lreadline -o {out}"
end

recipe luac: lib
    ingredients "lua-5.4.7/src/luac.c"
    cook "build/bin/luac" using "{CC} {CFLAGS} {in} -Lbuild -llua -lm -ldl -o {out}"
end

recipe build: lua luac
end

recipe test: lua
    test "build/bin/lua -e 'print(\"hello from lua built by cook\")'"
end

recipe clean
    rm -rf build
end
```

- [ ] **Step 4: Commit the Lua source and Cookfile**

```bash
git add examples/lua-build/
git commit -m "feat: add Lua 5.4.7 example — first real-world Cook project"
```

---

### Task 7: Build Lua with Cook and fix issues

**Files:**
- Potentially any Cook crate if bugs are found

- [ ] **Step 1: Build Cook in release mode**

```bash
cd /home/alex/dev/cook/cli
cargo build --release
```

- [ ] **Step 2: Run `cook build` on the Lua example**

```bash
cd /home/alex/dev/cook/examples/lua-build
../../cli/target/release/cook build
```

Expected: Cook compiles all `.c` files (except `lua.c` and `luac.c`) into `.o` files, archives them into `build/liblua.a`, then links `lua` and `luac` binaries.

If this fails, debug and fix the issue. Common things that might go wrong:
- Glob patterns not resolving correctly (path prefix issues)
- `{all}` placeholder not expanding correctly for `ar` step
- Missing `-I` flag for header includes (Lua headers are in `lua-5.4.7/src/`)
- Link order issues

**Note:** The Cookfile may need `-Ilua-5.4.7/src` added to CFLAGS if compilation fails with missing headers. The Lua source includes headers relative to its own `src/` directory, but since the Cookfile is in the parent, `{in}` paths will be like `lua-5.4.7/src/lapi.c` — the compiler should find sibling headers via the path in the `#include` directives. If not, add `-I lua-5.4.7/src` to CFLAGS.

- [ ] **Step 3: Verify the built binaries work**

```bash
./build/bin/lua -e 'print("hello from lua built by cook")'
./build/bin/luac -v
```

Expected: Lua prints "hello from lua built by cook", luac shows its version.

- [ ] **Step 4: Run `cook test`**

```bash
cd /home/alex/dev/cook/examples/lua-build
../../cli/target/release/cook test
```

Expected: Test step runs the lua binary and reports success.

- [ ] **Step 5: Test incremental rebuild**

```bash
# Touch one file and rebuild
touch lua-5.4.7/src/lapi.c
../../cli/target/release/cook build
```

Expected: Only `lapi.o` and `liblua.a` are rebuilt, not all `.o` files.

- [ ] **Step 6: Test clean rebuild**

```bash
../../cli/target/release/cook clean
../../cli/target/release/cook build
```

Expected: Full rebuild from scratch succeeds.

- [ ] **Step 7: Commit any fixes**

If any Cook code was modified to fix issues:
```bash
git add -A
git commit -m "fix: resolve issues found while building Lua 5.4.7 with Cook"
```
