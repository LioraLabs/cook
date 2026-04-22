# Parser: Lexer, AST, and Parsing Pipeline

## Overview

Parsing a Cookfile happens in two sequential stages:

1. **Lexing** (`src/parser/lexer.rs`): the raw text is scanned line-by-line and each line is classified into a `Token` variant. The result is a flat `Vec<Located<Token>>` where each entry pairs a token with its 1-indexed source line number.

2. **Parsing** (`src/parser/mod.rs`): the token stream is consumed left-to-right by a hand-written recursive-descent parser. It builds the structured `Cookfile` AST defined in `src/parser/ast.rs`.

The public entry point is `parse(source: &str) -> Result<Cookfile, ParseError>` in `src/parser/mod.rs:16`. It calls `tokenize()`, then walks the token stream.

---

## Lexer (`src/parser/lexer.rs`)

### `tokenize()`

`tokenize(source: &str) -> Result<Vec<Located<Token>>, LexError>` (line 71) iterates over each line of the source with `.lines().enumerate()`. Every line — including blank ones — produces exactly one `Located<Token>`. The `Located<T>` wrapper (line 17–21) carries the value and a 1-indexed `line: usize`.

### Token Variants

Defined in the `Token` enum at `src/parser/lexer.rs:4–15`:

| Variant | Produced when |
|---|---|
| `Comment(String)` | Line (trimmed) starts with `#`. The payload is everything after the `#`. |
| `RecipeHeader { name: String, deps: Vec<String> }` | Line starts with `recipe` followed by a space, tab, or `"`. Name and optional `:`-separated dep list are parsed as quoted strings. |
| `ConfigHeader { name: String }` | Line starts with `config` followed by a space, tab, or `"`. Only the name is captured; config blocks have no inline deps. |
| `VarDecl { name: String, value: String }` | Line matches `NAME "value"` where `NAME` is not a reserved keyword. Parsed by `try_parse_var_decl()` at line 48. |
| `RecipeEnd` | Trimmed line is exactly `end`. |
| `LuaLine(String)` | Trimmed line starts with `>` but not `>{`. The payload is the code after `>` (with one optional leading space stripped). |
| `LuaBlockOpen` | Trimmed line starts with `>{`. Signals the start of a multi-line Lua block; the block body is collected separately by the parser. |
| `Taste` | Trimmed line is exactly `taste`. |
| `Blank` | Trimmed line is empty (whitespace-only lines also produce `Blank`). |
| `Content(String)` | Everything else, including lines starting with `@`. The trimmed line is the payload. |

### How Each Line Type Is Recognized

Recognition is a priority cascade (lines 78–140). Order matters because some prefixes overlap:

1. Empty → `Blank`
2. Starts with `#` → `Comment`
3. Starts with `>{` → `LuaBlockOpen` (checked before `>` to avoid ambiguity)
4. Starts with `>` → `LuaLine`
5. Exactly `taste` → `Taste`
6. Exactly `end` → `RecipeEnd`
7. Starts with `recipe` + separator → `RecipeHeader` (requires a quoted name; unquoted names are a `LexError`)
8. Starts with `config` + separator → `ConfigHeader`
9. Does not start with `@` and matches `try_parse_var_decl()` → `VarDecl`
10. Everything else (including lines starting with `@`) → `Content`

**Important:** Lines beginning with `@` fall through to step 10 and become `Content` tokens. The `@` interactive prefix is not a lexer concern — it is interpreted by the parser.

### Keywords Blocked from Variable Names

`try_parse_var_decl()` rejects names that are reserved keywords (line 54):

```
recipe  config  end  ingredients  cook  plate  taste  using
```

This prevents `end "foo"` or `cook "bar"` from being misclassified as variable declarations at the global scope.

### Tokenization Example

Given this Cookfile snippet:

```
CC "gcc"

recipe "build": "setup"
    ingredients "src/*.c"
    gcc -c {in} -o {out}
    @./run-tests
    > print("done")
end
```

