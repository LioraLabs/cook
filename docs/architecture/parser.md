# Parser: Lexer, AST, and Parsing Pipeline

> **This document describes the Rust parser implementation.** The Cookfile language itself is defined by the Cook Standard in `standard/` — in particular `02-lexical.mdx`, `03-syntactic-grammar.mdx`, `04-recipes.mdx`, `04a-chores.mdx`, `07-cross-cookfile-composition.mdx`, and `appendix/A-grammar.mdx`. The Standard is authoritative for the language; this doc describes how the Rust crate implements it.
>
> This document may lag behind the Standard on specific constructs; when in doubt, the Standard (and the `cook-lang` source) are authoritative.

The parser lives in the `cook-lang` crate at `cli/crates/cook-lang/`. It has no dependencies on the rest of the CLI — `parse(source: &str)` is the only entry point and it returns a structurally validated AST.

## Overview

Parsing a Cookfile happens in two sequential stages:

1. **Lexing** (`cli/crates/cook-lang/src/lexer.rs`): the raw text is scanned line-by-line and each line is classified into a `Token` variant. The result is a flat `Vec<Located<Token>>` where each entry pairs a token with its 1-indexed source line number.

2. **Parsing** (`cli/crates/cook-lang/src/lib.rs`, with helpers in `recipe.rs`, `cook_line.rs`, `lua_block.rs`, `shell_block.rs`, and `brace_scan.rs`): the token stream is consumed left-to-right by a hand-written recursive-descent driver that builds the `Cookfile` AST defined in `ast.rs`.

The public entry point is `parse(source: &str) -> Result<Cookfile, ParseError>` at `cli/crates/cook-lang/src/lib.rs:128`. It calls `tokenize()`, then walks the token stream calling specialized helpers for each top-level form.

The crate also exposes `COOK_STANDARD_VERSION` (`cli/crates/cook-lang/src/lib.rs:17`), which names the Cook Standard cut the parser claims to fully implement (i.e., every case under `standard/conformance/` passes).

---

## Lexer (`cli/crates/cook-lang/src/lexer.rs`)

### `tokenize()`

`tokenize(source: &str) -> Result<Vec<Located<Token>>, LexError>` (`cli/crates/cook-lang/src/lexer.rs:120`) iterates over each line of the source with `.lines().enumerate()`. Every non-empty source line produces exactly one `Located<Token>`. The `Located<T>` wrapper (`cli/crates/cook-lang/src/lexer.rs:20`) carries the value and a 1-indexed `line: usize`.

### Token Variants

Defined in the `Token` enum at `cli/crates/cook-lang/src/lexer.rs:4`:

| Variant | Produced when |
|---|---|
| `Comment(String)` | Trimmed line starts with `#`. Payload is everything after the `#`. |
| `RecipeHeader { name, deps }` | At column 0: `recipe` followed by space/tab/`"`, then a quoted or bare identifier name, then an optional `: dep1 dep2 …` list. |
| `ChoreHeader { name, deps }` | At column 0: `chore` followed by space/tab/`"`, same shape as `recipe`. |
| `ConfigHeader { name: Option<String> }` | At column 0: either bare `config` (unnamed) or `config` + separator + name. |
| `UseDecl { name }` | At column 0: `use` followed by separator and a Lua-identifier name. |
| `ImportDecl { name, path }` | At column 0: `import` + space/tab + alias + space/tab + path. |
| `LuaLine(String)` | Trimmed line starts with `>` (not `>>` or `>{`). Payload is the code after `>` with one optional leading space stripped. |
| `LuaBlockOpen` | Trimmed line starts with `>{`. Signals an execute-phase Lua block; the body is collected later by the parser. |
| `InlineLuaLine(String)` | Trimmed line starts with `>>` (not `>>{` and not `>>>`). Payload is the code after `>>` with one optional leading space stripped. |
| `InlineLuaBlockOpen` | Trimmed line starts with `>>{`. Signals a register-phase Lua block. |
| `Blank` | Trimmed line is empty. |
| `Content(String)` | Everything else. The trimmed line is the payload. |

