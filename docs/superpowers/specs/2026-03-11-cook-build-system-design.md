# Cook Build System — Design Spec

## Vision

Cook is a modern build system written in Rust that uses Lua as its scripting engine. The Cookfile is a DSL that looks like a declarative recipe but is transpiled into Lua for execution. It sits in the gap between Make (powerful but ugly syntax), Just (clean but no file watching), and programmable build engines like Premake (Lua power but no approachable DSL).

## Cookfile Syntax

### Recipe Structure

```
recipe "build"
    ingredients = {"src/*.c", "include/*.h"}
    serves = "bin/app"
    requires = {"setup"}

    gcc src/main.c -Iinclude -o bin/app
    > local size = fs.size("bin/app")
    >{
        if size > 1000000 then
            print("big!")
        end
    }
    taste
    chmod +x bin/app
end
```

### Line Classification Rules (in priority order)

1. Line whose first non-whitespace character is `#` → comment (Cookfile comment, not emitted to Lua)
2. `>{` → start multiline Lua block (ends at brace-balanced `}`)
3. `>` → single-line Lua
4. `taste` (standalone on a line) → debugger breakpoint
5. `recipe` → recipe header
6. `end` (standalone, at recipe scope) → recipe close
7. Known metadata identifier followed by `=` → metadata (only valid before the first step in a recipe)
8. Everything else → shell command

**Comments:** Cookfile uses `#` for comments. Inside `>{ }` Lua blocks, `--` is used as per standard Lua. This avoids ambiguity with shell flags like `--silent`.

**Metadata boundary:** Metadata lines (`ingredients =`, `serves =`, `requires =`) are only recognized before the first step (shell line, Lua line, Lua block, or `taste`). Once a step is encountered, all subsequent lines are classified as steps. A line like `serves = "bin/app"` appearing after a shell command is treated as a shell command.

### Whitespace

No whitespace significance. Indentation is purely cosmetic. Structure is determined by delimiters (`recipe`/`end`, `>{`/`}`), not indentation.

### Metadata Fields

- `ingredients` — table of glob patterns for input files
- `serves` — output file path string, or table of path strings for multiple outputs
- `requires` — table of recipe names (explicit dependencies)

### String Syntax

Strings use double quotes (`"hello"`). Escape sequences follow Lua conventions (`\"`, `\\`, `\n`). Single quotes are not supported in metadata — use double quotes for consistency.

## Architecture

### Pipeline

```
Cookfile (text)
    → Parser (Rust) → AST
    → Analyzer (Rust) → Dependency Graph + Validation
    → Codegen (Rust) → Lua source string
    → Runtime (Rust + mlua) → Execute Lua with injected Rust functions
```

### Module Structure

```
src/
  main.rs              # CLI entry point (clap)
  cli/
    mod.rs             # CLI arg parsing, subcommands
  parser/
    mod.rs             # Cookfile → AST
    lexer.rs           # Line-level tokenization
    ast.rs             # AST node types
  analyzer/
    mod.rs             # Dependency graph, validation
    graph.rs           # DAG construction + cycle detection
  codegen/
    mod.rs             # AST → Lua source string
  runtime/
    mod.rs             # mlua VM setup + execution
    api.rs             # Rust functions exposed to Lua (cook.exec, fs.size, etc.)
    taste.rs           # REPL debugger (future)
  watcher/
    mod.rs             # File watching for `cook serve`
  env/
    mod.rs             # .env file loading
```

### Key Crates

- `mlua` — Lua VM
- `clap` — CLI parsing
- `notify` — File watching for `cook serve`
- `glob` — Ingredient pattern expansion
- `dotenvy` — `.env` loading

## Grammar

```
COOKFILE     = (RECIPE | COMMENT | BLANK)*

RECIPE       = "recipe" STRING NEWLINE
               METADATA*
               STEP*
               "end"

METADATA     = KNOWN_KEY "=" EXPRESSION
KNOWN_KEY    = "ingredients" | "serves" | "requires"
EXPRESSION   = STRING | TABLE
STRING       = '"' (CHAR | ESCAPE)* '"'
TABLE        = "{" (EXPRESSION ("," EXPRESSION)*)? "}"

STEP         = SHELL_LINE
             | LUA_LINE
             | LUA_BLOCK
             | TASTE
             | COMMENT

SHELL_LINE   = any line not matching other step patterns
LUA_LINE     = ">" REST_OF_LINE
LUA_BLOCK    = ">{" NEWLINE ANY* "}"   (brace-depth balanced)
TASTE        = "taste"
COMMENT      = "#" REST_OF_LINE
```

**Metadata/step boundary:** The parser tracks whether it has seen the first step in a recipe. `METADATA` rules only match before the first step. After the first step, `ingredients = ...` is a shell command.

**Lua block closing:** The parser uses brace-depth counting to find the matching `}`. Each `{` inside the block increments depth, each `}` decrements. The block ends when depth reaches zero. This correctly handles nested Lua tables and control structures. The parser is aware of Lua string literals (double-quoted, single-quoted, and `[[` long strings) and `--` comments, and ignores braces inside them.

**Line numbers:** All line numbers in the AST and generated code refer to the source Cookfile line where the construct begins (first line of a `>{` block, not last).

## Codegen Output

Input:
```
recipe "build"
    ingredients = {"src/*.c"}
    serves = "bin/app"
    gcc src/main.c -o bin/app
    > local size = fs.size("bin/app")
    >{
        if size > 1000000 then
            print("big!")
        end
    }
    taste
    chmod +x bin/app
end
```

