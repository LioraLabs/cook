# Cook/Plate Syntax Design Spec

## Overview

A complete overhaul of the Cookfile syntax. The assignment-based metadata (`ingredients = {…}`, `serves = "…"`, `requires = {…}`) is replaced with a declarative keyword-based system built around the cooking metaphor: `ingredients`, `cook`, `plate`. Dependencies move to the recipe header with `:` syntax. The `using` keyword enables declarative file transforms with template placeholders, accepting both shell strings and Lua blocks.

## Motivation

The current syntax requires users to write repetitive Lua loops for common file transforms (compile each `.c` to `.o`, then archive). The new syntax makes 1-to-1 transforms and many-to-one aggregations declarative one-liners, while preserving the escape hatch to shell and Lua for anything custom.

## The New Syntax

### Keywords

| Keyword | Purpose |
|---|---|
| `recipe name: dep1 dep2` | Recipe declaration + dependencies |
| `ingredients "glob1" "glob2"` | Input file globs (watched for changes) |
| `cook "output-pattern" using "cmd"` | File transform (1-to-1 or many-to-one) |
| `cook "output-path"` | Output declaration only (no transform) |
| `plate "cmd"` | Run command per cooked output |

Shell commands and Lua blocks (`>`, `>{…}`) remain valid as recipe steps and execute in their textual position relative to declarative keywords.

### Removed

- `serves` keyword (replaced by `cook`)
- `requires` keyword (replaced by `: dep` on recipe header)
- `= {…}` assignment syntax for metadata
- `--plate` CLI flag (renamed to `--emit-lua`)

### `cook` Modes

The `cook` keyword has three modes, determined by usage:

**1-to-1 transform** — `{in}` in the `using` clause. Runs once per input file:
```cookfile
cook "build/obj/{stem}.o" using "gcc -c {in} -Iinclude -O2 -o {out}"
```

**Many-to-one aggregation** — `{all}` in the `using` clause. Runs once, `{all}` expands to all previous cook outputs (or original ingredients if no previous cook step):
```cookfile
cook "build/libmath.a" using "ar rcs {out} {all}"
```

**Output declaration** — no `using` clause. Just declares what the recipe produces (for dependency resolution). Build logic handled by shell/Lua steps:
```cookfile
cook "bin/app"
```

`{in}` and `{all}` are mutually exclusive in a `using` clause. The presence of one or the other determines the mode.

### Template Placeholders

Available in `using` shell strings and `plate` commands:

| Placeholder | Context | Expands to |
|---|---|---|
| `{in}` | `cook using` (shell) | Input file path from first ingredient group |
| `{out}` | `cook using`, `plate` | Output file path |
| `{stem}` | `cook using` output pattern + command | `path.stem()` of `{in}` |
| `{name}` | `cook using` output pattern + command | `path.name()` of `{in}` |
| `{ext}` | `cook using` output pattern + command | `path.ext()` of `{in}` |
| `{dir}` | `cook using` output pattern + command | `path.dir()` of `{in}` |
| `{all}` | `cook using` (shell) | All previous cook outputs, space-joined (or original ingredients if no previous cook) |

### Ingredient Group Access