`>>>` (three or more `>` at line start) is reserved and produces `LexError::ReservedTripleArrow` (`cli/crates/cook-lang/src/lexer.rs:33`).

### Lua-Line vs Inline-Lua-Line

There are two Lua line prefixes, distinguished by phase:

- `>` (and `>{ … }`) — **execute-phase**. The code runs on the worker VM at execute time. Tokenized as `LuaLine` / `LuaBlockOpen` and built into `Step::Lua` / `Step::LuaBlock`.
- `>>` (and `>>{ … }`) — **register-phase** ("inline" Lua). The code is inlined into the recipe-body Lua function and runs during registration. Tokenized as `InlineLuaLine` / `InlineLuaBlockOpen` and built into `Step::InlineLua` / `Step::InlineLuaBlock`.

The order of checks in `tokenize()` matters: `>>>` is rejected first, then `>>{`, then `>>`, then `>{`, then bare `>`. The longer prefix always wins.

### How Each Line Type Is Recognized

Recognition is a priority cascade (`cli/crates/cook-lang/src/lexer.rs:123`–`242`). Order matters because some prefixes overlap:

1. Empty trimmed line → `Blank`.
2. Starts with `#` → `Comment`.
3. Starts with `>>>` → `LexError::ReservedTripleArrow`.
4. Starts with `>>{` → `InlineLuaBlockOpen`.
5. Starts with `>>` → `InlineLuaLine`.
6. Starts with `>{` → `LuaBlockOpen`.
7. Starts with `>` → `LuaLine`.
8. Column 0 + `recipe` + separator → `RecipeHeader`.
9. Column 0 + `chore` + separator → `ChoreHeader`.
10. Column 0 + bare `config` → `ConfigHeader { name: None }`.
11. Column 0 + `config` + separator → `ConfigHeader { name: Some(_) }`.
12. Column 0 + `use` + separator → `UseDecl`.
13. Column 0 + `import` + separator → `ImportDecl`.
14. Anything else → `Content(trimmed_line)`.

**Column-0 requirement.** `recipe`, `chore`, `config`, `use`, and `import` are only recognized as keywords when the original raw line begins at column 0 (no leading whitespace). An indented `recipe inner` becomes `Content("recipe inner")` and dispatches as a shell command inside a recipe body (CS-0019 / E.5). The check is `!line.starts_with(|c: char| c.is_whitespace())` on the raw line, before trimming.

**Separator requirement.** A keyword is only recognized if the next byte is a space, tab, or `"` (for `recipe` / `chore` / `config` / `use`) or a space/tab (for `import`). This prevents `configure` from lexing as a `config` header. Bare `config` (with no name) is allowed; bare `recipe`, `chore`, `use`, `import` are not.

**Recipe / chore names.** Names may be quoted (`recipe "build"`) or bare (`recipe build`). Bare identifiers are read by `parse_name()` (`cli/crates/cook-lang/src/lexer.rs:89`) using the `is_ident_char` predicate (alphanumeric, `_`, `-`, `.`). Several names are rejected at lex time:
- Reserved segments — `stem`, `name`, `ext`, `dir`, `in`, `out`, `all` (last-segment check) and `env` (first-segment check) — produce `LexError::ReservedRecipeName`.
- Dotted recipe/chore names at the *declaration* site (e.g., `recipe backend.build`) are rejected with `DottedDeclaredRecipeName` / `DottedDeclaredChoreName`. Dotted names remain legal in *dependency* lists because they resolve through `import` aliases.

**`use` names.** `use NAME` is dropped verbatim into a `local NAME = …` Lua binding by codegen, so the name must be a valid Lua identifier (`^[A-Za-z_][A-Za-z0-9_]*$`). Hyphens, dots, and digit-leading names are rejected with `LexError::InvalidUseName` (`cli/crates/cook-lang/src/lexer.rs:77`).

### `@` Is Not a Lexer Concern