The lexer produces (one token per line, with 1-indexed line numbers):

| Line | Token |
|---:|---|
| 1 | `VarDecl { name: "CC", value: "gcc" }` |
| 2 | `Blank` |
| 3 | `RecipeHeader { name: "build", deps: ["setup"] }` |
| 4 | `Content("ingredients \"src/*.c\"")` |
| 5 | `Content("gcc -c {in} -o {out}")` |
| 6 | `Content("@./run-tests")` |
| 7 | `LuaLine("print(\"done\")")` |
| 8 | `RecipeEnd` |

Note that line 4 (`ingredients ...`) is `Content` at the lexer stage — the `ingredients` keyword is not special to the lexer. Line 6 (`@./run-tests`) is also `Content`; the parser is responsible for interpreting `@`.

---

## AST Structures (`src/parser/ast.rs`)

### `Cookfile` (line 2–6)

The top-level parse result:

```rust
pub struct Cookfile {
    pub vars:    Vec<(String, String)>,
    pub configs: HashMap<String, Vec<(String, String)>>,
    pub recipes: Vec<Recipe>,
}
```

- `vars` — global variable declarations as `(name, value)` pairs, in source order.
- `configs` — named configuration blocks; each maps to a list of `(name, value)` overrides.
- `recipes` — the recipe list, in source order.

### `Recipe` (line 9–15)

```rust
pub struct Recipe {
    pub name:        String,
    pub deps:        Vec<String>,
    pub ingredients: Vec<String>,
    pub steps:       Vec<Step>,
    pub line:        usize,
}
```

- `name` — the quoted recipe name.
- `deps` — recipes this one depends on (from the `: "dep1" "dep2"` header syntax).
- `ingredients` — glob patterns from the `ingredients` line inside the recipe body. At most one `ingredients` line is allowed per recipe.
- `steps` — the ordered sequence of actions to perform.
- `line` — source line of the `recipe` header, used in error messages.

### `Step` (line 35–42)

```rust
pub enum Step {
    Shell     { command: String, line: usize, interactive: bool },
    Lua       { code: String,    line: usize },
    LuaBlock  { code: String,    line: usize },
    Taste     { line: usize },
    Cook      { step: CookStep,  line: usize },
    Plate     { step: PlateStep, line: usize },
}
```

- `Shell` — a bare shell command. `interactive: true` when the source line was prefixed with `@`.
- `Lua` — a single-line Lua expression, from a `> code` line.
- `LuaBlock` — a multi-line Lua block, from a `>{` ... `}` block. `code` contains the collected lines joined with `\n`.
- `Taste` — a debug breakpoint step; the bare `taste` keyword.
- `Cook` — a file-transformation step with an output pattern and optional build command.
- `Plate` — a run step that executes a command for each output file.

### `CookStep` (line 24–27)

```rust
pub struct CookStep {
    pub outputs:      Vec<String>,
    pub using_clause: Option<UsingClause>,
}
```

- `outputs` — the list of quoted output file patterns (e.g., `["build/obj/{stem}.o"]`, or `["out/parser.rs", "out/parser.h"]` for a multi-output step). A `cook` line must declare at least one output.
- `using_clause` — the optional build command. `None` means the `cook` line is a declaration only (used to announce the output without providing a build command inline).

**Multi-output cook steps.** When two or more quoted patterns appear before `using`, the step represents a single invocation that produces all of those outputs in one shot (e.g., a parser generator that writes both a `.rs` and a `.h` file). All listed patterns share one cache entry and one work unit — they are materialized together or not at all. Multi-output form requires a block-style `using` clause (see `UsingClause::ShellBlock` / `LuaBlock` below); the single-line `using "cmd"` form is single-output only.

### `PlateStep` (line 30–32)

```rust
pub struct PlateStep {
    pub command: String,
}
```

