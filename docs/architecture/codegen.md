# Codegen: AST-to-Lua Transpilation

## Overview

The codegen stage transforms the `Cookfile` AST into a Lua source string that the runtime can load and execute. It lives entirely in `src/codegen/mod.rs` (864 lines). The public entry point is `generate(cookfile: &Cookfile) -> String` (line 14).

The output is a sequence of `cook.recipe(...)` calls — one per recipe in the Cookfile. Each call wraps a Lua function body containing the translated steps. The runtime loads this string into a Lua VM and calls each registered recipe function, first in capture mode (to discover what work exists) and again implicitly through the scheduler (to execute it).

---

## Recipe Wrapping

`generate()` (line 14) iterates over `cookfile.recipes` and emits one recipe block per entry. The shape is always:

```lua
cook.recipe("name", {ingredients = {"glob1", "glob2"}, requires = {"dep1"}}, function()
    -- steps
end)
```

The metadata table (second argument) is produced by `generate_metadata(recipe)` (line 82). Its fields:

- `ingredients` — the list of glob patterns from the recipe's `ingredients` line, as a Lua array of quoted strings. Omitted when the list is empty.
- `requires` — the list of explicit dependency recipe names from the `: "dep1"` header syntax. Omitted when the list is empty.

When both lists are empty, `generate_metadata` returns `{}` (line 101).

Examples:

```lua
-- No deps or ingredients:
cook.recipe("clean", {}, function() ... end)

-- With ingredients only:
cook.recipe("compile", {ingredients = {"src/*.c"}}, function() ... end)

-- With both:
cook.recipe("build", {ingredients = {"src/*.c"}, requires = {"deps"}}, function() ... end)
```

After the closing `end)` of each recipe, `generate()` emits a blank line (line 76).

---

## Step Translation Rules

Inside the recipe function body, each `Step` variant from the AST maps to a specific Lua form. All emitted lines are indented with four spaces.

### `Shell` — bare shell command

A `Step::Shell` with `interactive: false` becomes:

```lua
cook.exec([[command text]], <line>)
```

A `Step::Shell` with `interactive: true` (source line started with `@`) becomes:

```lua
cook.interactive([[command text]], <line>)
```