- **Shell `using`**: `{in}` always refers to the first ingredient group. Non-first groups are watch-only (trigger rebuilds but aren't iterated).
- **Lua block `using`**: `input` = current file from first group. `input_1`, `input_2`, etc. = full arrays for each ingredient group for custom iteration.

### Lua Blocks in `using`

`using` accepts Lua blocks with `>{…}`. Local variables are injected:
- `input` — current input file path (from first ingredient group)
- `output` — expanded output path for this input
- `input_1`, `input_2`, etc. — full arrays for each ingredient group

(`in`/`out` avoided because `in` is a Lua reserved word.)

```cookfile
cook "build/obj/{stem}.o" using >{
    local cc = cook.env.CC
    cook.sh(cc .. " -c " .. input .. " -Iinclude -O2 -o " .. output)
}
```

### Parsing `>{` in `using` Clauses

Currently the lexer recognizes `>{` only at the start of a trimmed line. With `cook "..." using >{`, it appears mid-line. When the parser encounters a `cook` or `plate` line ending with `using >{`, it switches to Lua block collection mode using the same brace-depth counting as current `>{` blocks. The Lua block is stored in the AST as part of the `UsingClause`.

## Execution Order Within a Recipe

1. Resolve `ingredients` globs
2. Execute steps in textual order:
   - `cook` steps execute their transforms (1-to-1 loop or many-to-one)
   - `plate` steps iterate over outputs from the last `cook` step
   - Shell/Lua steps execute inline at their declared position
3. `cook` steps chain: each step's inputs come from `ingredients` (first step) or previous `cook` outputs (subsequent steps)

Shell and Lua steps are **not deferred** — they run at their textual position. This means:

```cookfile
recipe lib: setup
    ingredients "lib/*.c"
    cook "build/obj/{stem}.o" using "gcc -c {in} -o {out}"
    > print("compiled " .. #recipe.ingredients[1] .. " files")
    cook "build/libmath.a" using "ar rcs {out} {all}"
end
```

The `print` runs between the two `cook` steps.

## Complete Examples

### Task runner (no metadata)

```cookfile
recipe setup
    mkdir -p build/obj bin
end

recipe clean
    rm -rf build bin
end
```

### 1-to-1 transform + many-to-one aggregation

```cookfile
recipe lib: setup
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" using "gcc -c {in} -Iinclude -O2 -o {out}"
    cook "build/libmath.a" using "ar rcs {out} {all}"
end
```

### 1-to-1 transform + plate for post-processing

```cookfile
recipe test: lib
    ingredients "tests/test_*.c"
    cook "build/{stem}" using "cc {in} -Iinclude -Lbuild -lmath -lm -o {out}"
    plate "./{out}"
end
```

### Simple build with output declaration

```cookfile
recipe build: lib
    ingredients "src/*.c"
    cook "bin/app"
    gcc src/main.c -Iinclude -Lbuild -lmath -lm -O2 -o bin/app
end
```

### Lua block in using

```cookfile
recipe lib: setup
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" using >{
        local cc = cook.env.CC
        cook.sh(cc .. " -c " .. input .. " -Iinclude -O2 -o " .. output)
    }
    cook "build/libmath.a" using "ar rcs {out} {all}"
end
```

### Asset pipeline (chained cook steps)

```cookfile
recipe assets: setup
    ingredients "raw/*.png"
    cook "tmp/{stem}.resized.png" using "magick {in} -resize 512x512 {out}"
    cook "tmp/{stem}.optimized.png" using "optipng {in} -out {out}"
    cook "dist/assets.tar" using "tar cf {out} {all}"
end
```

Note: In chained `cook` steps, `{stem}` reflects the actual input filename. So the second step receives `tmp/foo.resized.png` and `{stem}` = `foo.resized`. This is expected — users should be aware that stems compound through the pipeline.

### .env integration

```cookfile
recipe lib: setup
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" using >{
        local cc = cook.env.CC
        local cflags = cook.env.CFLAGS
        cook.sh(cc .. " " .. cflags .. " -Iinclude -c " .. input .. " -o " .. output)
    }
    cook "build/libmath.a" using "ar rcs {out} {all}"
end
```

## What Changes

### Parser (`src/parser/`)

- **Lexer** (`src/parser/lexer.rs`): Recognize `cook`, `plate`, `using`, `ingredients` as keyword-initiated lines. Parse `: "dep1" "dep2"` on recipe headers. Parse space-separated quoted strings after keywords. Handle `using >{` mid-line by switching to Lua block collection mode.
- **Parser** (`src/parser/mod.rs`): Updated to parse new keyword-based metadata lines and build new AST nodes.
- **AST** (`src/parser/ast.rs`): Replace `RecipeMetadata` with new structures:
  - `ingredients: Vec<String>` — glob patterns (on `Recipe`)
  - `deps: Vec<String>` — dependencies (on `Recipe`)
  - `CookStep { output_pattern: String, using_clause: Option<UsingClause> }` — file transform or output declaration
  - `PlateStep { command: String }` — per-output command
  - `UsingClause` enum: `Shell(String)` or `LuaBlock(String)`
  - `CookStep` and `PlateStep` become variants in the existing `Step` enum
  - Remove: `RecipeMetadata`, `Serves` enum

### Codegen (`src/codegen/`)

- `src/codegen/mod.rs`: Generate Lua for `cook` steps:
  - 1-to-1 mode (`{in}`): emit loop over inputs, expand placeholders per iteration
  - Many-to-one mode (`{all}`): emit single call with all previous outputs joined
  - Declaration mode (no `using`): emit nothing (metadata only)
- Generate Lua for `plate` steps: loop over last cook outputs, expand `{out}`
- For Lua block `using` clauses: inject `local input = ...` and `local output = ...` (and `input_1`, `input_2` arrays) before user code
- Use underscore-prefixed internal variables (`_cook_inputs`, `_cook_outputs`) to avoid shadowing user Lua code

### Runtime (`src/runtime/`)

- `src/runtime/api.rs`: `RegisteredMetadata` updated — remove `serves`, add `cook_outputs` for tracking. Remove `ServedValue` enum.
- `src/runtime/mod.rs`: Recipe context table updated. `cook` step outputs tracked for chaining and `{all}` expansion. `recipe.ingredients` still available.

### Analyzer (`src/analyzer/`)

- `src/analyzer/mod.rs`: `build_recipe_info` reads dependencies from `recipe.deps` instead of `recipe.metadata.requires`. Output paths extracted from `CookStep` entries (those without `using` or the last `cook` step's output) for implicit dependency resolution.
- `src/analyzer/graph.rs`: No changes needed — already consumes `requires: Vec<String>`.

### CLI (`src/cli/mod.rs`)

- `--plate` flag renamed to `--emit-lua`
- `cmd_menu` updated: references to `recipe.metadata.serves`, `recipe.metadata.ingredients`, `recipe.metadata.requires` replaced with new AST fields
- `cmd_init` starter Cookfile template updated to new syntax

### Watcher (`src/watcher/mod.rs`)

- `collect_globs_for_recipes` updated: reads `recipe.ingredients` from new AST structure instead of `recipe.metadata.ingredients`

### Tests

- All parser, codegen, runtime, analyzer, and integration tests rewritten for new syntax

### Examples + README

- `examples/Cookfile` rewritten with new syntax
- `README.md` updated with new examples

## Edge Cases

- Recipe with no metadata (task runner): still works, just shell/Lua steps
- `cook` with no previous `cook` step: inputs come from `ingredients`
- `cook` with `{all}` but no previous `cook` steps: `{all}` expands to original ingredients (first group, space-joined)
- `cook` without `using`: just declares the output path for dependency resolution
- Multiple `ingredients` lines: disallowed — use one line with multiple globs
- `plate` with no `cook` steps: error, nothing to plate
- Empty ingredient glob (no matches): `cook` step produces no outputs, downstream steps get empty inputs
- Chained `cook` steps: `{stem}` reflects the actual input filename (stems compound through pipeline)
- Non-first ingredient groups: watch-only unless explicitly accessed via `input_1`, `input_2` in Lua blocks
- Glob ordering: follows Rust `glob` crate ordering (alphabetical within directory)
