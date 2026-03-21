# Cook/Plate Syntax Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace assignment-based metadata syntax (`ingredients = {…}`, `serves = "…"`, `requires = {…}`) with declarative keyword syntax (`ingredients`, `cook`, `plate`, `:` deps on recipe header).

**Architecture:** Parser rewrites the AST to carry deps, ingredients, and new Step variants (Cook, Plate). Codegen generates Lua loops for cook transforms with template placeholder expansion. Analyzer/Runtime/CLI read from updated AST fields.

**Tech Stack:** Rust, mlua 0.10, Lua 5.4, clap 4

**Spec:** `docs/superpowers/specs/2026-03-12-cook-serve-plate-syntax-design.md`

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `src/parser/ast.rs` | Rewrite | New AST types: Recipe with deps/ingredients, CookStep, PlateStep, UsingClause |
| `src/parser/lexer.rs` | Modify | RecipeHeader carries deps, parse `: "dep1" "dep2"` |
| `src/parser/mod.rs` | Rewrite | Keyword-based parsing for ingredients/cook/plate, remove old metadata parsing |
| `src/codegen/mod.rs` | Rewrite | Template placeholder expansion, cook/plate loop generation |
| `src/analyzer/mod.rs` | Modify | Read deps/ingredients/serves from new AST fields |
| `src/runtime/api.rs` | Modify | Remove ServedValue, simplify RegisteredMetadata |
| `src/runtime/mod.rs` | Modify | Update recipe context table (remove serves) |
| `src/cli/mod.rs` | Modify | `--plate` → `--emit-lua`, update menu/init |
| `src/watcher/mod.rs` | Modify | Read ingredients from new AST field |
| `tests/integration.rs` | Rewrite | All Cookfile strings use new syntax |
| `examples/Cookfile` | Rewrite | New syntax showcase |
| `README.md` | Modify | Update examples to new syntax |

## Generated Lua Reference

This is the contract between codegen and runtime. For the Cookfile:

```cookfile
recipe "lib": "setup"
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" using "gcc -c {in} -Iinclude -O2 -o {out}"
    cook "build/libmath.a" using "ar rcs {out} {all}"
end
```

The codegen produces:

```lua
cook.recipe("lib", {ingredients = {"lib/*.c", "include/*.h"}, requires = {"setup"}}, function()
    local _cook_outputs_1 = {}
    for _, _cook_in in ipairs(recipe.ingredients[1]) do
        local _cook_stem = path.stem(_cook_in)
        local _cook_name = path.name(_cook_in)
        local _cook_ext = path.ext(_cook_in)
        local _cook_dir = path.dir(_cook_in)
        local _cook_out = "build/obj/" .. _cook_stem .. ".o"
        cook.exec("gcc -c " .. _cook_in .. " -Iinclude -O2 -o " .. _cook_out, 4)
        table.insert(_cook_outputs_1, _cook_out)
    end
    local _cook_outputs_2 = {}
    local _cook_all = table.concat(_cook_outputs_1, " ")
    local _cook_out = "build/libmath.a"
    cook.exec("ar rcs " .. _cook_out .. " " .. _cook_all, 5)
    table.insert(_cook_outputs_2, _cook_out)
end)
```

For plate steps like `plate "./{out}"` after cook step 2:

```lua
for _, _plate_out in ipairs(_cook_outputs_2) do
    cook.exec("./" .. _plate_out, 6)
end
```

For Lua block using like `cook "build/obj/{stem}.o" using >{ ... }`:

```lua
local _cook_outputs_1 = {}
for _, _cook_in in ipairs(recipe.ingredients[1]) do
    local _cook_stem = path.stem(_cook_in)
    local _cook_name = path.name(_cook_in)
    local _cook_ext = path.ext(_cook_in)
    local _cook_dir = path.dir(_cook_in)
    local _cook_out = "build/obj/" .. _cook_stem .. ".o"
    local input = _cook_in
    local output = _cook_out
    local input_1 = recipe.ingredients[1]
    local input_2 = recipe.ingredients[2]
    -- user's lua code here --
    table.insert(_cook_outputs_1, _cook_out)
end
```

### Cook step mode detection

| Using clause | Contains `{in}` | Contains `{all}` | Mode |
|---|---|---|---|
| Shell | yes | no | 1-to-1 (loop per input) |
| Shell | no | yes | Many-to-one (single run) |
| Shell | no | no | Many-to-one (single run, no expansion) |
| LuaBlock | n/a | n/a | Always 1-to-1 |
| None | n/a | n/a | Declaration only (no code emitted) |

### Input chaining

- First cook step inputs: `recipe.ingredients[1]`
- Subsequent cook step inputs: `_cook_outputs_N` (previous step)
- `{all}` with no previous cook: `table.concat(recipe.ingredients[1], " ")`
- `{all}` with previous cook: `table.concat(_cook_outputs_N, " ")`

---

## Chunk 1: Parser Layer

### Task 1: AST Redesign

**Files:**
- Rewrite: `src/parser/ast.rs`

NOTE: After this task, the project will NOT compile until Tasks 2-7 are complete. This is expected for a syntax overhaul.

- [ ] **Step 1: Write new AST types and tests**

Replace the entire contents of `src/parser/ast.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Cookfile {
    pub recipes: Vec<Recipe>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub name: String,
    pub deps: Vec<String>,
    pub ingredients: Vec<String>,
    pub steps: Vec<Step>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UsingClause {
    Shell(String),
    LuaBlock(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CookStep {
    pub output_pattern: String,
    pub using_clause: Option<UsingClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlateStep {
    pub command: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Shell { command: String, line: usize },
    Lua { code: String, line: usize },
    LuaBlock { code: String, line: usize },
    Taste { line: usize },
    Cook { step: CookStep, line: usize },
    Plate { step: PlateStep, line: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recipe_construction() {
        let recipe = Recipe {
            name: "build".to_string(),
            deps: vec!["setup".to_string()],
            ingredients: vec!["src/*.c".to_string()],
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
                Step::Taste { line: 5 },
            ],
            line: 1,
        };
        assert_eq!(recipe.name, "build");
        assert_eq!(recipe.deps, vec!["setup"]);
        assert_eq!(recipe.steps.len(), 2);
    }

    #[test]
    fn test_recipe_no_metadata() {
        let recipe = Recipe {
            name: "clean".to_string(),
            deps: vec![],
            ingredients: vec![],
            steps: vec![Step::Shell {
                command: "rm -rf build".to_string(),
                line: 2,
            }],
            line: 1,
        };
        assert!(recipe.deps.is_empty());
        assert!(recipe.ingredients.is_empty());
    }

    #[test]
    fn test_cook_step_declaration_only() {
        let step = CookStep {
            output_pattern: "bin/app".to_string(),
            using_clause: None,
        };
        assert!(step.using_clause.is_none());
    }

    #[test]
    fn test_cook_step_lua_block() {
        let step = CookStep {
            output_pattern: "build/obj/{stem}.o".to_string(),
            using_clause: Some(UsingClause::LuaBlock(
                "cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)".to_string(),
            )),
        };
        assert!(matches!(step.using_clause, Some(UsingClause::LuaBlock(_))));
    }

    #[test]
    fn test_plate_step() {
        let step = PlateStep {
            command: "./{out}".to_string(),
        };
        assert_eq!(step.command, "./{out}");
    }
}
```

- [ ] **Step 2: Verify AST tests pass (module only)**

Run: `cargo test --lib parser::ast -- --nocapture 2>&1 | head -20`