The command string is wrapped with `wrap_lua_string()` (line 375), which produces `[[...]]` for most commands or `[=[...]=]` if the command contains `]]` (see [String Escaping](#string-escaping)).

The `<line>` argument is the 1-indexed source line number from the AST. It is threaded through to runtime error messages so that command failures report the Cookfile line.

### `Lua` — single-line Lua expression

A `Step::Lua { code, .. }` is emitted verbatim as a single indented line:

```lua
    <code>
```

No wrapping, no API call. The code executes directly in the recipe function scope.

### `LuaBlock` — multi-line Lua block

A `Step::LuaBlock { code, .. }` is emitted line-by-line, each indented:

```lua
    <line 1>
    <line 2>
    ...
```

The `code` field contains the collected lines joined with `\n`, so `code.lines()` restores the original structure.

### `Taste` — skipped

`Step::Taste` emits no Lua output (line 46–47). The `cook.taste` API is registered in the runtime for legacy reasons but the DAG codegen path does not emit it.

### `Cook` — file transformation step

Handled by `generate_cook_step()` (line 121). Three possible modes depending on the `using_clause`; see [Cook Step Modes](#cook-step-modes) below. Always wrapped in `cook.begin_step()` / `cook.end_step()` markers.

### `Plate` — run step for each output

Handled by `generate_plate_step()` (line 248). Like `Cook`, always wrapped in `cook.begin_step()` / `cook.end_step()` markers. See [Plate Steps](#plate-steps) below.

---

## Cook Step Modes

`generate_cook_step()` (line 121) determines the generation mode by calling `cook_step_mode()` (line 107), which inspects the `using_clause` field of the `CookStep`:

| `using_clause` | Outputs | Mode |
|---|---|---|
| `None` | any | `DeclarationOnly` |
| `Some(UsingClause::ShellBlock(_))` | any | `BlockStep` |
| `Some(UsingClause::LuaBlock(_))` | > 1 | `BlockStep` |
| `Some(UsingClause::LuaBlock(_))` | 1 | `OneToOne` |
| `Some(UsingClause::Shell(cmd))` where `cmd` contains `{in}` | 1 | `OneToOne` |
| `Some(UsingClause::Shell(cmd))` where `cmd` does not contain `{in}` | 1 | `ManyToOne` |

The single-line `using "cmd"` form is single-output only; the parser rejects it when more than one quoted output appears on the `cook` line.

The input source for a cook step is either `recipe.ingredients[1]` (first cook step in the recipe, with no prior cook step) or `_cook_outputs_N` from the preceding cook step (line 130–134). Each cook step increments a `cook_index` counter (line 25–26), and `prev_cook_index` tracks the last value so the chaining works correctly.

### DeclarationOnly (`using_clause: None`)

No build command — just declares the output path. Typically used when the build command is handled elsewhere (e.g., by the runtime itself or a prior step).

**Example Cookfile:**
```
cook "bin/app"
```

**Generated Lua:**
```lua
    cook.begin_step()
    local _cook_outputs_1 = {"bin/app"}
    cook.end_step()
```

The output list is a single-element array containing the literal output pattern string. No `cook.layer()` call is emitted.

### OneToOne (`{in}` present in shell, or any LuaBlock)

One output is produced per input file. The codegen emits a `for` loop over the input source, computing path components for each file, then calls `cook.layer()` to register the transformation.

**Example Cookfile:**
```
ingredients "src/*.c"
cook "build/{stem}.o" using "gcc -c {in} -o {out}"
```

**Generated Lua:**
```lua
    cook.begin_step()
    local _cook_outputs_1 = {}
    for _, _cook_in in ipairs(recipe.ingredients[1]) do
        local _cook_stem = path.stem(_cook_in)
        local _cook_name = path.name(_cook_in)
        local _cook_ext = path.ext(_cook_in)
        local _cook_dir = path.dir(_cook_in)
        local _cook_out = "build/" .. _cook_stem .. ".o"
        cook.layer(_cook_in, _cook_out, <hash>, function()
            cook.exec("gcc -c " .. _cook_in .. " -o " .. _cook_out, <line>)
        end)
        table.insert(_cook_outputs_1, _cook_out)
    end
    cook.end_step()
```

The path helper variables (`_cook_stem`, `_cook_name`, `_cook_ext`, `_cook_dir`) are always emitted regardless of whether the output pattern uses them — this keeps the template expansion code simple.

The `<hash>` argument passed to `cook.layer()` is `hash_str(cmd)` computed at codegen time (line 161). The runtime uses this hash for cache invalidation: if the command text changes, the hash changes, and cached outputs are considered stale.

For a **LuaBlock** `using` clause, the codegen emits the raw user Lua source as the `lua_code` field on `cook.add_unit`, together with an `ingredient_groups` list built from `recipe.ingredients`. The worker pool (`cook-luaotp`) later executes that code against a fresh Lua state where `input`, `output`, `inputs`, `outputs`, and `input_N` are pre-populated as globals.

**Example Cookfile:**
```
cook "build/{stem}.o" using >{
    cook.sh("gcc -c " .. input .. " -o " .. output)
}
```

**Generated Lua (OneToOne):**
```lua
    local _cook_outputs_1 = {}
    cook.step_group(function()
        for _, _cook_in in ipairs(recipe.ingredients[1]) do
            local _cook_stem = path.stem(_cook_in)
            local _cook_name = path.name(_cook_in)
            local _cook_ext = path.ext(_cook_in)
            local _cook_dir = path.dir(_cook_in)
            local _cook_out = "build/" .. _cook_stem .. ".o"
            cook.add_unit({inputs = {_cook_in}, output = _cook_out, lua_code = [[
                cook.sh("gcc -c " .. input .. " -o " .. output)
            ]], ingredient_groups = {recipe.ingredients[1]}})
            table.insert(_cook_outputs_1, _cook_out)
        end
    end)
```

### ManyToOne (`{in}` absent in shell command)

A single invocation that consumes all inputs at once. Used for linker-style steps that combine many object files into one archive or binary.

**Example Cookfile:**
```
cook "build/{stem}.o" using "gcc -c {in} -o {out}"
cook "build/lib.a" using "ar rcs {out} {all}"
```

The second `cook` step has no `{in}` in its command, so it becomes `ManyToOne`. Its input source is `_cook_outputs_1` from the prior step.

**Generated Lua (second cook step):**
```lua
    cook.begin_step()
    local _cook_outputs_2 = {}
    local _cook_all = table.concat(_cook_outputs_1, " ")
    local _cook_out = "build/lib.a"
    cook.layer(_cook_outputs_1, _cook_out, <hash>, function()
        cook.exec("ar rcs " .. _cook_out .. " " .. _cook_all, <line>)
    end)
    table.insert(_cook_outputs_2, _cook_out)
    cook.end_step()
```

Note: `cook.layer()` here receives the full input list as its first argument (not a single file). The runtime handles this by treating the list as the dependency set for cache checking. `_cook_all` is a space-joined string used inside the command via `{all}` template expansion.

### BlockStep (multi-output cook steps)

Entered when the `using` clause is a plain-shell block (`using { ... }`) or when a Lua block is paired with two or more declared outputs. All declared outputs are produced by a single invocation, share one cache entry, and appear atomically — either all are materialized or none are.

The codegen emits one `cook.layer()` call with the full output list, and threads the declared outputs back into `_cook_outputs_N` so downstream steps can consume them.

**Example Cookfile (plain-shell block, working form):**
```
cook "out/parser.rs" "out/parser.h" using {
    lalrpop src/grammar.lalrpop --out-dir out
}
```

The generated Lua registers one layer whose `{out}` expands per-output inside the block body (the emitted code iterates the declared outputs and runs the command for each — or, for tools that emit all outputs in a single invocation, the block body runs once and the layer records all declared paths as produced).

**Example Cookfile (Lua block, multi-output):**
```
cook "out/parser.rs" "out/parser.h" using >{
    cook.sh("lalrpop src/grammar.lalrpop --out-dir out")
}
```

The multi-output Lua-block form is emitted as a single `cook.add_unit` call with the user's Lua source passed via the `lua_code` field and any resolved ingredient-group tables passed via `ingredient_groups`. The worker pool (`cook-luaotp`) executes the code against a fresh Lua state where `inputs`, `outputs`, `input` (= `inputs[1]`), `output` (= `outputs[1]`), and `input_N` (= ingredient group N) are pre-populated globals.

### Plate Steps

`generate_plate_step()` (line 248) handles `Step::Plate`. It loops over the output list from the last cook step (or `recipe.ingredients[1]` if there was no prior cook step) and calls `cook.layer()` with `nil` as the output argument, signaling that this is a run-only step with no output file.

**Example Cookfile:**
```
cook "bin/app"
plate "./{out}"
```

**Generated Lua:**
```lua
    cook.begin_step()
    local _cook_outputs_1 = {"bin/app"}
    cook.end_step()
    cook.begin_step()
    for _, _plate_out in ipairs(_cook_outputs_1) do
        cook.layer(_plate_out, nil, <hash>, function()
            cook.exec("./" .. _plate_out, <line>)
        end)
    end
    cook.end_step()
```

The `{out}` placeholder in the plate command expands to `_plate_out` (line 309), not `_cook_out`. This is a separate variable because plate steps do not produce a new output file — the loop variable is the file being run.

---

## `cook.begin_step()` / `cook.end_step()` Markers

Every `Cook` and `Plate` step is bracketed by `cook.begin_step()` and `cook.end_step()` calls (lines 53, 62, 69, 71). These calls are no-ops in execute mode. In capture mode, the runtime uses them to define **step group boundaries**.

When `cook.begin_step()` fires, the runtime opens a new step group. All `cook.layer()` calls between a `begin_step()` / `end_step()` pair are added to that group. When `cook.end_step()` fires, the group is closed and recorded in `step_groups`.

The DAG builder (`src/scheduler/builder.rs`) uses `step_groups` to assign units within the same group to the same parallelism tier: they can all run simultaneously because they share no file dependencies. Units in different groups are serialized — a later group's work cannot start until all units in the earlier group complete.

Shell steps (`cook.exec`, `cook.interactive`) are not wrapped in `begin_step()` / `end_step()` and are therefore always sequential with respect to adjacent steps.

---

## Template Expansion

Shell commands and output patterns in `Cook` and `Plate` steps are not emitted as literal strings. Instead, they are expanded into Lua concatenation expressions so the variable values are resolved at runtime (after ingredient globs are expanded and path components computed).

Expansion is a two-pass process handled by `expand_template_with_env_fallback()` (line 316).

### Pass 1 — Builtin Variables

`expand_template_to_lua()` (line 293) defines the builtin variable map:

| Placeholder | Lua variable |
|---|---|
| `{in}` | `_cook_in` |
| `{out}` | `_cook_out` |
| `{stem}` | `_cook_stem` |
| `{name}` | `_cook_name` |
| `{ext}` | `_cook_ext` |
| `{dir}` | `_cook_dir` |
| `{all}` | `_cook_all` |

`expand_output_pattern()` (line 280) uses a smaller set (`{stem}`, `{name}`, `{ext}`, `{dir}`, `{in}`) because output patterns cannot reference `{out}` or `{all}`. The plate command expander `expand_plate_cmd()` (line 308) only recognises `{out}` → `_plate_out`.

### Pass 2 — Environment Variable Fallback

After builtin placeholders are resolved, any remaining `{VAR}` tokens are treated as environment variable lookups and expanded to `cook.env["VAR"]` (line 345).

```
"{CC} {CFLAGS} -c {in} -o {out}"
→  cook.env["CC"] .. " " .. cook.env["CFLAGS"] .. " -c " .. _cook_in .. " -o " .. _cook_out
```

### How the Expansion Works

`expand_template_with_env_fallback()` (line 316) walks the template string looking for `{`:

1. Any literal text before `{` is pushed as a double-quoted Lua string: `"literal text"`.
2. The content between `{` and the matching `}` is checked against the builtin table. If found, the Lua variable name is pushed directly (no quotes). If not found, `cook.env["VAR"]` is pushed.
3. The final list of parts is joined with ` .. `.

If there is exactly one part, no concatenation is emitted (just the bare variable name or string). If the template is entirely empty, `""` is returned (line 362).

Examples (from the unit tests):

```rust
expand_template_to_lua("echo hello")
// → "\"echo hello\""

expand_template_to_lua("{in}")
// → "_cook_in"

expand_template_to_lua("gcc -c {in} -o {out}")
// → "\"gcc -c \" .. _cook_in .. \" -o \" .. _cook_out"

expand_template_to_lua("ar rcs {out} {all}")
// → "\"ar rcs \" .. _cook_out .. \" \" .. _cook_all"
```

---

## String Escaping

Two helpers handle string encoding for different contexts.

### `escape_lua_string()` (line 369)

Used when emitting content inside double-quoted Lua strings. Applies three replacements in order:

1. `\` → `\\`
2. `"` → `\"`
3. newline → `\n`

This is used for recipe names in `cook.recipe("name", ...)` and for the string literal parts of template expansion output.

### `wrap_lua_string()` (line 375)

Used for shell command arguments to `cook.exec()` and `cook.interactive()`. Instead of double-quoted strings, these use Lua long-string syntax to avoid interference with shell quoting:

- If the command does not contain `]]` → `[[command]]`
- If the command contains `]]` → `[=[command]=]`

This means a command like `echo hello` becomes `[[echo hello]]` and a command like `echo ]]` becomes `[=[echo ]]]=]`. The bracket-level-1 form `[=[...]=]` is always safe because it requires `]=]` to close, which cannot appear in a shell command that only contains `]]`.