Lines beginning with `@` fall through to `Content`. The `@` interactive marker is interpreted by the parser inside a recipe body — see *Recipe Scope* below. Keeping `@` out of the lexer means a line like `@./run-tests` is just `Content("@./run-tests")` and remains uniform with every other shell line.

---

## AST Structures (`cli/crates/cook-lang/src/ast.rs`)

### `Cookfile` (`cli/crates/cook-lang/src/ast.rs:51`)

The top-level parse result:

```rust
pub struct Cookfile {
    pub config_blocks: Vec<ConfigBlock>,
    pub recipes:       Vec<Recipe>,
    pub chores:        Vec<Chore>,
    pub uses:          Vec<UseStatement>,
    pub imports:       Vec<ImportDecl>,
}
```

There is no `vars` field — top-level `NAME "value"` declarations no longer exist in the language. All variables live inside `config` blocks, whose bodies are raw Lua source.

### `ConfigBlock` (`cli/crates/cook-lang/src/ast.rs:36`)

```rust
pub struct ConfigBlock {
    pub name: Option<String>,   // None for the unnamed block
    pub body: String,           // raw Lua source between the `config` header and the next top-level keyword
    pub line: usize,
}
```

Configs are Lua-bodied: the parser captures the raw source from the line *after* the `config` header up to (but not including) the next column-0 top-level keyword (or EOF). The body is **not** tokenized as Cookfile steps — it is handed to Lua at registration time. See `parse_config_block_lua` in `cli/crates/cook-lang/src/recipe.rs:83`.

At most one unnamed config block is allowed; named blocks must have unique names. Both rules are enforced in the global loop (`cli/crates/cook-lang/src/lib.rs:160`–`176`).

### `UseStatement` (`cli/crates/cook-lang/src/ast.rs:2`)

```rust
pub struct UseStatement {
    pub module_name: String,
    pub line: usize,
}
```

A `use NAME` line. Codegen emits `local NAME = cook.load_module("NAME")` at the top of the recipe-body Lua function.

### `ImportDecl` and `ImportPath` (`cli/crates/cook-lang/src/ast.rs:9`, `:29`)

```rust
pub enum ImportPath {
    Tree(String),   // tree-relative (forward-only, no `..`, not absolute)
    Sigil(String),  // workspace-root-anchored (the prefix `//` has been stripped)
}

pub struct ImportDecl {
    pub name: String,
    pub path: ImportPath,
    pub line: usize,
}
```

The lexer captures the raw path as a string; `parse()` validates it via `validate_and_classify_import_path` (`cli/crates/cook-lang/src/lib.rs:37`):

- A path starting with `//` is **sigil-anchored**: the stored `Sigil(s)` payload is the text *after* the sigil, and it must not contain `..` segments or begin with another `/`.
- Any other path is **tree-relative**: absolute paths (leading `/`) and `..` segments are both rejected.

Sigil-anchored paths resolve from the workspace root; tree-relative paths resolve from the importing Cookfile's directory. Duplicate import names within one Cookfile are rejected at parse time (`cli/crates/cook-lang/src/lib.rs:264`).

### `Recipe` (`cli/crates/cook-lang/src/ast.rs:60`)

```rust
pub struct Recipe {
    pub name:        String,
    pub deps:        Vec<String>,
    pub ingredients: Vec<String>,
    pub excludes:    Vec<String>,
    pub steps:       Vec<Step>,
    pub line:        usize,
}
```

- `deps` — recipes/chores this one depends on (from the `: dep1 dep2 …` header tail). Dotted names like `backend.build` resolve through `import` aliases at register time.
- `ingredients` / `excludes` — glob patterns from the recipe's single `ingredients` line. Include patterns are bare `"pattern"`; exclude patterns are `!"pattern"`. At most one `ingredients` line is allowed per recipe.
- `steps` — the ordered sequence of actions.
- `line` — source line of the `recipe` header, used in error messages.

### `Chore` (`cli/crates/cook-lang/src/ast.rs:43`)

```rust
pub struct Chore {
    pub name:  String,
    pub deps:  Vec<String>,
    pub steps: Vec<Step>,
    pub line:  usize,
}
```

