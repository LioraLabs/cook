# Dogfood Cook: Build Lua 5.4 from Source

**Date:** 2026-04-04
**Status:** Approved

## Goal

Prove Cook can build a real C project end-to-end by replacing Lua 5.4.7's Makefile with a Cookfile. This is the first dogfooding milestone — a forcing function that reveals which features actually matter.

## Why Lua

- Cook runs on Lua internally — building Lua with Cook is a compelling story
- Small (~35 C files), no external dependencies, fast builds
- Exercises real features: glob expansion, static library archiving, executable linking, platform-specific flags
- Only requires M2-M3 level features from the roadmap

## Feature: Ingredient Exclusion (`!"pattern"`)

Building Lua requires compiling all `src/*.c` except `lua.c` and `luac.c` (which each contain `main()`). Cook has no exclusion support today. This is the one feature we need to add.

### Parser (cook-lang)

Support `!` prefix before quoted strings in `ingredients` lines:

```
ingredients "src/*.c" !"src/lua.c" !"src/luac.c"
```

The `!` must appear immediately before the opening quote. Strings without `!` are includes; strings with `!` are excludes.

### AST

Add `excludes: Vec<String>` to `Recipe`:

```rust
pub struct Recipe {
    pub name: String,
    pub deps: Vec<String>,
    pub ingredients: Vec<String>,
    pub excludes: Vec<String>,  // NEW
    pub steps: Vec<Step>,
}
```

### Codegen (cook-luagen)

Emit excludes in the Lua recipe metadata table:

```lua
cook.recipe("lib", {ingredients={"src/*.c"}, excludes={"src/lua.c","src/luac.c"}}, function()
    ...
end)
```

### Runtime (cook-register)

In glob resolution (`setup_recipe_context` or equivalent):

1. Expand all include patterns via `glob::glob()`
2. Expand all exclude patterns via `glob::glob()`
3. Subtract exclude matches from include matches
4. Store the filtered result as the resolved ingredients

### Watcher (cook-cli)

Exclude patterns do not feed the file watcher — only include globs trigger rebuilds.

## Cookfile Design

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

## What This Exercises

| Cook feature | How Lua uses it |
|---|---|
| Variables | CC, AR, CFLAGS |
| Recipe dependencies | lib → lua/luac → build |
| Ingredient globs | `src/*.c` |
| **Ingredient exclusion** | `!"src/lua.c"` (NEW) |
| 1-to-1 compilation | `.c` → `.o` via `{stem}` |
| Many-to-one archiving | `.o` files → `.a` |
| Executable linking | `.c` + `.a` → binary |
| Test steps | Run the built lua binary |

## Success Criteria

1. `cook build` in `examples/lua-build/` produces working `build/bin/lua` and `build/bin/luac` binaries
2. `cook test` runs the lua binary and verifies output
3. Incremental rebuild works — changing one `.c` file only recompiles that file
4. `cook clean && cook build` works from scratch

## Out of Scope

- cpp module (raw DSL is sufficient)
- Config blocks (debug/release — save for later)
- Cross-platform support (Linux first, macOS follow-up)
- Header dependency tracking (M4 — separate milestone)