- `command` — the quoted command to run (e.g., `"./{out}"`).

Note: `PlateStep` does not have a `using_clause` field. The `using` keyword is only valid on `cook` lines.

### `UsingClause` (line 18–21)

```rust
pub enum UsingClause {
    Shell(String),
    LuaBlock(String),
    ShellBlock(Vec<String>),
}
```

- `Shell(String)` — a quoted shell command string following `using` (the `using "cmd"` form). Single-output only.
- `LuaBlock(String)` — a `>{` ... `}` Lua block following `using` (the `using >{ ... }` form). The payload is the collected Lua source; codegen emits it as `lua_code` on `cook.add_unit`, which the worker pool executes with `input`/`output`/`inputs`/`outputs` and `input_N` globals.
- `ShellBlock(Vec<String>)` — a plain-brace block following `using` (the `using { ... }` form). Each element is one shell command line; lines run sequentially in a single shell work unit that claims all declared outputs.

**Cook-line syntax summary:**

```
cook "out"                          # declaration only
cook "out" using "cmd ..."          # single-output shell (supports {in}/{out}/{all})
cook "out" using >{ lua ... }       # single-output Lua block
cook "a" "b" using { shell ... }    # multi-output shell block (a and b produced together)
cook "a" "b" using >{ lua ... }     # multi-output Lua block (a and b produced together)
```

---

## Parser (`src/parser/mod.rs`)

### Error Types

`ParseError` (line 8–14) wraps both stages:

```rust
pub enum ParseError {
    Lex(LexError),
    Parse { line: usize, message: String },
}
```

`LexError` (lexer.rs:23–29) covers two cases: `UnterminatedString` and `MissingRecipeName`. Both carry a `line` number.

### Four Parsing Scopes

The parser operates in four distinct scopes, each implemented as a separate loop or function:

#### 1. Global Scope (`parse()`, line 16–87)

The top-level loop in `parse()`. It consumes tokens until the input is exhausted. Valid token types at global scope:

- `Comment` / `Blank` — skipped.
- `VarDecl` — added to `vars`. Error if a recipe has already been seen (`seen_recipe` flag, line 23).
- `ConfigHeader` — delegates to `parse_config_block()`. Error if a recipe has already been seen.
- `RecipeHeader` — sets `seen_recipe = true`, delegates to `parse_recipe()`.
- `RecipeEnd`, `Content`, `LuaLine`, `LuaBlockOpen`, `Taste` — all produce a `Parse` error ("unexpected content outside of a recipe").

**Ordering constraint:** Variables and config blocks must appear before any `recipe` declaration. The `seen_recipe` flag enforces this. A `VarDecl` or `ConfigHeader` that appears after the first recipe triggers an error at lines 32–37 and 42–47.

#### 2. Config Block Scope (`parse_config_block()`, line 89–124)

Called immediately after a `ConfigHeader` token is consumed. Loops until `RecipeEnd`:

- `RecipeEnd` — closes the block and returns.
- `VarDecl` — appended to the config's variable list.
- `Comment` / `Blank` — skipped.
- Anything else — error: "config blocks may only contain variable declarations".

If the token stream ends without a `RecipeEnd`, the error references the `open_line` of the `config` header.

#### 3. Recipe Scope (`parse_recipe()`, line 243–370)

Called after a `RecipeHeader` token is consumed. Loops until `RecipeEnd`.

Token handling inside a recipe:

- `RecipeEnd` — closes the recipe, returns the built `Recipe`.
- `Comment` / `Blank` — skipped.
- `Content(text)` — dispatched by prefix (see below).
- `LuaLine(code)` — appended as `Step::Lua`.
- `LuaBlockOpen` — delegates to `collect_lua_block()`, appended as `Step::LuaBlock`.
- `Taste` — appended as `Step::Taste`.
- `RecipeHeader` — error: previous recipe not closed with `end`.
- `ConfigHeader` — error: config inside a recipe.
- `VarDecl` — treated as a shell command. The lexer would have emitted `VarDecl` for `NAME "value"` even inside a recipe; the parser recovers this by reconstructing the original text as `NAME "value"` and wrapping it in `Step::Shell { interactive: false }` (lines 353–361).