A chore is a top-level callable with no build outputs. Same `Step` enum as a recipe, but `ingredients`, `cook`, `plate`, and `test` are **not** allowed in a chore body (see `chore_banned` in `cli/crates/cook-lang/src/recipe.rs:539`). Recipes and chores share a single callable namespace: duplicate-name detection at the global level (`callable_decls` map in `cli/crates/cook-lang/src/lib.rs:142`) rejects any collision between two recipes, two chores, or one of each, per App. A.2.

### `Step` (`cli/crates/cook-lang/src/ast.rs:102`)

```rust
pub enum Step {
    Shell          { command: String, line: usize, interactive: bool },
    Lua            { code: String,    line: usize },  // `>` — execute-phase
    LuaBlock       { code: String,    line: usize },  // `>{ … }` — execute-phase
    InlineLua      { code: String,    line: usize },  // `>>` — register-phase
    InlineLuaBlock { code: String,    line: usize },  // `>>{ … }` — register-phase
    Cook           { step: CookStep,  line: usize },
    Plate          { step: PlateStep, line: usize },
    Test           { step: TestStep,  line: usize },
}
```

| Variant | Source form | Phase |
|---|---|---|
| `Shell` | Bare command, optionally `@`-prefixed | execute |
| `Lua` | `> code` | execute |
| `LuaBlock` | `>{ … }` | execute |
| `InlineLua` | `>>` or a module-call line like `cpp.binary(…)` | register |
| `InlineLuaBlock` | `>>{ … }` or a multi-line module call | register |
| `Cook` | `cook "out" using …` | declaration (register) |
| `Plate` | `plate { … }` or `plate >{ … }` | declaration (register) |
| `Test` | `test { … }` with optional modifier tail | declaration (register) |

`Step::is_imperative()` (`cli/crates/cook-lang/src/ast.rs:125`) returns `true` for `Shell` / `Lua` / `LuaBlock`. The parser uses this to enforce the App. A.3 region-ordering rule (see *Recipe Scope* below).

### `CookStep` (`cli/crates/cook-lang/src/ast.rs:82`)

```rust
pub struct CookStep {
    pub outputs:      Vec<String>,
    pub using_clause: Option<UsingClause>,
}
```

- `outputs` — one or more quoted output patterns (e.g., `["build/obj/{stem}.o"]`, or `["out/parser.rs", "out/parser.h"]` for a multi-output step). A `cook` line must declare at least one output.
- `using_clause` — `None` means the `cook` line is a declaration only (the output is announced without an inline build command).

**Multi-output cook steps.** When two or more quoted patterns appear before the body, the step represents a single invocation that produces all of those outputs together. They share one cache entry and one work unit — materialized together or not at all.

### `PlateStep`, `TestStep`, and `Body` (`cli/crates/cook-lang/src/ast.rs:79`, `:88`, `:93`)

```rust
pub type Body = UsingClause;

pub struct PlateStep { pub body: Body }
pub struct TestStep  {
    pub body:        Body,
    pub as_name:     Option<String>,
    pub timeout:     Option<u64>,
    pub should_fail: bool,
}
```

`Body` is a type alias for `UsingClause` (CS-0024): `cook`'s `using_clause`, `plate`'s body, and `test`'s body all share one production. Test steps additionally accept a canonical-order modifier tail after the closing `}`: `as 'NAME'`, `timeout N`, `should_fail`, parsed by `parse_test_modifier_tail` (`cli/crates/cook-lang/src/recipe.rs:155`). The `as` modifier is rejected on `cook` and `plate` (`reject_as_modifier_on_non_test`, `cli/crates/cook-lang/src/recipe.rs:246`).

### `UsingClause` (`cli/crates/cook-lang/src/ast.rs:69`)

```rust
pub enum UsingClause {
    ShellBlock(Vec<String>),  // `{ cmd1; cmd2; … }` — one element per non-blank line
    LuaBlock(String),         // `>{ … }` — raw Lua source
}
```