This will fail because other modules reference the old AST. That's expected. The ast module's own tests should pass if we could compile it in isolation. We'll verify all AST tests pass after Task 3.

- [ ] **Step 3: Commit**

```bash
git add src/parser/ast.rs
git commit -m "refactor: replace RecipeMetadata with deps/ingredients/CookStep/PlateStep AST types"
```

---

### Task 2: Lexer — RecipeHeader with Dependencies

**Files:**
- Modify: `src/parser/lexer.rs`

- [ ] **Step 1: Update Token enum and tokenize function**

Change `RecipeHeader(String)` to carry deps. Update the recipe header parsing branch to extract `: "dep1" "dep2"` after the name.

In the Token enum, replace:
```rust
RecipeHeader(String),
```
with:
```rust
RecipeHeader { name: String, deps: Vec<String> },
```

In the `tokenize` function, replace the `RecipeHeader` branch (the `else if trimmed.starts_with("recipe")` block) with:

```rust
        } else if trimmed.starts_with("recipe")
            && trimmed.len() > 6
            && (trimmed.as_bytes()[6] == b' '
                || trimmed.as_bytes()[6] == b'\t'
                || trimmed.as_bytes()[6] == b'"')
        {
            let rest = trimmed["recipe".len()..].trim();
            if !rest.starts_with('"') {
                return Err(LexError::MissingRecipeName { line: line_num });
            }
            let rest = &rest[1..];
            let end = rest
                .find('"')
                .ok_or(LexError::UnterminatedString { line: line_num })?;
            let name = rest[..end].to_string();
            let after_name = rest[end + 1..].trim();

            // Parse optional deps: `: "dep1" "dep2"`
            let deps = if let Some(after_colon) = after_name.strip_prefix(':') {
                parse_quoted_strings(after_colon.trim(), line_num)?
            } else {
                vec![]
            };

            Token::RecipeHeader { name, deps }
```

