# Configuration System Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bare variables, named config blocks, `.env` overrides, CLI `--set` flag, positional config selection, and `{VAR}` template expansion to Cook.

**Architecture:** The parser learns two new constructs (bare vars and config blocks) that produce new AST fields. The CLI resolves all env sources into a single HashMap at startup. The codegen extends template expansion with a second pass that emits `cook.env.VAR` lookups for non-built-in `{VAR}` placeholders.

**Tech Stack:** Rust (edition 2024), clap, mlua, existing parser/codegen infrastructure.

**Spec:** `docs/superpowers/specs/2026-03-15-configuration-system-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/parser/ast.rs` | Modify | Add `vars` and `configs` fields to `Cookfile` |
| `src/parser/lexer.rs` | Modify | Add `ConfigHeader` and `VarDecl` token types |
| `src/parser/mod.rs` | Modify | Parse bare vars and config blocks before recipes |
| `src/cli/mod.rs` | Modify | Add `--set` global arg, positional config; `resolve_env()` pipeline |
| `src/codegen/mod.rs` | Modify | Two-pass template expansion for `{VAR}` → `cook.env.VAR` |
| `tests/integration.rs` | Modify | End-to-end tests for config system |

---

## Chunk 1: Parser and AST

### Task 1: AST — add vars and configs to Cookfile

**Files:**
- Modify: `src/parser/ast.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/parser/ast.rs`:

```rust
#[test]
fn test_cookfile_with_vars_and_configs() {
    let cookfile = Cookfile {
        vars: vec![
            ("CC".to_string(), "gcc".to_string()),
            ("CFLAGS".to_string(), "-Wall".to_string()),
        ],
        configs: {
            let mut m = std::collections::HashMap::new();
            m.insert("debug".to_string(), vec![
                ("CFLAGS".to_string(), "-g -O0 -Wall".to_string()),
            ]);
            m
        },
        recipes: vec![],
    };
    assert_eq!(cookfile.vars.len(), 2);
    assert_eq!(cookfile.configs.len(), 1);
    assert_eq!(cookfile.configs["debug"].len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_cookfile_with_vars_and_configs -- --nocapture`
Expected: FAIL — `Cookfile` has no `vars` or `configs` fields.

- [ ] **Step 3: Add vars and configs fields to Cookfile**

In `src/parser/ast.rs`, change the `Cookfile` struct:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Cookfile {
    pub vars: Vec<(String, String)>,
    pub configs: std::collections::HashMap<String, Vec<(String, String)>>,
    pub recipes: Vec<Recipe>,
}
```

- [ ] **Step 4: Fix all compilation errors**

Every place that constructs a `Cookfile` needs `vars: vec![]` and `configs: HashMap::new()`. Update these specific sites:

In `src/parser/mod.rs`, the `parse()` function's `Ok(Cookfile { recipes })` becomes:

```rust
Ok(Cookfile { vars: vec![], configs: std::collections::HashMap::new(), recipes })
```

In `src/codegen/mod.rs`, the `make_cookfile` test helper becomes:

```rust
fn make_cookfile(recipes: Vec<Recipe>) -> Cookfile {
    Cookfile { vars: vec![], configs: std::collections::HashMap::new(), recipes }
}
```

Search for any other `Cookfile {` constructors with `cargo check` and fix similarly.

Run: `cargo check`
Expected: compiles clean.

- [ ] **Step 5: Run all tests to verify nothing broke**

Run: `cargo test`
Expected: all tests pass (existing behavior unchanged).

- [ ] **Step 6: Commit**

```bash
git add src/parser/ast.rs src/parser/mod.rs src/codegen/mod.rs
git commit -m "feat(ast): add vars and configs fields to Cookfile"
```

---

### Task 2: Lexer — add VarDecl and ConfigHeader tokens

**Files:**
- Modify: `src/parser/lexer.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/parser/lexer.rs`:

```rust
#[test]
fn test_var_decl() {
    let tokens = tokenize(r#"CC "gcc""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::VarDecl {
            name: "CC".to_string(),
            value: "gcc".to_string(),
        }
    );
}

#[test]
fn test_var_decl_with_spaces_in_value() {
    let tokens = tokenize(r#"CFLAGS "-Wall -Wextra""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::VarDecl {
            name: "CFLAGS".to_string(),
            value: "-Wall -Wextra".to_string(),
        }
    );
}