There is no bare-string form (`using "cmd"` was removed in CS-0024). A one-line shell block is written as `{ cmd }`.

Body grammar summary:

```
cook "out"                          # declaration only
cook "out" { shell … }        # shell block (single- or multi-line)
cook "out" >{ lua … }         # Lua block

plate { shell … }                   # shell block
plate >{ lua … }                    # Lua block

test { shell … } as 'NAME' timeout 30 should_fail   # any subset, canonical order
test >{ lua … }
```

---

## Parser

### Error Types

`ParseError` (`cli/crates/cook-lang/src/lib.rs:27`) wraps both stages:

```rust
pub enum ParseError {
    Lex(LexError),
    Parse { line: usize, message: String },
}
```

`LexError` (`cli/crates/cook-lang/src/lexer.rs:26`) covers `UnterminatedString`, `MissingRecipeName`, `ReservedRecipeName`, `ReservedTripleArrow`, `InvalidUseName`, `DottedDeclaredRecipeName`, and `DottedDeclaredChoreName`. All variants carry a `line` number.

### Five Parsing Scopes

The parser operates in five scopes, each implemented as a separate function:

#### 1. Global Scope (`parse()`, `cli/crates/cook-lang/src/lib.rs:128`)

The top-level loop in `parse()`. Valid tokens at global scope:

- `Comment` / `Blank` — skipped.
- `ConfigHeader` — delegate to `parse_config_block_lua()`. Error if any recipe or chore has already been seen.
- `RecipeHeader` — record the name in `callable_decls`, delegate to `parse_recipe()`.
- `ChoreHeader` — record the name in `callable_decls`, delegate to `parse_chore()`.
- `UseDecl` — append to `uses`. Error if any recipe or chore has already been seen.
- `ImportDecl` — validate the path, check for duplicate alias, append to `imports`. Error if any recipe or chore has already been seen.
- `Content`, `LuaLine`, `LuaBlockOpen`, `InlineLuaLine`, `InlineLuaBlockOpen` — error: "unexpected content outside of a recipe".

**Ordering constraint.** Configs, uses, and imports must appear before any recipe or chore. The shared `seen_recipe` flag (`cli/crates/cook-lang/src/lib.rs:137`) enforces this — it is set to `true` by both `RecipeHeader` and `ChoreHeader`, and checked on every subsequent `ConfigHeader` / `UseDecl` / `ImportDecl`.

**Duplicate-callable detection.** A single `BTreeMap<String, (CallableKind, usize)>` records every recipe and chore declaration. Any subsequent collision — recipe-vs-recipe, chore-vs-chore, or recipe-vs-chore — is rejected via `duplicate_callable_error` (`cli/crates/cook-lang/src/lib.rs:109`), which names both the new and prior declaration's kind and line.

#### 2. Config-Block Scope (`parse_config_block_lua()`, `cli/crates/cook-lang/src/recipe.rs:83`)

Called immediately after a `ConfigHeader` token is consumed. Unlike a recipe body, a config block has no `end` keyword and is **not** parsed as Cookfile steps. It is closed implicitly by the next column-0 top-level keyword (or EOF).

The function scans forward in the token stream until it finds the next `RecipeHeader`, `ChoreHeader`, `ConfigHeader`, `UseDecl`, or `ImportDecl` — that terminator is **left in place** for `parse()` to dispatch. The body is the raw source lines from `header_line` through the line before the terminator, with trailing blank lines trimmed so that whitespace between blocks does not end up in the body.

The body is stored verbatim in `ConfigBlock.body`; the engine hands it to Lua at registration time.

#### 3. Recipe Scope (`parse_recipe()`, `cli/crates/cook-lang/src/recipe.rs:268`)

Called after a `RecipeHeader` is consumed. Loops until the recipe body ends.

**Body termination is implicit (CS-0019).** There is no `end` keyword in v0.4+. A recipe (or chore) body is closed by any of:
- The next column-0 top-level keyword (`RecipeHeader`, `ChoreHeader`, `ConfigHeader`, `UseDecl`, `ImportDecl`) — that token is left in place for `parse()` to dispatch.
- End of file.