Add this helper function before `tokenize` (or after, order doesn't matter in Rust):

```rust
fn parse_quoted_strings(text: &str, line: usize) -> Result<Vec<String>, LexError> {
    let mut result = Vec::new();
    let mut remaining = text.trim();
    while !remaining.is_empty() {
        if !remaining.starts_with('"') {
            return Err(LexError::UnterminatedString { line });
        }
        let rest = &remaining[1..];
        let end = rest
            .find('"')
            .ok_or(LexError::UnterminatedString { line })?;
        result.push(rest[..end].to_string());
        remaining = rest[end + 1..].trim();
    }
    Ok(result)
}
```

- [ ] **Step 2: Update lexer tests**

Replace all `Token::RecipeHeader("...".to_string())` patterns with `Token::RecipeHeader { name: "...".to_string(), deps: vec![] }`.

Add new tests for dep parsing:

```rust
    #[test]
    fn test_recipe_header_with_deps() {
        let tokens = tokenize(r#"recipe "build": "setup" "lib""#).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::RecipeHeader {
                name: "build".to_string(),
                deps: vec!["setup".to_string(), "lib".to_string()],
            }
        );
    }

    #[test]
    fn test_recipe_header_with_one_dep() {
        let tokens = tokenize(r#"recipe "build": "setup""#).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::RecipeHeader {
                name: "build".to_string(),
                deps: vec!["setup".to_string()],
            }
        );
    }

    #[test]
    fn test_recipe_header_no_deps() {
        let tokens = tokenize(r#"recipe "build""#).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::RecipeHeader {
                name: "build".to_string(),
                deps: vec![],
            }
        );
    }
```

Also update `test_multiline_source` to use the new pattern:
```rust
        assert_eq!(tokens[1].value, Token::RecipeHeader { name: "build".to_string(), deps: vec![] });
```

- [ ] **Step 3: Commit**

```bash
git add src/parser/lexer.rs
git commit -m "feat: lexer parses recipe header deps with : syntax"
```

---

### Task 3: Parser Rewrite — Keyword Parsing

**Files:**
- Rewrite: `src/parser/mod.rs`

- [ ] **Step 1: Update parse() to use new RecipeHeader**

In `parse()`, change the `Token::RecipeHeader(name)` match arm to:

```rust
            Token::RecipeHeader { name, deps } => {
                let recipe_line = tok.line;
                let name = name.clone();
                let deps = deps.clone();
                pos += 1;
                let (recipe, new_pos) =
                    parse_recipe(name, deps, recipe_line, &tokens, pos, &source_lines)?;
                recipes.push(recipe);
                pos = new_pos;
            }
```

- [ ] **Step 2: Rewrite parse_recipe with keyword parsing**

Replace `parse_recipe` entirely. The new version:
- Accepts `deps: Vec<String>` parameter
- Removes `metadata` and `in_steps` tracking
- Recognizes `ingredients`, `cook`, `plate` keyword prefixes on Content lines
- Handles `cook ... using >{` by collecting a Lua block

```rust
fn parse_recipe(
    name: String,
    deps: Vec<String>,
    recipe_line: usize,
    tokens: &[Located<Token>],
    start: usize,
    source_lines: &[&str],
) -> Result<(Recipe, usize), ParseError> {
    let mut pos = start;
    let mut ingredients = Vec::new();
    let mut steps = Vec::new();

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::RecipeEnd => {
                pos += 1;
                return Ok((
                    Recipe {
                        name,
                        deps,
                        ingredients,
                        steps,
                        line: recipe_line,
                    },
                    pos,
                ));
            }
            Token::Comment(_) | Token::Blank => {
                pos += 1;
            }
            Token::Content(text) => {
                if let Some(rest) = strip_keyword(text, "ingredients") {
                    if !ingredients.is_empty() {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message: "duplicate 'ingredients' line (use one line with multiple globs)".to_string(),
                        });
                    }
                    ingredients = parse_quoted_strings_parser(rest, tok.line)?;
                } else if let Some(rest) = strip_keyword(text, "cook") {
                    let (cook_step, new_pos) =
                        parse_cook_line(rest, tok.line, tokens, pos, source_lines)?;
                    steps.push(Step::Cook {
                        step: cook_step,
                        line: tok.line,
                    });
                    pos = new_pos;
                    continue;
                } else if let Some(rest) = strip_keyword(text, "plate") {
                    let command = parse_single_quoted_string(rest, tok.line)?;
                    steps.push(Step::Plate {
                        step: PlateStep { command },
                        line: tok.line,
                    });
                } else {
                    steps.push(Step::Shell {
                        command: text.clone(),
                        line: tok.line,
                    });
                }
                pos += 1;
            }
            Token::LuaLine(code) => {
                steps.push(Step::Lua {
                    code: code.clone(),
                    line: tok.line,
                });
                pos += 1;
            }
            Token::LuaBlockOpen => {
                let block_line = tok.line;
                pos += 1;
                let (code, new_pos) = collect_lua_block(block_line, tokens, pos, source_lines)?;
                steps.push(Step::LuaBlock {
                    code,
                    line: block_line,
                });
                pos = new_pos;
            }
            Token::Taste => {
                steps.push(Step::Taste { line: tok.line });
                pos += 1;
            }
            Token::RecipeHeader { .. } => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: format!("recipe '{}' was not closed with 'end'", name),
                });
            }
        }
    }

    Err(ParseError::Parse {
        line: recipe_line,
        message: format!("recipe '{}' was not closed with 'end'", name),
    })
}
```

- [ ] **Step 3: Add helper functions**

Add these helpers replacing the old `try_split_metadata`/`parse_metadata_value`/`parse_string_table` functions. Keep `collect_lua_block` and `count_brace_delta` as-is.

```rust
/// Checks if text starts with keyword followed by whitespace.
/// Returns the rest of the line after the keyword, or None.
fn strip_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    if text.starts_with(keyword) {
        let rest = &text[keyword.len()..];
        if rest.is_empty() {
            return Some(rest);
        }
        if rest.starts_with(' ') || rest.starts_with('\t') {
            return Some(rest.trim());
        }
    }
    None
}

/// Parses space-separated quoted strings: `"a" "b" "c"` → vec!["a", "b", "c"]
fn parse_quoted_strings_parser(text: &str, line: usize) -> Result<Vec<String>, ParseError> {
    let mut result = Vec::new();
    let mut remaining = text.trim();
    while !remaining.is_empty() {
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
        result.push(rest[..end].to_string());
        remaining = rest[end + 1..].trim();
    }
    Ok(result)
}

/// Parses a single quoted string: `"value"` → `value`
fn parse_single_quoted_string(text: &str, line: usize) -> Result<String, ParseError> {
    let text = text.trim();
    if !text.starts_with('"') {
        return Err(ParseError::Parse {
            line,
            message: format!("expected '\"', found: {}", text),
        });
    }
    let rest = &text[1..];
    let end = rest.find('"').ok_or(ParseError::Parse {
        line,
        message: "unterminated string".to_string(),
    })?;
    Ok(rest[..end].to_string())
}

/// Parses a cook line after the `cook` keyword prefix.
/// Handles: `"output"`, `"output" using "cmd"`, `"output" using >{...}`
fn parse_cook_line(
    rest: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(CookStep, usize), ParseError> {
    let rest = rest.trim();

    // Parse output pattern
    if !rest.starts_with('"') {
        return Err(ParseError::Parse {
            line,
            message: "cook: expected quoted output pattern".to_string(),
        });
    }
    let after_quote = &rest[1..];
    let end = after_quote.find('"').ok_or(ParseError::Parse {
        line,
        message: "cook: unterminated output pattern".to_string(),
    })?;
    let output_pattern = after_quote[..end].to_string();
    let after_pattern = after_quote[end + 1..].trim();

    // Check for `using` clause
    if after_pattern.is_empty() {
        // Declaration only: cook "output"
        return Ok((
            CookStep {
                output_pattern,
                using_clause: None,
            },
            current_pos + 1,
        ));
    }

    if !after_pattern.starts_with("using") {
        return Err(ParseError::Parse {
            line,
            message: format!("cook: expected 'using' after output pattern, found: {}", after_pattern),
        });
    }

    let after_using = after_pattern["using".len()..].trim();

    if after_using.starts_with(">{") {
        // Lua block using clause — collect block from source lines
        let (code, new_pos) = collect_lua_block(line, tokens, current_pos + 1, source_lines)?;
        Ok((
            CookStep {
                output_pattern,
                using_clause: Some(UsingClause::LuaBlock(code)),
            },
            new_pos,
        ))
    } else if after_using.starts_with('"') {
        // Shell using clause
        let cmd = parse_single_quoted_string(after_using, line)?;
        Ok((
            CookStep {
                output_pattern,
                using_clause: Some(UsingClause::Shell(cmd)),
            },
            current_pos + 1,
        ))
    } else {
        Err(ParseError::Parse {
            line,
            message: format!("cook using: expected quoted command or >{{ block, found: {}", after_using),
        })
    }
}
```

- [ ] **Step 4: Remove old helper functions**

Delete: `try_split_metadata`, `parse_metadata_value`, `parse_string_literal`, `parse_string_table`.

- [ ] **Step 5: Rewrite parser tests**

Replace all tests in the `mod tests` block. Key tests needed:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_cookfile() {
        let result = parse("").unwrap();
        assert_eq!(result.recipes.len(), 0);
    }

    #[test]
    fn test_minimal_recipe() {
        let source = r#"recipe "build"
    gcc -o main main.c
end
"#;
        let result = parse(source).unwrap();
        assert_eq!(result.recipes.len(), 1);
        assert_eq!(result.recipes[0].name, "build");
        assert!(result.recipes[0].deps.is_empty());
        assert!(result.recipes[0].ingredients.is_empty());
        assert_eq!(result.recipes[0].steps.len(), 1);
        assert_eq!(
            result.recipes[0].steps[0],
            Step::Shell {
                command: "gcc -o main main.c".to_string(),
                line: 2,
            }
        );
    }

    #[test]
    fn test_recipe_with_deps() {
        let source = r#"recipe "build": "setup" "lib"
    echo building
end
"#;
        let result = parse(source).unwrap();
        let recipe = &result.recipes[0];
        assert_eq!(recipe.deps, vec!["setup".to_string(), "lib".to_string()]);
    }

    #[test]
    fn test_recipe_with_ingredients() {
        let source = r#"recipe "lib"
    ingredients "lib/*.c" "include/*.h"
    echo compiling
end
"#;
        let result = parse(source).unwrap();
        let recipe = &result.recipes[0];
        assert_eq!(
            recipe.ingredients,
            vec!["lib/*.c".to_string(), "include/*.h".to_string()]
        );
    }

    #[test]
    fn test_duplicate_ingredients_error() {
        let source = r#"recipe "lib"
    ingredients "lib/*.c"
    ingredients "include/*.h"
end
"#;
        let result = parse(source);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("duplicate"), "error was: {}", msg);
    }

    #[test]
    fn test_cook_step_shell() {
        let source = r#"recipe "lib"
    ingredients "lib/*.c"
    cook "build/obj/{stem}.o" using "gcc -c {in} -o {out}"
end
"#;
        let result = parse(source).unwrap();
        let recipe = &result.recipes[0];
        assert_eq!(recipe.steps.len(), 1);
        match &recipe.steps[0] {
            Step::Cook { step, line } => {
                assert_eq!(*line, 3);
                assert_eq!(step.output_pattern, "build/obj/{stem}.o");
                assert_eq!(
                    step.using_clause,
                    Some(UsingClause::Shell("gcc -c {in} -o {out}".to_string()))
                );
            }
            other => panic!("expected Cook step, got {:?}", other),
        }
    }

    #[test]
    fn test_cook_step_many_to_one() {
        let source = r#"recipe "lib"
    ingredients "lib/*.c"
    cook "build/lib.a" using "ar rcs {out} {all}"
end
"#;
        let result = parse(source).unwrap();
        match &result.recipes[0].steps[0] {
            Step::Cook { step, .. } => {
                assert_eq!(step.output_pattern, "build/lib.a");
                assert_eq!(
                    step.using_clause,
                    Some(UsingClause::Shell("ar rcs {out} {all}".to_string()))
                );
            }
            other => panic!("expected Cook, got {:?}", other),
        }
    }

    #[test]
    fn test_cook_step_declaration_only() {
        let source = r#"recipe "build"
    ingredients "src/*.c"
    cook "bin/app"
    gcc src/main.c -o bin/app
end
"#;
        let result = parse(source).unwrap();
        let recipe = &result.recipes[0];
        assert_eq!(recipe.steps.len(), 2);
        match &recipe.steps[0] {
            Step::Cook { step, .. } => {
                assert_eq!(step.output_pattern, "bin/app");
                assert!(step.using_clause.is_none());
            }
            other => panic!("expected Cook, got {:?}", other),
        }
        assert!(matches!(&recipe.steps[1], Step::Shell { .. }));
    }

    #[test]
    fn test_cook_step_lua_block() {
        let source = r#"recipe "lib"
    ingredients "lib/*.c"
    cook "build/obj/{stem}.o" using >{
        cook.sh("gcc -c " .. input .. " -o " .. output)
    }
end
"#;
        let result = parse(source).unwrap();
        match &result.recipes[0].steps[0] {
            Step::Cook { step, .. } => {
                assert_eq!(step.output_pattern, "build/obj/{stem}.o");
                match &step.using_clause {
                    Some(UsingClause::LuaBlock(code)) => {
                        assert!(code.contains("cook.sh"), "code was: {}", code);
                    }
                    other => panic!("expected LuaBlock, got {:?}", other),
                }
            }
            other => panic!("expected Cook, got {:?}", other),
        }
    }

    #[test]
    fn test_plate_step() {
        let source = r#"recipe "test"
    ingredients "tests/*.c"
    cook "build/{stem}" using "cc {in} -o {out}"
    plate "./{out}"
end
"#;
        let result = parse(source).unwrap();
        let recipe = &result.recipes[0];
        assert_eq!(recipe.steps.len(), 2);
        match &recipe.steps[1] {
            Step::Plate { step, line } => {
                assert_eq!(*line, 4);
                assert_eq!(step.command, "./{out}");
            }
            other => panic!("expected Plate, got {:?}", other),
        }
    }

    #[test]
    fn test_mixed_steps() {
        let source = r#"recipe "lib": "setup"
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" using "gcc -c {in} -o {out}"
    > print("compiled")
    cook "build/libmath.a" using "ar rcs {out} {all}"
end
"#;
        let result = parse(source).unwrap();
        let recipe = &result.recipes[0];
        assert_eq!(recipe.deps, vec!["setup".to_string()]);
        assert_eq!(recipe.ingredients, vec!["lib/*.c".to_string(), "include/*.h".to_string()]);
        assert_eq!(recipe.steps.len(), 3);
        assert!(matches!(&recipe.steps[0], Step::Cook { .. }));
        assert!(matches!(&recipe.steps[1], Step::Lua { .. }));
        assert!(matches!(&recipe.steps[2], Step::Cook { .. }));
    }

    #[test]
    fn test_task_runner_no_metadata() {
        let source = r#"recipe "clean"
    rm -rf build bin
end
"#;
        let result = parse(source).unwrap();
        let recipe = &result.recipes[0];
        assert!(recipe.deps.is_empty());
        assert!(recipe.ingredients.is_empty());
        assert_eq!(recipe.steps.len(), 1);
    }

    #[test]
    fn test_multiple_recipes() {
        let source = r#"recipe "setup"
    mkdir -p build
end

recipe "build": "setup"
    echo building
end
"#;
        let result = parse(source).unwrap();
        assert_eq!(result.recipes.len(), 2);
        assert_eq!(result.recipes[0].name, "setup");
        assert_eq!(result.recipes[1].name, "build");
        assert_eq!(result.recipes[1].deps, vec!["setup".to_string()]);
    }

    #[test]
    fn test_unclosed_recipe() {
        let source = r#"recipe "build"
    gcc -o main main.c
"#;
        assert!(parse(source).is_err());
    }

    #[test]
    fn test_lua_block_in_recipe() {
        let source = r#"recipe "build"
>{
    local x = 1
    print(x)
}
end
"#;
        let result = parse(source).unwrap();
        assert_eq!(result.recipes[0].steps.len(), 1);
        assert!(matches!(&result.recipes[0].steps[0], Step::LuaBlock { .. }));
    }

    #[test]
    fn test_lua_block_nested_braces() {
        let source = r#"recipe "build"
>{
    if true then
        local t = {1, 2, 3}
    end
}
end
"#;
        let result = parse(source).unwrap();
        match &result.recipes[0].steps[0] {
            Step::LuaBlock { code, .. } => {
                assert!(code.contains("local t = {1, 2, 3}"));
            }
            other => panic!("expected LuaBlock, got {:?}", other),
        }
    }

    #[test]
    fn test_taste_step() {
        let source = r#"recipe "debug"
    taste
end
"#;
        let result = parse(source).unwrap();
        assert_eq!(result.recipes[0].steps[0], Step::Taste { line: 2 });
    }

    #[test]
    fn test_comments_and_blanks_skipped() {
        let source = r#"recipe "build"
    # comment
    gcc -o main main.c

end
"#;
        let result = parse(source).unwrap();
        assert_eq!(result.recipes[0].steps.len(), 1);
    }

    #[test]
    fn test_end_outside_recipe() {
        assert!(parse("end\n").is_err());
    }

    #[test]
    fn test_unclosed_lua_block() {
        let source = r#"recipe "build"
>{
    local x = 1
end
"#;
        assert!(parse(source).is_err());
    }

    #[test]
    fn test_lua_block_brace_in_string() {
        let source = r#"recipe "build"
>{
    local s = "}"
    print(s)
}
end
"#;
        let result = parse(source).unwrap();
        match &result.recipes[0].steps[0] {
            Step::LuaBlock { code, .. } => {
                assert!(code.contains(r#"local s = "}""#));
            }
            other => panic!("expected LuaBlock, got {:?}", other),
        }
    }

    #[test]
    fn test_lua_block_brace_in_comment() {
        let source = "recipe \"build\"\n>{\n    local x = 1 -- }\n    print(x)\n}\nend\n";
        let result = parse(source).unwrap();
        match &result.recipes[0].steps[0] {
            Step::LuaBlock { code, .. } => {
                assert!(code.contains("local x = 1 -- }"));
            }
            other => panic!("expected LuaBlock, got {:?}", other),
        }
    }

    #[test]
    fn test_strip_keyword() {
        assert_eq!(strip_keyword("ingredients \"a\"", "ingredients"), Some("\"a\""));
        assert_eq!(strip_keyword("cook \"x\"", "cook"), Some("\"x\""));
        assert_eq!(strip_keyword("plate \"x\"", "plate"), Some("\"x\""));
        assert_eq!(strip_keyword("cooking", "cook"), None);
        assert_eq!(strip_keyword("ingredient", "ingredients"), None);
    }
}
```

- [ ] **Step 6: Run parser tests**

Run: `cargo test --lib parser -- --nocapture`

Expected: All parser tests pass. Other modules will have compilation errors (expected).

- [ ] **Step 7: Commit**

```bash
git add src/parser/
git commit -m "feat: parser supports new keyword syntax (ingredients/cook/plate/deps)"
```

---

## Chunk 2: Codegen

### Task 4: Codegen Rewrite

**Files:**
- Rewrite: `src/codegen/mod.rs`

- [ ] **Step 1: Write template expansion helpers**

Add at the bottom of the file (before `#[cfg(test)]`):

```rust
fn escape_lua_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Expands a template like `"gcc -c {in} -o {out}"` into a Lua expression:
/// `"gcc -c " .. _cook_in .. " -o " .. _cook_out`
fn expand_template_to_lua(template: &str) -> String {
    let placeholder_map: &[(&str, &str)] = &[
        ("in", "_cook_in"),
        ("out", "_cook_out"),
        ("stem", "_cook_stem"),
        ("name", "_cook_name"),
        ("ext", "_cook_ext"),
        ("dir", "_cook_dir"),
        ("all", "_cook_all"),
    ];

    let mut parts = Vec::new();
    let mut remaining = template;

    while let Some(start) = remaining.find('{') {
        let prefix = &remaining[..start];
        if !prefix.is_empty() {
            parts.push(format!("\"{}\"", escape_lua_string(prefix)));
        }
        remaining = &remaining[start + 1..];
        let end = remaining.find('}').expect("unclosed placeholder in template");
        let name = &remaining[..end];
        let lua_var = placeholder_map
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
            .unwrap_or_else(|| format!("\"{{{}}}\"", name));
        parts.push(lua_var);
        remaining = &remaining[end + 1..];
    }

    if !remaining.is_empty() {
        parts.push(format!("\"{}\"", escape_lua_string(remaining)));
    }

    if parts.is_empty() {
        "\"\"".to_string()
    } else {
        parts.join(" .. ")
    }
}

/// Determines cook step mode from the using clause.
enum CookMode {
    OneToOne,
    ManyToOne,
    Declaration,
}

fn cook_step_mode(step: &CookStep) -> CookMode {
    match &step.using_clause {
        None => CookMode::Declaration,
        Some(UsingClause::LuaBlock(_)) => CookMode::OneToOne,
        Some(UsingClause::Shell(cmd)) => {
            if cmd.contains("{in}") {
                CookMode::OneToOne
            } else {
                CookMode::ManyToOne
            }
        }
    }
}
```

- [ ] **Step 2: Rewrite generate() and generate_metadata()**

Replace the entire `generate` and `generate_metadata` functions:

```rust
pub fn generate(cookfile: &Cookfile) -> String {
    let mut out = String::from("-- Generated by Cook\n");

    for recipe in &cookfile.recipes {
        out.push_str(&format!("cook.recipe(\"{}\", ", recipe.name));
        out.push_str(&generate_metadata(recipe));
        out.push_str(", function()\n");

        let mut cook_step_index = 0;
        let mut last_cook_index: Option<usize> = None;

        for step in &recipe.steps {
            match step {
                Step::Shell { command, line } => {
                    let wrapped = wrap_lua_string(command);
                    out.push_str(&format!("    cook.exec({}, {})\n", wrapped, line));
                }
                Step::Lua { code, .. } => {
                    out.push_str(&format!("    {}\n", code));
                }
                Step::LuaBlock { code, .. } => {
                    for code_line in code.lines() {
                        out.push_str(&format!("    {}\n", code_line));
                    }
                }
                Step::Taste { line } => {
                    out.push_str(&format!("    cook.taste({})\n", line));
                }
                Step::Cook { step: cook_step, line } => {
                    cook_step_index += 1;
                    generate_cook_step(
                        &mut out,
                        cook_step,
                        *line,
                        cook_step_index,
                        last_cook_index,
                        &recipe.ingredients,
                    );
                    last_cook_index = Some(cook_step_index);
                }
                Step::Plate { step: plate_step, line } => {
                    generate_plate_step(
                        &mut out,
                        plate_step,
                        *line,
                        last_cook_index,
                    );
                }
            }
        }

        out.push_str("end)\n\n");
    }

    out
}

fn generate_metadata(recipe: &Recipe) -> String {
    let mut fields = Vec::new();

    if !recipe.ingredients.is_empty() {
        let items: Vec<String> = recipe.ingredients.iter().map(|s| format!("\"{}\"", s)).collect();
        fields.push(format!("ingredients = {{{}}}", items.join(", ")));
    }

    if !recipe.deps.is_empty() {
        let items: Vec<String> = recipe.deps.iter().map(|s| format!("\"{}\"", s)).collect();
        fields.push(format!("requires = {{{}}}", items.join(", ")));
    }

    if fields.is_empty() {
        "{}".to_string()
    } else {
        format!("{{{}}}", fields.join(", "))
    }
}
```

- [ ] **Step 3: Write generate_cook_step()**

```rust
fn generate_cook_step(
    out: &mut String,
    cook_step: &CookStep,
    line: usize,
    index: usize,
    prev_cook_index: Option<usize>,
    ingredients: &[String],
) {
    let input_source = match prev_cook_index {
        Some(prev) => format!("_cook_outputs_{}", prev),
        None => "recipe.ingredients[1]".to_string(),
    };

    match cook_step_mode(cook_step) {
        CookMode::Declaration => {
            // Just track the declared output for chaining
            out.push_str(&format!(
                "    local _cook_outputs_{} = {{\"{}\"}}\n",
                index, cook_step.output_pattern
            ));
        }
        CookMode::OneToOne => {
            let output_expr = expand_template_to_lua(&cook_step.output_pattern);
            out.push_str(&format!("    local _cook_outputs_{} = {{}}\n", index));
            out.push_str(&format!(
                "    for _, _cook_in in ipairs({}) do\n",
                input_source
            ));
            out.push_str("        local _cook_stem = path.stem(_cook_in)\n");
            out.push_str("        local _cook_name = path.name(_cook_in)\n");
            out.push_str("        local _cook_ext = path.ext(_cook_in)\n");
            out.push_str("        local _cook_dir = path.dir(_cook_in)\n");
            out.push_str(&format!(
                "        local _cook_out = {}\n",
                output_expr
            ));

            match &cook_step.using_clause {
                Some(UsingClause::Shell(cmd)) => {
                    let cmd_expr = expand_template_to_lua(cmd);
                    out.push_str(&format!(
                        "        cook.exec({}, {})\n",
                        cmd_expr, line
                    ));
                }
                Some(UsingClause::LuaBlock(code)) => {
                    out.push_str("        local input = _cook_in\n");
                    out.push_str("        local output = _cook_out\n");
                    // Inject input_N arrays for ingredient group access
                    for (i, _) in ingredients.iter().enumerate() {
                        out.push_str(&format!(
                            "        local input_{} = recipe.ingredients[{}]\n",
                            i + 1,
                            i + 1
                        ));
                    }
                    for code_line in code.lines() {
                        out.push_str(&format!("        {}\n", code_line));
                    }
                }
                None => unreachable!(),
            }

            out.push_str(&format!(
                "        table.insert(_cook_outputs_{}, _cook_out)\n",
                index
            ));
            out.push_str("    end\n");
        }
        CookMode::ManyToOne => {
            let output_expr = expand_template_to_lua(&cook_step.output_pattern);
            out.push_str(&format!("    local _cook_outputs_{} = {{}}\n", index));
            out.push_str(&format!(
                "    local _cook_all = table.concat({}, \" \")\n",
                input_source
            ));
            out.push_str(&format!("    local _cook_out = {}\n", output_expr));

            if let Some(UsingClause::Shell(cmd)) = &cook_step.using_clause {
                let cmd_expr = expand_template_to_lua(cmd);
                out.push_str(&format!(
                    "    cook.exec({}, {})\n",
                    cmd_expr, line
                ));
            }

            out.push_str(&format!(
                "    table.insert(_cook_outputs_{}, _cook_out)\n",
                index
            ));
        }
    }
}
```

- [ ] **Step 4: Write generate_plate_step()**

```rust
fn generate_plate_step(
    out: &mut String,
    plate_step: &PlateStep,
    line: usize,
    last_cook_index: Option<usize>,
) {
    let source = match last_cook_index {
        Some(idx) => format!("_cook_outputs_{}", idx),
        None => "recipe.ingredients[1]".to_string(),
    };

    let cmd_template = plate_step.command.replace("{out}", "\" .. _plate_out .. \"");
    // Use proper template expansion for plate commands
    let cmd_expr = expand_template_to_lua(&plate_step.command.replace("{out}", "{_plate_out_}"));
    // Actually, plate only supports {out}, so do it simply:
    let cmd_expr = expand_plate_cmd(&plate_step.command);

    out.push_str(&format!(
        "    for _, _plate_out in ipairs({}) do\n",
        source
    ));
    out.push_str(&format!(
        "        cook.exec({}, {})\n",
        cmd_expr, line
    ));
    out.push_str("    end\n");
}

/// Expand plate command template. Only {out} is valid.
fn expand_plate_cmd(template: &str) -> String {
    let mut parts = Vec::new();
    let mut remaining = template;

    while let Some(start) = remaining.find("{out}") {
        let prefix = &remaining[..start];
        if !prefix.is_empty() {
            parts.push(format!("\"{}\"", escape_lua_string(prefix)));
        }
        parts.push("_plate_out".to_string());
        remaining = &remaining[start + 5..];
    }

    if !remaining.is_empty() {
        parts.push(format!("\"{}\"", escape_lua_string(remaining)));
    }

    if parts.is_empty() {
        "\"\"".to_string()
    } else {
        parts.join(" .. ")
    }
}
```

- [ ] **Step 5: Rewrite codegen tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_cookfile(recipes: Vec<Recipe>) -> Cookfile {
        Cookfile { recipes }
    }

    fn make_recipe(name: &str, deps: Vec<&str>, ingredients: Vec<&str>, steps: Vec<Step>) -> Recipe {
        Recipe {
            name: name.to_string(),
            deps: deps.into_iter().map(String::from).collect(),
            ingredients: ingredients.into_iter().map(String::from).collect(),
            steps,
            line: 1,
        }
    }

    #[test]
    fn test_expand_template_no_placeholders() {
        assert_eq!(expand_template_to_lua("echo hello"), "\"echo hello\"");
    }

    #[test]
    fn test_expand_template_single_placeholder() {
        assert_eq!(expand_template_to_lua("{in}"), "_cook_in");
    }

    #[test]
    fn test_expand_template_mixed() {
        let result = expand_template_to_lua("gcc -c {in} -o {out}");
        assert_eq!(result, "\"gcc -c \" .. _cook_in .. \" -o \" .. _cook_out");
    }

    #[test]
    fn test_expand_template_stem_in_path() {
        let result = expand_template_to_lua("build/obj/{stem}.o");
        assert_eq!(result, "\"build/obj/\" .. _cook_stem .. \".o\"");
    }

    #[test]
    fn test_expand_template_all() {
        let result = expand_template_to_lua("ar rcs {out} {all}");
        assert_eq!(result, "\"ar rcs \" .. _cook_out .. \" \" .. _cook_all");
    }

    #[test]
    fn test_minimal_recipe() {
        let cookfile = make_cookfile(vec![make_recipe(
            "build",
            vec![],
            vec![],
            vec![Step::Shell {
                command: "echo hello".to_string(),
                line: 2,
            }],
        )]);
        let output = generate(&cookfile);
        assert!(output.contains("cook.recipe(\"build\", {}, function()"));
        assert!(output.contains("cook.exec([[echo hello]], 2)"));
    }

    #[test]
    fn test_recipe_with_deps_and_ingredients() {
        let cookfile = make_cookfile(vec![make_recipe(
            "lib",
            vec!["setup"],
            vec!["lib/*.c", "include/*.h"],
            vec![],
        )]);
        let output = generate(&cookfile);
        assert!(output.contains("ingredients = {\"lib/*.c\", \"include/*.h\"}"));
        assert!(output.contains("requires = {\"setup\"}"));
    }

    #[test]
    fn test_cook_step_one_to_one() {
        let cookfile = make_cookfile(vec![make_recipe(
            "lib",
            vec![],
            vec!["lib/*.c"],
            vec![Step::Cook {
                step: CookStep {
                    output_pattern: "build/{stem}.o".to_string(),
                    using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".to_string())),
                },
                line: 3,
            }],
        )]);
        let output = generate(&cookfile);
        assert!(output.contains("for _, _cook_in in ipairs(recipe.ingredients[1])"));
        assert!(output.contains("local _cook_stem = path.stem(_cook_in)"));
        assert!(output.contains("local _cook_out = \"build/\" .. _cook_stem .. \".o\""));
        assert!(output.contains("cook.exec(\"gcc -c \" .. _cook_in .. \" -o \" .. _cook_out, 3)"));
        assert!(output.contains("table.insert(_cook_outputs_1, _cook_out)"));
    }

    #[test]
    fn test_cook_step_many_to_one() {
        let cookfile = make_cookfile(vec![make_recipe(
            "lib",
            vec![],
            vec!["lib/*.c"],
            vec![
                Step::Cook {
                    step: CookStep {
                        output_pattern: "build/{stem}.o".to_string(),
                        using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".to_string())),
                    },
                    line: 3,
                },
                Step::Cook {
                    step: CookStep {
                        output_pattern: "build/lib.a".to_string(),
                        using_clause: Some(UsingClause::Shell("ar rcs {out} {all}".to_string())),
                    },
                    line: 4,
                },
            ],
        )]);
        let output = generate(&cookfile);
        // Second cook step chains from first
        assert!(output.contains("local _cook_all = table.concat(_cook_outputs_1, \" \")"));
        assert!(output.contains("local _cook_out = \"build/lib.a\""));
        assert!(output.contains("table.insert(_cook_outputs_2, _cook_out)"));
    }

    #[test]
    fn test_cook_step_declaration() {
        let cookfile = make_cookfile(vec![make_recipe(
            "build",
            vec![],
            vec!["src/*.c"],
            vec![Step::Cook {
                step: CookStep {
                    output_pattern: "bin/app".to_string(),
                    using_clause: None,
                },
                line: 3,
            }],
        )]);
        let output = generate(&cookfile);
        assert!(output.contains("local _cook_outputs_1 = {\"bin/app\"}"));
        assert!(!output.contains("for _"));
    }

    #[test]
    fn test_cook_step_lua_block() {
        let cookfile = make_cookfile(vec![make_recipe(
            "lib",
            vec![],
            vec!["lib/*.c", "include/*.h"],
            vec![Step::Cook {
                step: CookStep {
                    output_pattern: "build/{stem}.o".to_string(),
                    using_clause: Some(UsingClause::LuaBlock(
                        "cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)".to_string(),
                    )),
                },
                line: 3,
            }],
        )]);
        let output = generate(&cookfile);
        assert!(output.contains("local input = _cook_in"));
        assert!(output.contains("local output = _cook_out"));
        assert!(output.contains("local input_1 = recipe.ingredients[1]"));
        assert!(output.contains("local input_2 = recipe.ingredients[2]"));
        assert!(output.contains("cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)"));
    }

    #[test]
    fn test_plate_step() {
        let cookfile = make_cookfile(vec![make_recipe(
            "test",
            vec![],
            vec!["tests/*.c"],
            vec![
                Step::Cook {
                    step: CookStep {
                        output_pattern: "build/{stem}".to_string(),
                        using_clause: Some(UsingClause::Shell("cc {in} -o {out}".to_string())),
                    },
                    line: 3,
                },
                Step::Plate {
                    step: PlateStep {
                        command: "./{out}".to_string(),
                    },
                    line: 4,
                },
            ],
        )]);
        let output = generate(&cookfile);
        assert!(output.contains("for _, _plate_out in ipairs(_cook_outputs_1)"));
        assert!(output.contains("\"./\" .. _plate_out"));
    }

    #[test]
    fn test_taste_emitted() {
        let cookfile = make_cookfile(vec![make_recipe(
            "test",
            vec![],
            vec![],
            vec![Step::Taste { line: 3 }],
        )]);
        let output = generate(&cookfile);
        assert!(output.contains("cook.taste(3)"));
    }

    #[test]
    fn test_lua_line_emitted() {
        let cookfile = make_cookfile(vec![make_recipe(
            "test",
            vec![],
            vec![],
            vec![Step::Lua {
                code: "print(\"hello\")".to_string(),
                line: 2,
            }],
        )]);
        let output = generate(&cookfile);
        assert!(output.contains("    print(\"hello\")"));
    }

    #[test]
    fn test_shell_with_double_brackets() {
        let cookfile = make_cookfile(vec![make_recipe(
            "test",
            vec![],
            vec![],
            vec![Step::Shell {
                command: "echo ]]".to_string(),
                line: 2,
            }],
        )]);
        let output = generate(&cookfile);
        assert!(output.contains("[=[echo ]]]=]"));
    }

    #[test]
    fn test_escape_lua_string() {
        assert_eq!(escape_lua_string("hello"), "hello");
        assert_eq!(escape_lua_string("he\"llo"), "he\\\"llo");
        assert_eq!(escape_lua_string("he\\llo"), "he\\\\llo");
    }
}
```

- [ ] **Step 6: Run codegen tests**

Run: `cargo test --lib codegen -- --nocapture`

Expected: All codegen tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/codegen/mod.rs
git commit -m "feat: codegen generates cook/plate loops with template expansion"
```

---

## Chunk 3: Downstream Consumers

### Task 5: Analyzer Updates

**Files:**
- Modify: `src/analyzer/mod.rs`

- [ ] **Step 1: Update build_recipe_info**

Replace `build_recipe_info`:

```rust
pub fn build_recipe_info(cookfile: &Cookfile) -> HashMap<String, RecipeInfo> {
    cookfile
        .recipes
        .iter()
        .map(|recipe| {
            // Extract serves from cook step output patterns
            let serves: Vec<String> = recipe
                .steps
                .iter()
                .filter_map(|step| {
                    if let Step::Cook { step: cook_step, .. } = step {
                        Some(cook_step.output_pattern.clone())
                    } else {
                        None
                    }
                })
                .collect();
            (
                recipe.name.clone(),
                RecipeInfo {
                    ingredients: recipe.ingredients.clone(),
                    serves,
                    requires: recipe.deps.clone(),
                },
            )
        })
        .collect()
}
```

Update the imports at the top:
```rust
use crate::parser::ast::*;
```

- [ ] **Step 2: Run analyzer tests**

Run: `cargo test --lib analyzer -- --nocapture`

Expected: All graph tests pass (they don't depend on AST). The `build_recipe_info` tests aren't separate unit tests — they'll be verified via integration tests.

- [ ] **Step 3: Commit**

```bash
git add src/analyzer/mod.rs
git commit -m "refactor: analyzer reads deps/serves from new AST fields"
```

---

### Task 6: Runtime Updates

**Files:**
- Modify: `src/runtime/api.rs`
- Modify: `src/runtime/mod.rs`

- [ ] **Step 1: Simplify RegisteredMetadata in api.rs**

Remove `ServedValue` enum. Simplify `RegisteredMetadata`:

```rust
/// Raw metadata stored at registration time, resolved at execution time.
#[derive(Debug)]
pub struct RegisteredMetadata {
    pub ingredients: Vec<String>,
}
```

In `register_cook_api`, update the `cook.recipe` registration function — remove serves extraction:

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

            recipes_clone.borrow_mut().push(RegisteredRecipe {
                name,
                function: key,
                metadata: RegisteredMetadata { ingredients },
            });
            Ok(())
        })?;