#[test]
fn test_config_header() {
    let tokens = tokenize(r#"config "debug""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::ConfigHeader {
            name: "debug".to_string(),
        }
    );
}

#[test]
fn test_config_header_not_keyword_prefix() {
    // "configure" should be Content, not ConfigHeader
    let tokens = tokenize("configure").unwrap();
    assert_eq!(tokens[0].value, Token::Content("configure".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_var_decl test_config_header test_config_header_not_keyword -- --nocapture`
Expected: FAIL — no `VarDecl` or `ConfigHeader` variants.

- [ ] **Step 3: Add token variants and lexer logic**

In `src/parser/lexer.rs`, add to the `Token` enum:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Comment(String),
    RecipeHeader { name: String, deps: Vec<String> },
    RecipeEnd,
    ConfigHeader { name: String },
    VarDecl { name: String, value: String },
    LuaLine(String),
    LuaBlockOpen,
    Taste,
    Blank,
    Content(String),
}
```

In the `tokenize` function, add two new branches **before** the `Content` fallback (after the `recipe` check, before the final `else`):

```rust
} else if trimmed.starts_with("config")
    && trimmed.len() > 6
    && (trimmed.as_bytes()[6] == b' '
        || trimmed.as_bytes()[6] == b'\t'
        || trimmed.as_bytes()[6] == b'"')
{
    let rest = trimmed["config".len()..].trim();
    if !rest.starts_with('"') {
        return Err(LexError::MissingRecipeName { line: line_num });
    }
    let rest = &rest[1..];
    let end = rest
        .find('"')
        .ok_or(LexError::UnterminatedString { line: line_num })?;
    let name = rest[..end].to_string();
    Token::ConfigHeader { name }
} else if let Some(var) = try_parse_var_decl(trimmed) {
    Token::VarDecl { name: var.0, value: var.1 }
} else {
    Token::Content(trimmed.to_string())
}
```

Add the `try_parse_var_decl` helper function (before `tokenize`):

```rust
/// Try to parse a line as a variable declaration: `NAME "value"`
/// A var name is one or more non-whitespace, non-quote characters,
/// followed by whitespace, then a quoted string.
fn try_parse_var_decl(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let space_pos = line.find(|c: char| c == ' ' || c == '\t')?;
    let name = &line[..space_pos];

    // Var names must not be keywords
    if matches!(name, "recipe" | "config" | "end" | "ingredients" | "cook" | "plate" | "taste" | "using") {
        return None;
    }

    let rest = line[space_pos..].trim();
    if !rest.starts_with('"') {
        return None;
    }
    let inner = &rest[1..];
    let end = inner.find('"')?;
    // Make sure there's nothing after the closing quote
    let after = inner[end + 1..].trim();
    if !after.is_empty() {
        return None;
    }
    Some((name.to_string(), inner[..end].to_string()))
}
```

**IMPORTANT:** The `VarDecl` branch must come **after** `recipe` and `config` checks but **before** the `Content` fallback. The `try_parse_var_decl` function rejects keywords so that `recipe "build"` is never misinterpreted as a var.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: all tests pass including the new ones.

- [ ] **Step 5: Commit**

```bash
git add src/parser/lexer.rs
git commit -m "feat(lexer): add VarDecl and ConfigHeader token types"
```

---

### Task 3: Parser — parse bare vars and config blocks into AST

**Files:**
- Modify: `src/parser/mod.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/parser/mod.rs`:

```rust
#[test]
fn test_bare_vars_parsed() {
    let source = r#"CC "gcc"
CFLAGS "-Wall"

recipe "build"
    echo hello
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.vars.len(), 2);
    assert_eq!(result.vars[0], ("CC".to_string(), "gcc".to_string()));
    assert_eq!(result.vars[1], ("CFLAGS".to_string(), "-Wall".to_string()));
    assert_eq!(result.recipes.len(), 1);
}

#[test]
fn test_config_blocks_parsed() {
    let source = r#"config "debug"
    CFLAGS "-g -O0"
end

config "release"
    CFLAGS "-O2"
    LDFLAGS "-s"
end

recipe "build"
    echo hello
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.configs.len(), 2);
    assert_eq!(result.configs["debug"], vec![("CFLAGS".to_string(), "-g -O0".to_string())]);
    assert_eq!(result.configs["release"].len(), 2);
    assert_eq!(result.recipes.len(), 1);
}

#[test]
fn test_vars_and_configs_together() {
    let source = r#"CC "gcc"

config "debug"
    CFLAGS "-g"
end

recipe "build"
    echo hello
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.vars.len(), 1);
    assert_eq!(result.configs.len(), 1);
    assert_eq!(result.recipes.len(), 1);
}

#[test]
fn test_empty_config_block() {
    let source = r#"config "empty"
end

recipe "build"
    echo hello
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.configs["empty"], vec![]);
}

#[test]
fn test_var_after_recipe_is_shell_command() {
    let source = r#"recipe "build"
    CC "gcc"
end
"#;
    let result = parse(source).unwrap();
    // Inside a recipe, VarDecl tokens should be treated as shell commands
    assert_eq!(result.vars.len(), 0);
    assert_eq!(result.recipes.len(), 1);
    // Verify it became a Shell step
    assert!(matches!(
        &result.recipes[0].steps[0],
        Step::Shell { command, .. } if command.contains("CC")
    ));
}

#[test]
fn test_config_after_recipe_errors() {
    let source = r#"recipe "build"
    echo hello
end

config "debug"
    CFLAGS "-g"
end
"#;
    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("config blocks must appear before recipes"), "got: {err}");
}

#[test]
fn test_duplicate_config_name_last_wins() {
    let source = r#"config "debug"
    CFLAGS "-g"
end

config "debug"
    CFLAGS "-g3 -O0"
end

recipe "build"
    echo hello
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.configs["debug"], vec![("CFLAGS".to_string(), "-g3 -O0".to_string())]);
}

#[test]
fn test_unterminated_config_block_errors() {
    let source = r#"config "debug"
    CFLAGS "-g"
"#;
    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not closed"), "got: {err}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_bare_vars_parsed test_config_blocks_parsed test_vars_and_configs_together test_empty_config_block -- --nocapture`
Expected: FAIL.

- [ ] **Step 3: Update the parser**

In `src/parser/mod.rs`, modify the `parse()` function to handle `VarDecl` and `ConfigHeader` tokens at the top level before recipes:

```rust
pub fn parse(source: &str) -> Result<Cookfile, ParseError> {
    let tokens = tokenize(source)?;
    let source_lines: Vec<&str> = source.lines().collect();
    let mut pos = 0;
    let mut recipes = Vec::new();
    let mut vars = Vec::new();
    let mut configs = std::collections::HashMap::new();
    let mut seen_recipe = false;

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::Comment(_) | Token::Blank => {
                pos += 1;
            }
            Token::VarDecl { name, value } => {
                if seen_recipe {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "variable declarations must appear before recipes".to_string(),
                    });
                }
                vars.push((name.clone(), value.clone()));
                pos += 1;
            }
            Token::ConfigHeader { name } => {
                if seen_recipe {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "config blocks must appear before recipes".to_string(),
                    });
                }
                let config_name = name.clone();
                pos += 1;
                let (config_vars, new_pos) = parse_config_block(&tokens, pos, tok.line)?;
                configs.insert(config_name, config_vars);
                pos = new_pos;
            }
            Token::RecipeHeader { name, deps } => {
                seen_recipe = true;
                let recipe_line = tok.line;
                let name = name.clone();
                let deps = deps.clone();
                pos += 1;
                let (recipe, new_pos) =
                    parse_recipe(name, deps, recipe_line, &tokens, pos, &source_lines)?;
                recipes.push(recipe);
                pos = new_pos;
            }
            Token::RecipeEnd => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "unexpected 'end' outside of a recipe or config block".to_string(),
                });
            }
            Token::Content(_) => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "unexpected content outside of a recipe".to_string(),
                });
            }
            Token::LuaLine(_) | Token::LuaBlockOpen | Token::Taste => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "unexpected content outside of a recipe".to_string(),
                });
            }
        }
    }

    Ok(Cookfile { vars, configs, recipes })
}
```

Add the `parse_config_block` helper:

```rust
fn parse_config_block(
    tokens: &[Located<Token>],
    start: usize,
    open_line: usize,
) -> Result<(Vec<(String, String)>, usize), ParseError> {
    let mut pos = start;
    let mut config_vars = Vec::new();

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::RecipeEnd => {
                pos += 1;
                return Ok((config_vars, pos));
            }
            Token::VarDecl { name, value } => {
                config_vars.push((name.clone(), value.clone()));
                pos += 1;
            }
            Token::Comment(_) | Token::Blank => {
                pos += 1;
            }
            _ => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "config blocks may only contain variable declarations".to_string(),
                });
            }
        }
    }

    Err(ParseError::Parse {
        line: open_line,
        message: "config block was not closed with 'end'".to_string(),
    })
}
```

Also handle `VarDecl` and `ConfigHeader` inside `parse_recipe` — the lexer doesn't know context, so the parser must handle these token types for exhaustive matching. Add match arms in `parse_recipe`:

```rust
Token::VarDecl { name, value } => {
    // Inside a recipe, treat as shell command
    let command = format!("{} \"{}\"", name, value);
    steps.push(Step::Shell {
        command,
        line: tok.line,
    });
    pos += 1;
}
Token::ConfigHeader { .. } => {
    return Err(ParseError::Parse {
        line: tok.line,
        message: "unexpected config block inside recipe".to_string(),
    });
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/parser/mod.rs
git commit -m "feat(parser): parse bare vars and config blocks before recipes"
```

---

## Chunk 2: CLI and Env Resolution

### Task 4: CLI — add --set flag, positional config, resolve_env pipeline

**Files:**
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Add --set global arg and update Serve subcommand**

In `src/cli/mod.rs`, add to the `Cli` struct:

```rust
/// Override a variable (KEY=VALUE), repeatable. Must appear before recipe name.
#[arg(long = "set", global = true, num_args = 1)]
pub set: Vec<String>,
```

Update the `Serve` subcommand to accept an optional config positional:

```rust
Serve {
    /// Recipe to serve
    #[arg(default_value = "build")]
    recipe: String,
    /// Config preset to use
    config: Option<String>,
},
```

- [ ] **Step 2: Extract config from External args**

In the `run()` function, update the `External` match arm to extract both recipe and optional config:

```rust
Some(Command::External(args)) => {
    let recipe = args.first().map(|s| s.as_str()).unwrap_or("build");
    let config = args.get(1).map(|s| s.as_str());
    cmd_run(&cli, recipe, config)
}
None => cmd_run(&cli, "build", None),
```

Update the `Serve` arm similarly:

```rust
Some(Command::Serve { recipe, config }) => {
    let recipe = recipe.clone();
    cmd_serve(&cli, &recipe, config.as_deref())
}
```

Update `cmd_run` signature to `fn cmd_run(cli: &Cli, recipe_name: &str, config: Option<&str>) -> Result<(), CookError>`.

Update `cmd_serve` signature to `fn cmd_serve(cli: &Cli, recipe_name: &str, config: Option<&str>) -> Result<(), CookError>`.

Update **both** `cmd_run` calls inside `cmd_serve` to pass `config` through — the initial build (line ~296) and the one inside the watch closure (line ~304).

**Note:** `cmd_menu` does not use `load_env` and does not need a config parameter. No changes needed for `cmd_menu`.

- [ ] **Step 3: Add resolve_env function**

Add to `src/cli/mod.rs`:

```rust
fn resolve_env(
    cookfile: &crate::parser::ast::Cookfile,
    selected_config: Option<&str>,
    dotenv_vars: std::collections::HashMap<String, String>,
    cli_sets: &[String],
) -> Result<std::collections::HashMap<String, String>, CookError> {
    // Layer 1: system env
    let mut env: std::collections::HashMap<String, String> = std::env::vars().collect();

    // Layer 2: Cookfile bare vars
    for (k, v) in &cookfile.vars {
        env.insert(k.clone(), v.clone());
    }

    // Layer 3: selected config block
    if let Some(config_name) = selected_config {
        let config_vars = cookfile.configs.get(config_name).ok_or_else(|| {
            let available: Vec<&String> = cookfile.configs.keys().collect();
            if available.is_empty() {
                CookError::Other(format!("unknown config '{}': no configs defined", config_name))
            } else {
                CookError::Other(format!(
                    "unknown config '{}'. available: {}",
                    config_name,
                    available.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ))
            }
        })?;
        for (k, v) in config_vars {
            env.insert(k.clone(), v.clone());
        }
    }

    // Layer 4: .env file
    for (k, v) in dotenv_vars {
        env.insert(k, v);
    }

    // Layer 5: CLI --set (split on first '=')
    for set_arg in cli_sets {
        if let Some(eq_pos) = set_arg.find('=') {
            let key = set_arg[..eq_pos].to_string();
            let value = set_arg[eq_pos + 1..].to_string();
            env.insert(key, value);
        } else {
            return Err(CookError::Other(format!(
                "--set value must be KEY=VALUE, got: {}", set_arg
            )));
        }
    }

    Ok(env)
}
```

- [ ] **Step 4: Wire resolve_env into cmd_run**

In `cmd_run`, replace the existing `load_env` call. The resolved env flows to both `Runtime::new()` and `execute_dag()` for child process inheritance:

```rust
    let dotenv_vars = load_env(cookfile_dir);
    let env_vars = resolve_env(&cookfile, config, dotenv_vars, &cli.set)?;
```

This replaces the old `let env_vars = load_env(cookfile_dir);` line. The rest of `cmd_run` uses `env_vars` unchanged.

- [ ] **Step 5: Update override_usage string**

Update the `override_usage` in the `Cli` struct:

```rust
override_usage = "cook [OPTIONS] [RECIPE] [CONFIG]",
```

- [ ] **Step 6: Run all tests**

Run: `cargo test`
Expected: all existing tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): add --set flag, positional config, and resolve_env pipeline"
```

---

## Chunk 3: Codegen Template Expansion

### Task 5: Codegen — two-pass template expansion for {VAR}

**Files:**
- Modify: `src/codegen/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/codegen/mod.rs`:

```rust
#[test]
fn test_config_var_in_cook_step() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                output_pattern: "build/{stem}.o".to_string(),
                using_clause: Some(UsingClause::Shell(
                    "{CC} {CFLAGS} -c {in} -o {out}".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    // Built-in {in} and {out} should become _cook_in and _cook_out
    assert!(output.contains("_cook_in"), "should expand {{in}} to _cook_in");
    assert!(output.contains("_cook_out"), "should expand {{out}} to _cook_out");
    // Config vars {CC} and {CFLAGS} should become cook.env lookups
    assert!(
        output.contains("cook.env.CC"),
        "should expand {{CC}} to cook.env.CC, got: {}",
        output
    );
    assert!(
        output.contains("cook.env.CFLAGS"),
        "should expand {{CFLAGS}} to cook.env.CFLAGS, got: {}",
        output
    );
}

#[test]
fn test_config_var_only_template() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                output_pattern: "build/{stem}.o".to_string(),
                using_clause: Some(UsingClause::Shell(
                    "{CC} -c {in} -o {out}".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("cook.env.CC"));
    assert!(output.contains("_cook_in"));
    assert!(output.contains("_cook_out"));
}

#[test]
fn test_no_config_vars_unchanged() {
    // When no {VAR} config vars are present, output is unchanged
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                output_pattern: "build/{stem}.o".to_string(),
                using_clause: Some(UsingClause::Shell(
                    "gcc -c {in} -o {out}".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("\"gcc -c \""));
    assert!(!output.contains("cook.env"));
}

#[test]
fn test_config_var_exact_concatenation() {
    // Verify the exact Lua concatenation format matches the spec:
    // cook.env.CC .. " " .. cook.env.CFLAGS .. " -c " .. _cook_in .. " -o " .. _cook_out
    let result = expand_template_with_env_fallback(
        "{CC} {CFLAGS} -c {in} -o {out}",
        &[("{in}", "_cook_in"), ("{out}", "_cook_out")],
    );
    assert_eq!(
        result,
        r#"cook.env.CC .. " " .. cook.env.CFLAGS .. " -c " .. _cook_in .. " -o " .. _cook_out"#,
        "concatenation format must match spec"
    );
}

#[test]
fn test_unclosed_brace_treated_as_literal() {
    let result = expand_template_with_env_fallback(
        "hello {world",
        &[],
    );
    assert_eq!(result, r#""hello {world""#);
    assert!(!result.contains("cook.env"));
}

#[test]
fn test_empty_template() {
    let result = expand_template_with_env_fallback("", &[]);
    assert_eq!(result, r#""""#);
}
```

**Note:** `expand_template_with_env_fallback` must be `pub(crate)` (or at least `#[cfg(test)]`-accessible) for the direct unit tests. Alternatively, wrap the exact-format test through `generate()` and assert on the full output string.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_config_var_in_cook_step test_config_var_only_template test_no_config_vars_unchanged -- --nocapture`
Expected: FAIL — `{CC}` currently stays as a literal string.

- [ ] **Step 3: Implement two-pass template expansion**

Modify `expand_template_with_vars` in `src/codegen/mod.rs` to do a second pass over remaining `{...}` tokens. After the existing built-in expansion, scan the result for any remaining `{WORD}` patterns and replace with `cook.env.WORD`:

Replace `expand_template_to_lua`:

```rust
fn expand_template_to_lua(template: &str) -> String {
    let builtins = &[
        ("{in}", "_cook_in"),
        ("{out}", "_cook_out"),
        ("{stem}", "_cook_stem"),
        ("{name}", "_cook_name"),
        ("{ext}", "_cook_ext"),
        ("{dir}", "_cook_dir"),
        ("{all}", "_cook_all"),
    ];
    expand_template_with_env_fallback(template, builtins)
}
```

Replace `expand_output_pattern`:

```rust
fn expand_output_pattern(pattern: &str) -> String {
    let builtins = &[
        ("{stem}", "_cook_stem"),
        ("{name}", "_cook_name"),
        ("{ext}", "_cook_ext"),
        ("{dir}", "_cook_dir"),
        ("{in}", "_cook_in"),
    ];
    expand_template_with_env_fallback(pattern, builtins)
}
```

Replace `expand_plate_cmd`:

```rust
fn expand_plate_cmd(template: &str) -> String {
    let builtins = &[("{out}", "_plate_out")];
    expand_template_with_env_fallback(template, builtins)
}
```

Add the new `expand_template_with_env_fallback` function:

```rust
/// Two-pass template expansion:
/// 1. Expand built-in placeholders ({in}, {out}, etc.) to Lua variable names
/// 2. Expand remaining {VAR} tokens to cook.env.VAR lookups
fn expand_template_with_env_fallback(template: &str, builtins: &[(&str, &str)]) -> String {
    // Pass 1: expand built-in placeholders
    // We tokenize the template into segments: literals, builtins, and unknown {VAR}s
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = template;

    while !remaining.is_empty() {
        // Find the earliest { that could be a placeholder
        let brace_pos = remaining.find('{');

        match brace_pos {
            None => {
                // No more braces — rest is literal
                parts.push(format!("\"{}\"", escape_lua_string(remaining)));
                break;
            }
            Some(brace_start) => {
                // Emit any literal text before the brace
                if brace_start > 0 {
                    parts.push(format!(
                        "\"{}\"",
                        escape_lua_string(&remaining[..brace_start])
                    ));
                }

                // Find the closing brace
                let after_brace = &remaining[brace_start..];
                if let Some(close) = after_brace.find('}') {
                    let placeholder = &after_brace[..close + 1]; // e.g. "{CC}"
                    let inner = &after_brace[1..close]; // e.g. "CC"

                    // Check if it's a built-in
                    if let Some(&(_, var_name)) = builtins.iter().find(|&&(p, _)| p == placeholder)
                    {
                        parts.push(var_name.to_string());
                    } else {
                        // Not a built-in — emit cook.env.VAR lookup
                        parts.push(format!("cook.env.{}", inner));
                    }

                    remaining = &remaining[brace_start + close + 1..];
                } else {
                    // Unclosed brace — treat rest as literal
                    parts.push(format!(
                        "\"{}\"",
                        escape_lua_string(&remaining[brace_start..])
                    ));
                    break;
                }
            }
        }
    }

    if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    }
}
```

Keep the old `expand_template_with_vars` function around for now — existing callers (none in production, only the old functions which are now replaced) can be cleaned up, or leave it as a dead function to remove in a later cleanup.

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: all tests pass including the new config var tests. Existing template tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/codegen/mod.rs
git commit -m "feat(codegen): two-pass template expansion with cook.env.VAR fallback"
```

---

## Chunk 4: Integration Tests and Example Update

### Task 6: Integration tests and example Cookfile

**Files:**
- Modify: `tests/integration.rs`
- Modify: `examples/Cookfile`

- [ ] **Step 1: Write integration tests**

Add to `tests/integration.rs`. Use the existing `cook_cmd()` helper and `tempfile::tempdir()` pattern:

```rust
#[test]
fn test_bare_vars_in_using_clause() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"GREETING "hello-from-var"

recipe "build"
    ingredients "Cookfile"
    cook "output.txt" using "echo {GREETING} > {out}"
end
"#,
    )
    .unwrap();

    let output = cook_cmd()
        .current_dir(dir.path())
        .args(["--no-taste", "build"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let content = fs::read_to_string(dir.path().join("output.txt")).unwrap();
    assert!(content.contains("hello-from-var"), "got: {content}");
}

#[test]
fn test_config_selection_positional() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"MSG "default"

config "release"
    MSG "release-mode"
end

recipe "build"
    ingredients "Cookfile"
    cook "output.txt" using "echo {MSG} > {out}"
end
"#,
    )
    .unwrap();

    // Without config: uses bare var default
    let output = cook_cmd()
        .current_dir(dir.path())
        .args(["--no-taste", "build"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let content = fs::read_to_string(dir.path().join("output.txt")).unwrap();
    assert!(content.contains("default"), "got: {content}");

    // Clean output file
    let _ = fs::remove_file(dir.path().join("output.txt"));

    // With config as second positional: uses config block
    let output = cook_cmd()
        .current_dir(dir.path())
        .args(["--no-taste", "build", "release"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let content = fs::read_to_string(dir.path().join("output.txt")).unwrap();
    assert!(content.contains("release-mode"), "got: {content}");
}

#[test]
fn test_unknown_config_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"config "debug"
    CFLAGS "-g"
end

recipe "build"
    echo hello
end
"#,
    )
    .unwrap();

    let output = cook_cmd()
        .current_dir(dir.path())
        .args(["build", "nonexistent"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown config"), "stderr: {stderr}");
}

#[test]
fn test_set_override() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"MSG "original"

recipe "build"
    ingredients "Cookfile"
    cook "output.txt" using "echo {MSG} > {out}"
end
"#,
    )
    .unwrap();

    let output = cook_cmd()
        .current_dir(dir.path())
        .args(["--no-taste", "--set", "MSG=overridden", "build"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let content = fs::read_to_string(dir.path().join("output.txt")).unwrap();
    assert!(content.contains("overridden"), "got: {content}");
}

#[test]
fn test_config_plus_set_override() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"CC "gcc"
MSG "default"

config "release"
    MSG "release-mode"
end

recipe "build"
    ingredients "Cookfile"
    cook "output.txt" using "echo {CC} {MSG} > {out}"
end
"#,
    )
    .unwrap();

    // --set overrides even the config block value
    let output = cook_cmd()
        .current_dir(dir.path())
        .args(["--no-taste", "--set", "CC=clang", "build", "release"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let content = fs::read_to_string(dir.path().join("output.txt")).unwrap();
    assert!(content.contains("clang"), "CC should be overridden, got: {content}");
    assert!(content.contains("release-mode"), "MSG should come from config, got: {content}");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --test integration test_bare_vars test_config_selection test_unknown_config test_set_override test_config_plus -- --nocapture`
Expected: FAIL — features not yet wired up.

- [ ] **Step 4: Update examples/Cookfile**

Update the example Cookfile to use bare vars and config blocks:

```
CC "gcc"
AR "ar"
CFLAGS "-Wall -Wextra"

config "debug"
    CFLAGS "-g -O0 -Wall -Wextra"
end

config "release"
    CFLAGS "-O2 -DNDEBUG -Wall -Wextra"
    LDFLAGS "-s"
end

recipe "setup"
    mkdir -p build/obj bin
end

recipe "lib": "setup"
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" using "{CC} {CFLAGS} -Iinclude -c {in} -o {out}"
    cook "build/libmath.a" using "{AR} rcs {out} {all}"
end

recipe "build": "lib"
    ingredients "src/*.c"
    cook "bin/app" using "{CC} {CFLAGS} {all} -Iinclude -Lbuild -lmath -lm -o {out}"
end

recipe "test": "lib"
    ingredients "tests/test_*.c"
    cook "build/{stem}" using "{CC} {in} -Iinclude -Lbuild -lmath -lm {CFLAGS} -o {out}"
    plate "./{out}"
end

recipe "run": "build"
    ./bin/app
end

recipe "clean"
    rm -rf build bin
end

recipe "all": "test" "build"
end
```

- [ ] **Step 5: Test the example end-to-end**

Run:
```bash
cargo run -- -f examples/Cookfile clean
cargo run -- -f examples/Cookfile --no-taste all
cargo run -- -f examples/Cookfile --no-taste all release
cargo run -- -f examples/Cookfile --no-taste all debug
```
Expected: all succeed.

- [ ] **Step 6: Run full test suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add tests/integration.rs examples/Cookfile
git commit -m "feat: add integration tests and update example with config system"
```

---