Output:
```lua
cook.recipe("build", {
    ingredients = {"src/*.c"},
    serves = "bin/app",
}, function()
    cook.exec("gcc src/main.c -o bin/app", 4)
    local size = fs.size("bin/app")
    if size > 1000000 then
        print("big!")
    end
    cook.taste(10)
    cook.exec("chmod +x bin/app", 11)
end)
```

- `cook.exec(cmd, line)` — wraps shell commands with source line for error reporting
- `cook.taste(line)` — debugger breakpoint, no-op in CI
- `cook.recipe(name, metadata, fn)` — registers recipe without executing
- Lua lines/blocks emitted verbatim
- All steps within a recipe share a single Lua function scope and execute sequentially in declaration order — variables from `>` lines are visible to subsequent steps

## Runtime & Lua API

### Core API

- `cook.recipe(name, metadata, fn)` — Register a recipe
- `cook.exec(cmd, line)` — Execute shell command, fail on non-zero exit. Returns stdout as a string, stderr passes through to terminal.
- `cook.taste(line)` — Drop into interactive REPL (no-op with `--no-taste`)
- `cook.env` — Table loaded from `.env`

### File System Helpers

- `fs.size(path)` — File size in bytes
- `fs.exists(path)` — Boolean check
- `fs.glob(pattern)` — Expand glob, returns table of paths
- `fs.read(path)` — Read file contents as string
- `fs.mtime(path)` — Last modified timestamp

### Shell Execution Model

- Each `cook.exec()` call spawns a separate subprocess via `/bin/sh -c "..."`
- Commands do not share shell state (no persistent `cd`, no shell variables between commands)
- Environment variables from `.env` and the parent process are inherited by each command
- Stdout from the command is captured and returned to Lua; stderr passes through to the terminal
- Non-zero exit code fails the build with an error referencing the Cookfile source line

### Execution Flow

1. Parse Cookfile → AST
2. Codegen → Lua source
3. Create mlua VM, register `cook.*` and `fs.*` functions
4. Load `.env` into `cook.env`
5. Execute Lua — registers recipes via `cook.recipe()` (does not run them)
6. Build dependency graph from `ingredients`/`serves` relationships + explicit `requires`
7. Topologically sort, execute recipes in order
8. Each recipe's function calls back into Rust for shell execution

Two-phase (register then execute) means `cook menu` can list recipes without running anything, and `cook serve` knows all globs to watch before any recipe fires.

## Dependency Resolution

### Dual Model

**Implicit (file-based):** String comparison between `serves` values and `ingredients` entries. If recipe B has `ingredients = {"bin/app"}` and recipe A has `serves = "bin/app"`, A runs before B. This is exact string matching, not glob expansion — implicit dependencies require a literal path match. Glob patterns in `ingredients` are only expanded at execution time for file watching and staleness checks.

**Explicit:** `requires = {"build"}` declares a dependency by recipe name for logical dependencies (e.g., deploy depends on test passing).

Both are combined into a single DAG. Cycle detection runs before execution.

### Future: Parallel Execution

The DAG naturally supports running independent recipes in parallel. V1 executes sequentially in topological order. Parallel execution is a future enhancement — the architecture supports it without changes to the Cookfile format.

## Environment

- Cook looks for `.env` in the same directory as the Cookfile
- If `.env` is missing, `cook.env` is an empty table (no error)
- `.env` values are available via `cook.env.VAR_NAME` in Lua and inherited by shell commands
- Shell commands inherit the parent process environment with `.env` values merged in. On conflict, `.env` values win (`.env` overrides system env for shell commands)
- `cook.env` in Lua always reflects `.env` file values, regardless of system environment

## File Watching (`cook serve`)

1. Parse Cookfile → collect all `ingredients` globs for target recipe + dependencies
2. Expand globs to get initial file list, also watch the glob parent directories for new files
3. Run full dependency chain (cold start)
4. Watch matched file paths via `notify`
5. On change: determine affected recipes by matching changed file against each recipe's `ingredients` globs, re-run from the earliest affected recipe in the dependency chain
6. Recipes with no ingredients that are dependencies of the target are not re-run on file changes (they only run on cold start)
7. Debounce: 200ms to batch rapid changes
8. Cookfile itself is watched — re-parse and restart on Cookfile changes
9. `cook serve` requires the target recipe (or at least one recipe in its dependency chain) to have `ingredients`. If no recipe in the chain has ingredients, Cook exits with an error: "nothing to watch"

## CLI Interface

```
cook [OPTIONS] [RECIPE]       Run a recipe (default: "build")
cook serve [RECIPE]           Watch & re-run on change (default: "build")
cook menu                     List all recipes with their ingredients/serves
cook init                     Generate a starter Cookfile
```

### Global Options

- `--file <path>` / `-f` — Specify Cookfile path (default: `./Cookfile`)
- `--plate` — Print transpiled Lua instead of executing
- `--quiet` / `-q` — Suppress Cook output, only show command output
- `--no-taste` — Skip `taste` breakpoints (for CI)

### Exit Codes

- `0` — success
- `1` — recipe failed (command returned non-zero)
- `2` — Cookfile parse error
- `3` — recipe not found

## `taste` Debugger (Future)

`taste` is an inline keyword (like `debugger` in JavaScript). When Cook hits it during execution, it drops into an interactive Lua REPL with full build context — access to `cook.env`, `fs.*`, all variables in scope. Type commands to inspect state, then continue execution.

Skipped automatically with `--no-taste` or in non-interactive environments.