```

- [ ] **Step 2: Update execute_recipe in mod.rs**

Remove the `serves` section from the recipe context table builder. The section to remove is:

```rust
        // Set serves
        match &recipe.metadata.serves {
            // ... entire match block
        }
```

Just delete that block. The `recipe.name` and `recipe.ingredients` setup stays.

- [ ] **Step 3: Update runtime tests in mod.rs**

Remove these tests that reference serves:
- `test_recipe_context_serves_single`
- `test_recipe_context_serves_multiple`

Update `test_recipe_context_no_metadata` — remove the `recipe.serves == nil` assertion:

```rust
    #[test]
    fn test_recipe_context_no_metadata() {
        let dir = TempDir::new().unwrap();
        let rt = make_runtime(dir.path());
        let lua_src = r#"
cook.recipe("check", {}, function()
    assert(recipe.name == "check", "bad name")
    assert(#recipe.ingredients == 0, "expected empty ingredients")
end)
"#;
        let result = rt.execute_recipe(lua_src, "check");
        assert!(result.is_ok());
    }
```

All other runtime tests use `{}` for metadata and don't reference `serves`, so they remain unchanged.

- [ ] **Step 4: Run runtime tests**

Run: `cargo test --lib runtime -- --nocapture`

Expected: All runtime tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/runtime/
git commit -m "refactor: remove ServedValue, simplify runtime metadata"
```

---

### Task 7: CLI + Watcher Updates

**Files:**
- Modify: `src/cli/mod.rs`
- Modify: `src/watcher/mod.rs`

- [ ] **Step 1: Rename --plate to --emit-lua in cli.rs**

Replace the `plate` field in `Cli`:

```rust
    /// Print transpiled Lua instead of executing
    #[arg(long = "emit-lua", global = true)]
    pub emit_lua: bool,
```

In `cmd_run`, replace `cli.plate` with `cli.emit_lua`:

```rust
    if cli.emit_lua {
```

- [ ] **Step 2: Update cmd_menu in cli.rs**

Replace the recipe display loop in `cmd_menu`:

```rust
fn cmd_menu(cli: &Cli) -> Result<(), CookError> {
    let (cookfile, _) = read_and_parse(cli)?;

    for recipe in &cookfile.recipes {
        let mut desc = format!("  {}", recipe.name);
        if !recipe.ingredients.is_empty() {
            desc.push_str(&format!("  ingredients: {:?}", recipe.ingredients));
        }
        if !recipe.deps.is_empty() {
            desc.push_str(&format!("  deps: {:?}", recipe.deps));
        }
        // Show cook step outputs
        for step in &recipe.steps {
            if let crate::parser::ast::Step::Cook { step: cook_step, .. } = step {
                desc.push_str(&format!("  cook: {}", cook_step.output_pattern));
            }
        }
        println!("{desc}");
    }

    Ok(())
}
```

- [ ] **Step 3: Update cmd_init in cli.rs**

Update the starter Cookfile template:

```rust
fn cmd_init() -> Result<(), CookError> {
    let path = std::path::Path::new("Cookfile");
    if path.exists() {
        return Err(CookError::Other("Cookfile already exists".to_string()));
    }
    std::fs::write(
        path,
        r#"recipe "build"
    echo "Hello from Cook!"
end
"#,
    )
    .map_err(|e| CookError::Other(format!("failed to write Cookfile: {e}")))?;
    println!("Created Cookfile");
    Ok(())
}
```

- [ ] **Step 4: Update watcher.rs**

In `collect_globs_for_recipes`, replace `recipe.metadata.ingredients` with `recipe.ingredients`:

```rust
    pub fn collect_globs_for_recipes(
        cookfile: &crate::parser::ast::Cookfile,
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

- [ ] **Step 5: Verify full compilation**

Run: `cargo build 2>&1 | tail -20`

Expected: Project compiles with no errors. This is the first time since Task 1 that the full project builds.

- [ ] **Step 6: Run all unit tests**

Run: `cargo test --lib -- --nocapture 2>&1 | tail -30`

Expected: All unit tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/cli/mod.rs src/watcher/mod.rs
git commit -m "refactor: rename --plate to --emit-lua, update CLI menu/init and watcher"
```

---

## Chunk 4: Integration Tests + Examples + README

### Task 8: Integration Tests

**Files:**
- Rewrite: `tests/integration.rs`

- [ ] **Step 1: Rewrite all integration tests**

Replace the entire file:

```rust
use std::fs;
use std::process::Command;

fn cook_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cook"))
}

#[test]
fn test_cook_init_creates_cookfile() {
    let dir = tempfile::tempdir().unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("init")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("Cookfile").exists());
}

#[test]
fn test_cook_runs_default_recipe() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    echo "cooked!"
end"#,
    )
    .unwrap();
    let output = cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cook_runs_named_recipe() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "hello"
    echo "hello from cook"
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("hello")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "run hello failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cook_menu() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    echo hello
end

recipe "test": "build"
    echo testing
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("menu")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("build"), "menu missing 'build': {stdout}");
    assert!(stdout.contains("test"), "menu missing 'test': {stdout}");
}

#[test]
fn test_cook_emit_lua() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    echo hello
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("--emit-lua")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("cook.recipe"),
        "emit-lua missing cook.recipe: {stdout}"
    );
    assert!(
        stdout.contains("cook.exec"),
        "emit-lua missing cook.exec: {stdout}"
    );
}

#[test]
fn test_cook_dependency_resolution() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "setup"
    echo "setting up"
end

recipe "build": "setup"
    echo "building"
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("build")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "dep resolution failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cook_lua_integration() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    echo "hello"
    > local x = 42
    >{
        if x == 42 then
            print("lua works!")
        end
    }
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("build")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "lua integration failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cook_env_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".env"), "MY_VAR=hello_cook").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    >{
        if cook.env.MY_VAR ~= "hello_cook" then
            error("env not loaded")
        end
    }
    echo "env works"
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("build")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "env test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cook_nonexistent_recipe() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    echo hello
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("nonexistent")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn test_cook_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("Cookfile"), "recipe\nend").unwrap();
    let output = cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn test_cook_command_failure_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    false
end"#,
    )
    .unwrap();
    let output = cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn test_cook_no_cookfile() {
    let dir = tempfile::tempdir().unwrap();
    let output = cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(!output.status.success());
}

#[test]
fn test_cook_empty_cookfile_recipe_not_found() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("Cookfile"), "# empty").unwrap();
    let output = cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn test_cook_custom_file_flag() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("custom.cook"),
        r#"recipe "build"
    echo "custom file!"
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .args(["-f", "custom.cook"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "custom file failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cook_ingredients_keyword() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.txt"), "hello").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    ingredients "src/*.txt"
    >{
        assert(#recipe.ingredients == 1, "expected 1 group")
        assert(#recipe.ingredients[1] == 1, "expected 1 file, got " .. #recipe.ingredients[1])
    }
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("build")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "ingredients test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cook_step_one_to_one_transform() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join("out")).unwrap();
    fs::write(dir.path().join("src/a.txt"), "aaa").unwrap();
    fs::write(dir.path().join("src/b.txt"), "bbb").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    ingredients "src/*.txt"
    cook "out/{stem}.copy" using "cp {in} {out}"
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("build")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "1-to-1 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("out/a.copy").exists(), "out/a.copy missing");
    assert!(dir.path().join("out/b.copy").exists(), "out/b.copy missing");
}

#[test]
fn test_cook_step_many_to_one() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.txt"), "aaa").unwrap();
    fs::write(dir.path().join("src/b.txt"), "bbb").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    ingredients "src/*.txt"
    cook "out/all.txt" using "cat {all} > {out}"
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("build")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "many-to-one failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("out/all.txt").exists(), "out/all.txt missing");
}

#[test]
fn test_plate_step() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/hello.sh"), "#!/bin/sh\necho plate_works").unwrap();
    // Make executable
    std::process::Command::new("chmod")
        .args(["+x", &dir.path().join("src/hello.sh").to_string_lossy()])
        .output()
        .unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    ingredients "src/*.sh"
    cook "build/{name}" using "cp {in} {out} && chmod +x {out}"
    plate "./{out}"
end"#,
    )
    .unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .arg("build")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "plate test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("plate_works"),
        "plate output missing: {stdout}"
    );
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test integration -- --nocapture 2>&1 | tail -30`

Expected: All integration tests pass.

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: rewrite integration tests for new keyword syntax"
```

---

### Task 9: Examples + README

**Files:**
- Rewrite: `examples/Cookfile`
- Modify: `README.md`

- [ ] **Step 1: Rewrite examples/Cookfile**

```cookfile
# Cookfile for a C math library project

recipe "setup"
    mkdir -p build/obj bin
end

recipe "lib": "setup"
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" using "gcc -c {in} -Iinclude -Wall -Wextra -O2 -o {out}"
    cook "build/libmath.a" using "ar rcs {out} {all}"
end

recipe "build": "lib"
    ingredients "src/*.c"
    cook "bin/app"
    gcc src/main.c -Iinclude -Lbuild -lmath -lm -Wall -Wextra -O2 -o bin/app
end

recipe "test": "lib"
    ingredients "tests/test_*.c"
    cook "build/{stem}" using "gcc {in} -Iinclude -Lbuild -lmath -lm -Wall -Wextra -o {out}"
    plate "./{out}"
end

recipe "run": "build"
    ./bin/app
end

recipe "clean"
    rm -rf build bin
end
```

- [ ] **Step 2: Update README.md**

Update all Cookfile examples to use new syntax. Key sections:

**"What Does a Cookfile Look Like?" — Build system example:**
```cookfile
recipe "build": "setup"
    ingredients "src/*.c"
    cook "bin/app"
    gcc src/main.c -Iinclude -Lbuild -lmath -O2 -o bin/app
end
```

**"With embedded Lua" example:**
```cookfile
recipe "lib": "setup"
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" using >{
        local obj = path.join("build/obj", path.stem(input) .. ".o")
        cook.sh("gcc -c " .. input .. " -Iinclude -O2 -o " .. output)
    }
    cook "build/libmath.a" using "ar rcs {out} {all}"