**`Content` token dispatch** (lines 274–317):

The `strip_keyword(text, keyword)` helper (line 126–137) returns the remainder if `text` begins with `keyword` followed by a space or tab (or if `keyword` exactly equals `text`). It returns `None` for prefixes that don't have a separator (e.g., `strip_keyword("cooking", "cook")` returns `None`).

Dispatch order for a `Content(text)` token:

1. `strip_keyword(text, "ingredients")` matches → parse the remainder as quoted strings, store in `ingredients`. Error if `ingredients` is already non-empty (duplicate).
2. `strip_keyword(text, "cook")` matches → `parse_cook_line()`, push `Step::Cook`.
3. `strip_keyword(text, "plate")` matches → parse a single quoted string as the command, push `Step::Plate`.
4. `text.strip_prefix('@')` succeeds → strip the `@`, require non-empty remainder, push `Step::Shell { interactive: true }`.
5. Otherwise → push `Step::Shell { command: text.clone(), interactive: false }`.

#### 4. Lua Block Scope (`collect_lua_block()`, line 375–422)

Called when a `LuaBlockOpen` token is encountered (either as a standalone step or as the `using >{` clause of a `cook` line). This scope operates on **raw source lines**, not tokens, to preserve original formatting.

The function receives:
- `open_line` — the 1-indexed line number of the `>{` token.
- `token_pos` — the position in the token stream immediately after `LuaBlockOpen`.
- `source_lines` — the original source split into lines.

It starts reading from `source_lines[open_line]` (the line immediately after `>{`) and walks forward, calling `count_brace_delta()` on each raw line. A running `depth` starts at 1 (for the opening `{`). When `depth` reaches 0 the closing line is found; that line is not included in the output.

After collecting lines, the function fast-forwards `token_pos` past all tokens whose line number is ≤ the closing brace's line, then returns.

**Brace counting (`count_brace_delta()`, line 427–496):**

Scans a single raw line character-by-character, incrementing `delta` for `{` and decrementing for `}`. Braces are ignored when they appear:

- Inside `--` line comments (rest of line skipped).
- Inside `[[ ... ]]` long strings.
- Inside double-quoted strings (with backslash-escape handling).
- Inside single-quoted strings (with backslash-escape handling).

This means a Lua line like `local s = "}"` contributes `delta = 0`, not `-1`.

### How `@` Is Handled

The `@` prefix never reaches the lexer's dedicated handling — by design, lines starting with `@` fall through to `Token::Content` (lexer.rs:138–139). Inside `parse_recipe()`, the `Content` dispatch checks `text.strip_prefix('@')` (mod.rs:298). If it matches and the remainder is non-empty, the `@` is stripped and `Step::Shell { interactive: true }` is created. An empty `@` (bare `@` with nothing after it) is a `Parse` error.

This design keeps the lexer simple and makes the interactive flag a parser-level semantic concern.

### `parse_cook_line()` (line 176–241)

Handles the text after the `cook` keyword. Parses:

1. One or more quoted output patterns (at least one required).
2. An optional `using` clause, which is one of:
   - A quoted shell string → `UsingClause::Shell` (single-output only).
   - `>{` followed by a Lua block → `UsingClause::LuaBlock`, collected via `collect_lua_block()`.
   - `{` followed by a plain-shell block → `UsingClause::ShellBlock`, collected as a list of non-empty lines until the matching `}`. This is the form to use when multiple outputs are declared.

If more than one output is declared, the `using` clause must be a block form (`{ ... }` or `>{ ... }`); `using "cmd"` is rejected at parse time.

If the `using` keyword is absent, `using_clause` is `None` and the step is a declaration-only cook.