Token handling inside a recipe:

- `Comment` / `Blank` — skipped.
- `Content(text)` — dispatched by prefix (see below).
- `LuaLine(code)` — appended as `Step::Lua`. Marks the imperative region as started.
- `LuaBlockOpen` — delegates to `collect_lua_block()`, appended as `Step::LuaBlock`. Marks the imperative region as started.
- `InlineLuaLine(code)` — appended as `Step::InlineLua`. Rejected if the imperative region has already started.
- `InlineLuaBlockOpen` — delegates to `collect_lua_block()`, appended as `Step::InlineLuaBlock`. Rejected if the imperative region has already started.

**`Content` token dispatch** (`cli/crates/cook-lang/src/recipe.rs:321`–`455`):

The `strip_keyword(text, keyword)` helper (`cli/crates/cook-lang/src/cook_line.rs:71`) returns the remainder if `text` begins with `keyword` followed by a space or tab (or exactly equals `keyword`). Dispatch order:

1. `strip_keyword(text, "ingredients")` matches → parse as include/exclude patterns via `parse_ingredients_line` (`cli/crates/cook-lang/src/cook_line.rs:86`), store in `ingredients` / `excludes`. Duplicate `ingredients` lines and any `ingredients` after the imperative region has started are both errors.
2. `strip_keyword(text, "cook")` matches → `parse_cook_line()`, push `Step::Cook`.
3. `strip_keyword(text, "plate")` matches → `parse_body_payload()`, push `Step::Plate`.
4. `strip_keyword(text, "test")` matches → `parse_body_payload()` + `parse_test_modifier_tail()`, push `Step::Test`.
5. `is_module_call(text)` matches (`ident.ident`-shaped) → `collect_module_call()` (handles multi-line via `LuaScanner`), push `Step::InlineLua` (single-line) or `Step::InlineLuaBlock` (multi-line).
6. `text.strip_prefix('@')` succeeds → strip the `@`, require non-empty remainder, push `Step::Shell { interactive: true }`. Marks the imperative region as started.
7. Otherwise → push `Step::Shell { interactive: false }`. Marks the imperative region as started.

**Region-ordering rule (App. A.3 / §recipes.step-kinds).** The parser tracks `imperative_began: Option<usize>` — the line on which the first imperative step (`Shell`, `Lua`, `LuaBlock`) appeared. Once set, any subsequent declarative-region step — `ingredients`, `cook`, `plate`, `test`, module-call, `InlineLua`, `InlineLuaBlock` — is rejected with a diagnostic naming both lines (`region_violation`, `cli/crates/cook-lang/src/recipe.rs:287`).

#### 4. Chore Scope (`parse_chore()`, `cli/crates/cook-lang/src/recipe.rs:519`)

Almost identical to recipe scope, with three differences:

- `ingredients`, `cook`, `plate`, `test` are all banned and produce a tailored error from `chore_banned` (`cli/crates/cook-lang/src/recipe.rs:539`): "'cook' is not allowed in a chore; use 'recipe' for build outputs".
- Bare shell commands inside a chore body are pushed as `Step::Shell { interactive: true }` regardless of whether `@` is present — chores are default-interactive.
- The same region-ordering rule applies, but only `InlineLua` / `InlineLuaBlock` / module-call can violate it (the others are banned outright).

#### 5. Lua-Block Scope (`collect_lua_block()`, `cli/crates/cook-lang/src/lua_block.rs:12`)

Called when a `LuaBlockOpen` or `InlineLuaBlockOpen` is encountered (either as a standalone step or as the `using >{` clause of a `cook` line). This scope operates on **raw source lines**, not tokens, to preserve original formatting.

It starts reading from the source line immediately after `>{` (or `>>{`) and walks forward, calling `LuaScanner::scan_line()` on each raw line. A running `depth` starts at 1. When `depth` reaches 0 the closing line is found; that line is not included in the output. After collecting, `token_pos` is fast-forwarded past every token whose line is ≤ the closing brace's line.