end
```

**CLI section:** Change `cook --plate` to `cook --emit-lua`

**Makefile vs Cookfile comparison — Cookfile side:**
```cookfile
recipe "setup"
    mkdir -p build/obj bin
end

recipe "lib": "setup"
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" using >{
        local cc = cook.env.CC
        local cflags = cook.env.CFLAGS
        cook.sh(cc .. " " .. cflags .. " -Iinclude -c " .. input .. " -o " .. output)
    }
    cook "build/libmath.a" using "ar rcs {out} {all}"
end

recipe "build": "lib"
    ingredients "src/*.c"
    cook "bin/app"
    >{
        local cc = cook.env.CC
        local cflags = cook.env.CFLAGS
        cook.sh(cc .. " " .. cflags .. " src/main.c -Iinclude -Lbuild -lmath -lm -o " .. "bin/app")
    }
end

recipe "test": "lib"
    ingredients "tests/test_*.c"
    cook "build/{stem}" using >{
        local cc = cook.env.CC
        cook.sh(cc .. " " .. input .. " -Iinclude -Lbuild -lmath -lm -o " .. output)
    }
    plate "./{out}"
end

recipe "clean"
    rm -rf build bin
end
```

- [ ] **Step 3: Run full test suite**

Run: `cargo test -- --nocapture 2>&1 | tail -30`

Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add examples/Cookfile README.md
git commit -m "docs: update examples and README to new cook/plate syntax"
```

---

## Final Verification

After all tasks:

1. `cargo build` — compiles clean
2. `cargo test` — all unit + integration tests pass
3. `cargo run -- -f examples/Cookfile menu` — lists recipes with new syntax
4. `cargo run -- --emit-lua -f examples/Cookfile` — shows generated Lua

Then use superpowers:finishing-a-development-branch to complete the work.
