# Recipe Context & cook.sh Design Spec

## Overview

Two improvements to Cook's Lua runtime API:

1. **`recipe.*` context** — expose resolved metadata (ingredients, serves, name) as Lua values inside recipe bodies
2. **`cook.sh(cmd)`** — user-facing shell execution without the internal line number parameter

## Problem

### Ingredients not accessible in Lua

Recipe metadata (ingredients, serves) is passed to `cook.recipe()` at registration time but the runtime discards it (`_meta`). Users who want to iterate over ingredient files must re-derive them with `fs.glob()`, duplicating what the metadata already declares:

```lua
-- today: redundant glob
local sources = fs.glob("lib/*.c")
```

### cook.exec leaks internal API

`cook.exec(cmd, line)` requires a Cookfile line number — an internal detail used for error reporting. When users call it from Lua blocks, they pass `0` as a meaningless placeholder:

```lua
cook.exec("gcc -c " .. src, 0)
```

## Design

### 1. recipe context table

This works in two phases matching Cook's existing architecture:

**Phase 1 — Registration (`cook.recipe()` in api.rs):** When `cook.recipe(name, metadata, fn)` is called during Lua source loading, the runtime stores the raw metadata alongside the function in `RegisteredRecipe`. Today `RegisteredRecipe` has `name` and `function`; it gains a `metadata` field to hold the ingredient patterns and serves value.

**Phase 2 — Execution (`execute_recipe()` in mod.rs):** Before calling the recipe function, the runtime:

1. Reads the stored metadata from `RegisteredRecipe`
2. Resolves each ingredient glob pattern into a list of matching file paths
3. Builds a `recipe` table with the following structure:
   - `recipe.name` — string, the recipe name
   - `recipe.ingredients` — table indexed by declaration order, where each entry is a list of resolved file paths
   - `recipe.serves` — string or table matching the metadata declaration, or `nil` if not set
4. Sets `recipe` as a Lua global before invoking the recipe function

Since Cook creates a fresh Lua VM per `execute_recipe` call, global scoping is safe. If the runtime is ever changed to reuse a VM across recipe calls, the `recipe` global must be overwritten before each invocation.

`list_recipes` does not set the `recipe` global, since it only runs the registration phase and never invokes recipe functions.

#### Example

Given this Cookfile:

```
recipe "lib"
    ingredients = {"lib/*.c", "include/*.h"}
    serves = "build/libmath.a"
    requires = {"setup"}

    >{
        local objects = {}
        for _, src in ipairs(recipe.ingredients[1]) do
            local name = src:match("([^/]+)%.c$")
            local obj = "build/obj/" .. name .. ".o"
            cook.sh("gcc -c " .. src .. " -Iinclude -o " .. obj)
            table.insert(objects, obj)
        end
        cook.sh("ar rcs " .. recipe.serves .. " " .. table.concat(objects, " "))
    }
end
```

The `recipe` table at runtime contains:

```lua
recipe = {
    name = "lib",
    ingredients = {
        [1] = {"lib/matrix.c", "lib/vec3.c"},        -- from "lib/*.c"
        [2] = {"include/matrix.h", "include/vec3.h"}, -- from "include/*.h"
    },
    serves = "build/libmath.a",
}
```

#### Glob resolution

- Ingredient glob patterns are resolved relative to `working_dir` (the directory containing the Cookfile). The runtime prepends `working_dir` to each pattern before calling `glob::glob()`, then strips the `working_dir` prefix from results so that resolved paths are Cookfile-relative (e.g., `"lib/vec3.c"` not `"/home/user/project/lib/vec3.c"`). This differs from `fs.glob()`, which resolves relative to the process working directory; a future improvement may align `fs.glob()` to also use `working_dir`.
- Resolution happens once per recipe execution, before the recipe function is called
- Glob resolution is always performed regardless of whether the recipe body references `recipe.ingredients` (the cost is negligible compared to the build steps that follow)
- If a glob pattern matches no files, the corresponding entry in `recipe.ingredients` is an empty table. No warning or error is raised.
- If no `ingredients` are declared, `recipe.ingredients` is an empty table `{}`

#### serves type

- When `serves` is a single output, `recipe.serves` is a string (e.g., `"build/libmath.a"`)
- When `serves` declares multiple outputs, `recipe.serves` is a table (e.g., `{"bin/app", "bin/helper"}`)
- When `serves` is not declared, `recipe.serves` is `nil`

#### Mutability

The `recipe` table is a plain Lua table. Mutations have no effect on Cook's behavior — the table is rebuilt fresh for each recipe execution.

### 2. cook.sh(cmd)

A user-facing shell execution function. Identical behavior to `cook.exec` but without the line number parameter.

```lua
cook.sh("gcc -c " .. src .. " -Iinclude -o " .. obj)
local output = cook.sh("git rev-parse HEAD")
```

#### Behavior

- Runs command via `/bin/sh -c`
- Inherits working directory and environment variables (same as `cook.exec`)
- Prints stdout to console
- Returns stdout as a string
- On non-zero exit: raises a Lua error with the `COOK_CMD_FAILED` sentinel
- For the line number in error reporting: always passes `0` in the `COOK_CMD_FAILED` sentinel. Unlike `cook.exec` (which reports Cookfile line numbers from codegen), `cook.sh` is called from generated Lua where line numbers have no meaningful correspondence to the original Cookfile. When `execute_recipe` encounters a line-0 failure, it renders the error as `"command failed (exit {code}): {command}"` without the `Cookfile:{line}:` prefix.

#### Relationship to cook.exec

- `cook.exec(cmd, line)` remains as the internal API that codegen emits for bare shell lines in the Cookfile DSL. It is not removed or hidden.
- `cook.sh(cmd)` is the documented, user-facing API for shell execution from Lua blocks.

## Files affected

- `src/runtime/api.rs` — store metadata in `RegisteredRecipe`, add `cook.sh` function
- `src/runtime/mod.rs` — in `execute_recipe`, resolve ingredient globs from stored metadata, build and set `recipe` global before calling recipe function, handle line-0 error display
- `examples/Cookfile` — update to use `recipe.ingredients[N]` and `cook.sh()`
- Tests — new unit tests for both features

## What this does NOT change

- Parser, lexer, AST — no syntax changes
- Codegen — still emits `cook.exec(cmd, line)` for bare shell lines, still passes metadata table
- Analyzer/dependency resolution — unchanged
- Watcher — unchanged
- CLI — unchanged