#### 6. Shell-Block Scope (`collect_shell_block()` + `try_inline_shell_block()`, `cli/crates/cook-lang/src/shell_block.rs:63`, `:15`)

Used by `parse_body_payload` (`cli/crates/cook-lang/src/cook_line.rs:13`) when a `{` (plain shell block) opens a `cook` / `plate` / `test` body.

- `try_inline_shell_block(after_open)` is tried first: it walks the rest of the opening line character-by-character. If it finds a matching `}` on the same line, it returns the trimmed inner text as a single-element command list. Otherwise it returns `None`.
- If inline detection fails, `collect_shell_block()` walks subsequent source lines using `ShellScanner` (heredoc-aware). Each non-blank line becomes one element of `Vec<String>`; blank lines are dropped.

### Brace-Aware Scanning (`cli/crates/cook-lang/src/brace_scan.rs`)

`>{ … }` Lua blocks and `{ … }` shell blocks both terminate on a balanced `}`. Naïve per-line brace counting fails for constructs whose interiors contain `{` / `}` that should be treated as data: Lua long strings, Lua block comments, shell single-/double-quoted strings, and POSIX heredocs. Two stateful scanners handle this:

- **`LuaScanner`** (`cli/crates/cook-lang/src/brace_scan.rs:34`) tracks Lua **long brackets** (`[[ … ]]`, `[==[ … ]==]` at any `=`-level) for both long strings and `--[==[ … ]==]` block comments. Inside an open long bracket, braces are ignored. Line comments (`-- …`) skip the rest of the line. Single- and double-quoted strings are scanned with backslash-escape handling.
- **`ShellScanner`** (`cli/crates/cook-lang/src/brace_scan.rs:196`) tracks single- and double-quoted strings and POSIX heredocs (`<<TAG`, `<<-TAG`, with quoted-delimiter variants). Heredoc state carries across lines, so a `}` byte in a heredoc body is treated as data.

The two scanners are kept separate because Lua state is meaningless to shell text and vice versa. Module-call collection (`collect_module_call`, `cli/crates/cook-lang/src/recipe.rs:33`) reuses `LuaScanner` because a module call's body is syntactically a Lua expression.

### How `@` Is Handled

`@` is invisible to the lexer — a line starting with `@` lexes as `Content("@…")`. Inside `parse_recipe()`, the `Content` dispatch checks `text.strip_prefix('@')` (`cli/crates/cook-lang/src/recipe.rs:428`). If it matches and the remainder is non-empty, the `@` is stripped and `Step::Shell { interactive: true }` is created. An empty `@` (bare `@` with nothing after it) is a parse error: "interactive '@' prefix requires a command".

This design keeps the lexer simple and makes the interactive flag a parser-level semantic concern. Inside `parse_chore()` the `@` is accepted for symmetry but is a no-op because chore shell steps are always interactive.

### `parse_cook_line()` (`cli/crates/cook-lang/src/cook_line.rs:118`)

Handles the text after the `cook` keyword:

1. Read one or more quoted output patterns (at least one required).
2. If nothing remains, return `CookStep { outputs, using_clause: None }` — a declaration-only cook.
3. Otherwise the body opener (`{` / `>{`) must follow; a legacy `using` token gets the CS-0099 migration diagnostic. Dispatch the body via `parse_body_payload()`.

### `parse_body_payload()` (`cli/crates/cook-lang/src/cook_line.rs:13`)

Shared body parser for `cook using`, `plate`, and `test`. Examines the text after the introducer keyword:

- `>>{` → error: register-phase Lua block is not a valid step body; use `>{ … }`.
- `>{` → delegate to `collect_lua_block()`, return `Body::LuaBlock`.
- `{` → try `try_inline_shell_block()` (one-line form) first; fall back to `collect_shell_block()` (multi-line). Return `Body::ShellBlock`.
- `"` → error: the bare-string form `"cmd"` was removed in CS-0024; rewrite as `{ cmd }`.
- Anything else → error naming the keyword in context.
