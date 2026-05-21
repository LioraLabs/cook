use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Comment(String),
    RecipeHeader { name: String, deps: Vec<String> },
    ChoreHeader { name: String, params: Vec<crate::ast::ChoreParam>, deps: Vec<String> },
    ConfigHeader { name: Option<String> },
    UseDecl { name: String },
    ImportDecl { name: String, path: String },
    RegisterHeader,
    LuaLine(String),
    LuaBlockOpen,
    InlineLuaLine(String),
    InlineLuaBlockOpen,
    Blank,
    Content(String),
}

#[derive(Debug, Clone)]
pub struct Located<T> {
    pub value: T,
    pub line: usize,
}

#[derive(Error, Debug)]
pub enum LexError {
    #[error("line {line}: unterminated string")]
    UnterminatedString { line: usize },
    #[error("line {line}: expected quoted name after keyword")]
    MissingRecipeName { line: usize },
    #[error("line {line}: '{segment}' is a reserved word and cannot be used in this position in a dotted recipe name")]
    ReservedRecipeName { line: usize, segment: String },
    #[error("line {line}: a run of three or more `>` characters at line start is reserved (§{{lexical.line-prefixes}})")]
    ReservedTripleArrow { line: usize },
    #[error("line {line}: 'use' name '{name}' is not a valid Lua identifier (must match /^[A-Za-z_][A-Za-z0-9_]*$/; '-' and '.' are not permitted)")]
    InvalidUseName { name: String, line: usize },
    #[error("line {line}: recipe name '{name}': dotted recipe names are not permitted at the declaration site; use 'import alias path' for cross-Cookfile namespacing")]
    DottedDeclaredRecipeName { name: String, line: usize },
    #[error("line {line}: chore name '{name}': dotted chore names are not permitted at the declaration site; use 'import alias path' for cross-Cookfile namespacing")]
    DottedDeclaredChoreName { name: String, line: usize },
    #[error("line {line}: chore '{chore}': duplicate parameter '{name}'")]
    DuplicateChoreParam { line: usize, chore: String, name: String },
    #[error("line {line}: chore '{chore}': parameter '{name}' uses reserved identifier")]
    ReservedChoreParam { line: usize, chore: String, name: String },
    #[error("line {line}: chore '{chore}': required parameter '{required}' must precede defaulted parameter '{defaulted}'")]
    RequiredAfterDefaulted { line: usize, chore: String, required: String, defaulted: String },
    #[error("line {line}: chore '{chore}': default for parameter '{name}' must be a quoted string or a parenthesised Lua expression")]
    BadChoreParamDefault { line: usize, chore: String, name: String },
    #[error("line {line}: chore '{chore}': unclosed default for parameter '{name}' (expected closing '\"' or ')')")]
    UnclosedChoreParamDefault { line: usize, chore: String, name: String },
    #[error("line {line}: recipe '{name}': recipes don't take parameters; use a 'chore' (§7) or a config preset (§5)")]
    RecipeWithParams { line: usize, name: String },
}

const RESERVED_RECIPE_SEGMENTS: &[&str] = &["stem", "name", "ext", "dir", "in", "out", "all", "env"];

fn check_reserved_recipe_name(name: &str, line: usize) -> Result<(), LexError> {
    let first_segment = name.split('.').next().unwrap_or(name);
    if first_segment == "env" {
        return Err(LexError::ReservedRecipeName {
            line,
            segment: "env".to_string(),
        });
    }
    let last_segment = name.rsplit('.').next().unwrap_or(name);
    if RESERVED_RECIPE_SEGMENTS.contains(&last_segment) {
        return Err(LexError::ReservedRecipeName {
            line,
            segment: last_segment.to_string(),
        });
    }
    Ok(())
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}

/// Validate that a `use NAME` argument is a Lua identifier — `^[A-Za-z_][A-Za-z0-9_]*$`.
/// Per CS-0035, `use` names are dropped verbatim into a `local NAME = ...` Lua binding
/// by the codegen layer, so they MUST be syntactically valid Lua identifiers; otherwise
/// the generated Lua is malformed and the failure surfaces far from the source. Hyphens
/// and dots are rejected outright; in particular, hyphen rejection eliminates the
/// `foo-bar` / `foo_bar` collision under the codegen's `replace('-', "_")` workaround.
fn check_use_name(name: &str, line: usize) -> Result<(), LexError> {
    let mut chars = name.chars();
    let ok_start = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
    let ok_rest = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok_start || !ok_rest || name.is_empty() {
        return Err(LexError::InvalidUseName { name: name.to_string(), line });
    }
    Ok(())
}

/// Parse either a quoted name (`"foo"`) or a bare identifier (`foo`, `backend.build`).
/// Returns `(name, remaining_text)`.
fn parse_name(text: &str, line: usize) -> Result<(String, &str), LexError> {
    let text = text.trim_start();
    if text.starts_with('"') {
        let rest = &text[1..];
        let end = rest
            .find('"')
            .ok_or(LexError::UnterminatedString { line })?;
        Ok((rest[..end].to_string(), rest[end + 1..].trim_start()))
    } else {
        let end = text
            .find(|c: char| !is_ident_char(c))
            .unwrap_or(text.len());
        if end == 0 || !is_ident_start(text.as_bytes()[0] as char) {
            return Err(LexError::MissingRecipeName { line });
        }
        Ok((text[..end].to_string(), text[end..].trim_start()))
    }
}

/// Parse the chore parameter list from `text`.
///
/// Reads bare-identifier params with optional `="STRING"` defaults, enforces
/// required-before-defaulted ordering, duplicate-name rejection, and
/// reserved-name rejection. Stops at `:` or end-of-input.
///
/// For `=(...)` (Lua-expression default), rejects with `BadChoreParamDefault`
/// for now. Task 6 will support it.
///
/// Returns `(params, remaining_text)`.
fn parse_chore_params<'a>(
    text: &'a str,
    chore_name: &str,
    line: usize,
) -> Result<(Vec<crate::ast::ChoreParam>, &'a str), LexError> {
    use crate::ast::ChoreParam;

    let mut params: Vec<ChoreParam> = Vec::new();
    let mut seen_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut seen_defaulted = false;
    let mut remaining = text.trim_start();

    loop {
        // Stop if we're at a `:` (dep list) or end of input.
        if remaining.is_empty() || remaining.starts_with(':') {
            break;
        }

        // Must start with an ident-start character; otherwise stop (unknown token).
        if !is_ident_start(remaining.as_bytes()[0] as char) {
            break;
        }

        // Parse bare identifier (param name). Param names use only [A-Za-z0-9_];
        // dots and hyphens are NOT allowed in param names.
        let end = remaining
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(remaining.len());
        let param_name = remaining[..end].to_string();
        remaining = remaining[end..].trim_start();

        // Reserved-name check.
        if RESERVED_RECIPE_SEGMENTS.contains(&param_name.as_str()) {
            return Err(LexError::ReservedChoreParam {
                line,
                chore: chore_name.to_string(),
                name: param_name,
            });
        }

        // Duplicate-name check.
        if !seen_names.insert(param_name.clone()) {
            return Err(LexError::DuplicateChoreParam {
                line,
                chore: chore_name.to_string(),
                name: param_name,
            });
        }

        // Check for a default value.
        if remaining.starts_with('=') {
            let after_eq = &remaining[1..];
            if after_eq.starts_with('"') {
                // String default: parse up to closing `"`.
                let inner = &after_eq[1..];
                let close = inner
                    .find('"')
                    .ok_or_else(|| LexError::UnclosedChoreParamDefault {
                        line,
                        chore: chore_name.to_string(),
                        name: param_name.clone(),
                    })?;
                let default_val = inner[..close].to_string();
                remaining = inner[close + 1..].trim_start();

                params.push(ChoreParam::DefaultedString {
                    name: param_name,
                    default: default_val,
                    line,
                    col: 0,
                });
                seen_defaulted = true;
            } else if after_eq.starts_with('(') {
                // Lua-expression default — not yet supported (Task 6).
                return Err(LexError::BadChoreParamDefault {
                    line,
                    chore: chore_name.to_string(),
                    name: param_name,
                });
            } else {
                return Err(LexError::BadChoreParamDefault {
                    line,
                    chore: chore_name.to_string(),
                    name: param_name,
                });
            }
        } else {
            // Required param — must precede any defaulted param.
            if seen_defaulted {
                // Find the name of the most-recently-added defaulted param.
                let defaulted_name = params
                    .iter()
                    .rev()
                    .find_map(|p| {
                        if let ChoreParam::DefaultedString { name, .. } = p {
                            Some(name.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();
                return Err(LexError::RequiredAfterDefaulted {
                    line,
                    chore: chore_name.to_string(),
                    required: param_name,
                    defaulted: defaulted_name,
                });
            }
            params.push(ChoreParam::Required { name: param_name, line, col: 0 });
        }
    }

    Ok((params, remaining))
}

/// Parse a space-separated list of names (quoted or bare).
fn parse_names(text: &str, line: usize) -> Result<Vec<String>, LexError> {
    let mut result = Vec::new();
    let mut remaining = text.trim();
    while !remaining.is_empty() {
        let (name, rest) = parse_name(remaining, line)?;
        result.push(name);
        remaining = rest;
    }
    Ok(result)
}

pub fn tokenize(source: &str) -> Result<Vec<Located<Token>>, LexError> {
    let mut tokens = Vec::new();

    for (idx, line) in source.lines().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        let token = if trimmed.is_empty() {
            Token::Blank
        } else if trimmed.starts_with('#') {
            Token::Comment(trimmed[1..].to_string())
        } else if trimmed.starts_with(">>>") {
            // Reserved for future prefixes; reject explicitly so a four-arrow
            // line is a sharp diagnostic rather than a confusing parse error
            // further down (§{lexical.line-prefixes}).
            return Err(LexError::ReservedTripleArrow { line: line_num });
        } else if trimmed.starts_with(">>{") {
            Token::InlineLuaBlockOpen
        } else if trimmed.starts_with(">>") {
            let code = &trimmed[2..];
            let code = code.strip_prefix(' ').unwrap_or(code);
            Token::InlineLuaLine(code.to_string())
        } else if trimmed.starts_with(">{") {
            Token::LuaBlockOpen
        } else if trimmed.starts_with('>') {
            let code = &trimmed[1..];
            let code = code.strip_prefix(' ').unwrap_or(code);
            Token::LuaLine(code.to_string())
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("recipe")
            && trimmed.len() > 6
            && (trimmed.as_bytes()[6] == b' '
                || trimmed.as_bytes()[6] == b'\t'
                || trimmed.as_bytes()[6] == b'"')
        {
            let rest = trimmed["recipe".len()..].trim();
            let (name, after_name) = parse_name(rest, line_num)?;
            // Reserved-segment check first so the existing, more-specific
            // diagnostics (e.g. "'env' is a reserved word") still fire for
            // names like `env.foo` instead of being shadowed by the generic
            // dotted-name rejection below.
            check_reserved_recipe_name(&name, line_num)?;
            if name.contains('.') {
                return Err(LexError::DottedDeclaredRecipeName { name, line: line_num });
            }

            // Recipes don't take parameters. Reject any token between the name
            // and the `:` (or end of header) that is not a dep list.
            let after_name_trimmed = after_name.trim_start();
            if !after_name_trimmed.is_empty() && !after_name_trimmed.starts_with(':') {
                return Err(LexError::RecipeWithParams { line: line_num, name });
            }

            let deps = if let Some(after_colon) = after_name.strip_prefix(':') {
                parse_names(after_colon.trim(), line_num)?
            } else {
                vec![]
            };

            Token::RecipeHeader { name, deps }
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("chore")
            && trimmed.len() > 5
            && (trimmed.as_bytes()[5] == b' '
                || trimmed.as_bytes()[5] == b'\t'
                || trimmed.as_bytes()[5] == b'"')
        {
            let rest = trimmed["chore".len()..].trim();
            let (name, after_name) = parse_name(rest, line_num)?;
            check_reserved_recipe_name(&name, line_num)?;
            if name.contains('.') {
                return Err(LexError::DottedDeclaredChoreName { name, line: line_num });
            }

            let (params, after_params) = parse_chore_params(after_name, &name, line_num)?;
            let deps = if let Some(after_colon) = after_params.strip_prefix(':') {
                parse_names(after_colon.trim(), line_num)?
            } else {
                vec![]
            };

            Token::ChoreHeader { name, params, deps }
        } else if !line.starts_with(|c: char| c.is_whitespace()) && trimmed == "config" {
            Token::ConfigHeader { name: None }
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("config")
            && trimmed.len() > 6
            && (trimmed.as_bytes()[6] == b' '
                || trimmed.as_bytes()[6] == b'\t'
                || trimmed.as_bytes()[6] == b'"')
        {
            let rest = trimmed["config".len()..].trim();
            let (name, _) = parse_name(rest, line_num)?;
            Token::ConfigHeader { name: Some(name) }
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("register")
            && (trimmed.len() == 8
                || (trimmed.len() > 8
                    && (trimmed.as_bytes()[8] == b' ' || trimmed.as_bytes()[8] == b'\t')))
        {
            Token::RegisterHeader
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("use")
            && trimmed.len() > 3
            && (trimmed.as_bytes()[3] == b' '
                || trimmed.as_bytes()[3] == b'\t'
                || trimmed.as_bytes()[3] == b'"')
        {
            let rest = trimmed["use".len()..].trim();
            let (name, _) = parse_name(rest, line_num)?;
            check_use_name(&name, line_num)?;
            Token::UseDecl { name }
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("import")
            && trimmed.len() > 6
            && (trimmed.as_bytes()[6] == b' ' || trimmed.as_bytes()[6] == b'\t')
        {
            let rest = trimmed["import".len()..].trim();
            let space_pos = rest.find(|c: char| c == ' ' || c == '\t');
            match space_pos {
                Some(pos) => {
                    let name = rest[..pos].to_string();
                    let path = rest[pos..].trim().to_string();
                    if path.is_empty() {
                        return Err(LexError::MissingRecipeName { line: line_num });
                    }
                    Token::ImportDecl { name, path }
                }
                None => {
                    return Err(LexError::MissingRecipeName { line: line_num });
                }
            }
        } else {
            // Anything else: a Content line. Whether it dispatches inside a
            // recipe body (shell_command, interactive_command, module_call,
            // ingredients_step, etc.) or is rejected at top level is the
            // syntactic-layer's concern (§{grammar.overview}, §{grammar.step-dispatch}).
            Token::Content(trimmed.to_string())
        };

        tokens.push(Located {
            value: token,
            line: line_num,
        });
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_comment_line() {
        let tokens = tokenize("# this is a comment").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::Comment(" this is a comment".to_string())
        );
    }

    #[test]
    fn test_indented_comment() {
        let tokens = tokenize("   # indented comment").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::Comment(" indented comment".to_string())
        );
    }

    #[test]
    fn test_recipe_header() {
        let tokens = tokenize(r#"recipe "build""#).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::RecipeHeader { name: "build".to_string(), deps: vec![] });
    }

    #[test]
    fn test_recipe_header_extra_spaces() {
        let tokens = tokenize(r#"recipe   "build""#).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::RecipeHeader { name: "build".to_string(), deps: vec![] });
    }

    #[test]
    fn test_recipe_prefix_is_shell_command() {
        let tokens = tokenize("recipes_cleanup").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::Content("recipes_cleanup".to_string())
        );
    }

    #[test]
    fn test_bare_end_is_content() {
        let tokens = tokenize("end").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::Content("end".to_string()));
    }

    #[test]
    fn test_indented_end_is_content() {
        let tokens = tokenize("   end").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::Content("end".to_string()));
    }

    #[test]
    fn test_lua_line() {
        let tokens = tokenize("> print('hello')").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::LuaLine("print('hello')".to_string())
        );
    }

    #[test]
    fn test_empty_lua_line() {
        let tokens = tokenize(">").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::LuaLine("".to_string()));
    }

    #[test]
    fn test_lua_block_open() {
        let tokens = tokenize(">{").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::LuaBlockOpen);
    }

    #[test]
    fn test_taste_is_content() {
        let tokens = tokenize("taste").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::Content("taste".to_string()));
    }

    #[test]
    fn test_taste_with_args_is_content() {
        let tokens = tokenize("taste test").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::Content("taste test".to_string()));
    }

    #[test]
    fn test_blank_line() {
        let tokens = tokenize("").unwrap();
        assert_eq!(tokens.len(), 0); // no lines from empty string
    }

    #[test]
    fn test_whitespace_only_blank() {
        let tokens = tokenize("   ").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::Blank);
    }

    #[test]
    fn test_shell_command() {
        let tokens = tokenize("gcc -o main main.c").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::Content("gcc -o main main.c".to_string())
        );
    }

    #[test]
    fn test_shell_command_with_double_dash() {
        let tokens = tokenize("cargo test -- --nocapture").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::Content("cargo test -- --nocapture".to_string())
        );
    }

    #[test]
    fn test_multiline_source() {
        let source = r#"# header comment
recipe "build"
  gcc -o main main.c
"#;
        let tokens = tokenize(source).unwrap();
        assert_eq!(tokens.len(), 3);
        assert_eq!(
            tokens[0].value,
            Token::Comment(" header comment".to_string())
        );
        assert_eq!(tokens[1].value, Token::RecipeHeader { name: "build".to_string(), deps: vec![] });
        assert_eq!(
            tokens[2].value,
            Token::Content("gcc -o main main.c".to_string())
        );
    }

    #[test]
    fn test_indented_recipe_is_content() {
        // CS-0019 (E.5): the `recipe` keyword is recognised only at column 0.
        let tokens = tokenize("    recipe inner").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::Content("recipe inner".to_string()));
    }

    #[test]
    fn test_indented_config_is_content() {
        let tokens = tokenize("  config debug").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::Content("config debug".to_string()));
    }

    #[test]
    fn test_indented_use_is_content() {
        let tokens = tokenize("\tuse cpp").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::Content("use cpp".to_string()));
    }

    #[test]
    fn test_indented_import_is_content() {
        let tokens = tokenize("    import backend ./backend").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::Content("import backend ./backend".to_string()),
        );
    }

    #[test]
    fn test_recipe_bare_name() {
        let tokens = tokenize("recipe build").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::RecipeHeader { name: "build".to_string(), deps: vec![] }
        );
    }

    #[test]
    fn test_recipe_bare_name_with_deps() {
        let tokens = tokenize("recipe build: lib setup").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::RecipeHeader {
                name: "build".to_string(),
                deps: vec!["lib".to_string(), "setup".to_string()],
            }
        );
    }

    #[test]
    fn test_recipe_bare_dotted_dep() {
        let tokens = tokenize("recipe bundle: backend.build frontend.build").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::RecipeHeader {
                name: "bundle".to_string(),
                deps: vec!["backend.build".to_string(), "frontend.build".to_string()],
            }
        );
    }

    #[test]
    fn test_recipe_mixed_quoted_bare_deps() {
        let tokens = tokenize(r#"recipe build: lib "my setup""#).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::RecipeHeader {
                name: "build".to_string(),
                deps: vec!["lib".to_string(), "my setup".to_string()],
            }
        );
    }

    #[test]
    fn test_missing_recipe_name() {
        let result = tokenize("recipe :");
        assert!(result.is_err());
    }

    #[test]
    fn test_unterminated_recipe_name() {
        let result = tokenize(r#"recipe "build"#);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, LexError::UnterminatedString { line: 1 }));
    }

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

    #[test]
    fn test_config_header() {
        let tokens = tokenize(r#"config "debug""#).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::ConfigHeader {
                name: Some("debug".to_string()),
            }
        );
    }

    #[test]
    fn test_config_header_not_keyword_prefix() {
        // "configure" should be Content, not ConfigHeader
        let tokens = tokenize("configure").unwrap();
        assert_eq!(tokens[0].value, Token::Content("configure".to_string()));
    }

    #[test]
    fn test_use_decl() {
        let tokens = tokenize(r#"use "cpp""#).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::UseDecl { name: "cpp".to_string() });
    }

    #[test]
    fn test_use_prefix_is_content() {
        let tokens = tokenize("useful").unwrap();
        assert_eq!(tokens[0].value, Token::Content("useful".to_string()));
    }

    #[test]
    fn test_use_bare_name() {
        let tokens = tokenize("use cpp").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::UseDecl { name: "cpp".to_string() });
    }

    #[test]
    fn test_use_name_with_space_rejected() {
        // CS-0035: `use NAME` becomes `local NAME = cook.load_module(...)`.
        // A name with whitespace is not a valid Lua identifier.
        let result = tokenize(r#"use "has spaces""#);
        assert!(result.is_err(), "expected error for use name with spaces");
        assert!(matches!(
            result.unwrap_err(),
            LexError::InvalidUseName { line: 1, .. }
        ));
    }

    #[test]
    fn test_use_name_with_dash_rejected() {
        // CS-0035: hyphens are rejected — `foo-bar` is not a Lua identifier
        // and avoids the silent `foo-bar` ↔ `foo_bar` collision in codegen.
        let result = tokenize("use foo-bar");
        assert!(result.is_err(), "expected error for use name with dash");
        assert!(matches!(
            result.unwrap_err(),
            LexError::InvalidUseName { line: 1, .. }
        ));
    }

    #[test]
    fn test_use_name_with_dots_rejected() {
        // CS-0035: dotted names like `cpp.bad` are not valid Lua identifiers.
        let result = tokenize("use cpp.bad");
        assert!(result.is_err(), "expected error for dotted use name");
        assert!(matches!(
            result.unwrap_err(),
            LexError::InvalidUseName { line: 1, .. }
        ));
    }

    #[test]
    fn test_use_name_starting_with_digit_rejected() {
        let result = tokenize(r#"use "9lives""#);
        assert!(result.is_err(), "expected error for digit-leading use name");
        assert!(matches!(
            result.unwrap_err(),
            LexError::InvalidUseName { line: 1, .. }
        ));
    }

    #[test]
    fn test_use_name_underscore_accepted() {
        let tokens = tokenize("use my_module").unwrap();
        assert_eq!(
            tokens[0].value,
            Token::UseDecl { name: "my_module".to_string() }
        );
    }

    #[test]
    fn test_use_name_leading_underscore_accepted() {
        let tokens = tokenize("use _private").unwrap();
        assert_eq!(
            tokens[0].value,
            Token::UseDecl { name: "_private".to_string() }
        );
    }

    #[test]
    fn test_config_bare_name() {
        let tokens = tokenize("config debug").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::ConfigHeader { name: Some("debug".to_string()) }
        );
    }

    #[test]
    fn test_implicit_form_is_now_content() {
        // CS-0018 (E.6): the bare `name: deps` line at column 0, formerly
        // an implicit recipe header, is now a `Content` token. Inside a
        // recipe body it would dispatch as a `shell_command`; at top level
        // it is rejected as not a valid `toplevel_item`.
        let tokens = tokenize("build: lib setup").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::Content("build: lib setup".to_string()),
        );
    }

    #[test]
    fn test_bare_colon_line_at_column_0_is_content() {
        let tokens = tokenize("clean:").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::Content("clean:".to_string()));
    }

    #[test]
    fn test_import_decl() {
        let tokens = tokenize("import backend ./services/backend").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::ImportDecl {
                name: "backend".to_string(),
                path: "./services/backend".to_string(),
            }
        );
    }

    #[test]
    fn test_import_decl_relative_parent() {
        let tokens = tokenize("import proto ../../libs/proto").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::ImportDecl {
                name: "proto".to_string(),
                path: "../../libs/proto".to_string(),
            }
        );
    }

    #[test]
    fn test_import_prefix_is_content() {
        let tokens = tokenize("important").unwrap();
        assert_eq!(tokens[0].value, Token::Content("important".to_string()));
    }

    #[test]
    fn test_import_missing_path() {
        let result = tokenize("import backend");
        assert!(result.is_err());
    }

    #[test]
    fn test_import_missing_name_and_path() {
        let tokens = tokenize("import").unwrap();
        assert_eq!(tokens[0].value, Token::Content("import".to_string()));
    }

    #[test]
    fn test_bare_config_keyword_tokenizes() {
        let tokens = tokenize("config").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::ConfigHeader { name: None });
    }

    #[test]
    fn test_named_config_keyword_tokenizes() {
        let tokens = tokenize("config release").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::ConfigHeader { name: Some("release".to_string()) }
        );
    }

    #[test]
    fn test_config_prefix_not_a_token() {
        // "configure" starts with "config" but is a bareword command
        let tokens = tokenize("configure --prefix=/usr").unwrap();
        assert!(!matches!(tokens[0].value, Token::ConfigHeader { .. }));
    }

    #[test]
    fn test_chore_header_bare_name() {
        let tokens = tokenize("chore clean").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].value,
            Token::ChoreHeader { name: "clean".to_string(), params: vec![], deps: vec![] },
        );
    }

    #[test]
    fn test_chore_header_quoted_name() {
        let tokens = tokenize(r#"chore "play""#).unwrap();
        assert_eq!(
            tokens[0].value,
            Token::ChoreHeader { name: "play".to_string(), params: vec![], deps: vec![] },
        );
    }

    #[test]
    fn test_chore_header_with_deps() {
        let tokens = tokenize("chore play: build setup").unwrap();
        assert_eq!(
            tokens[0].value,
            Token::ChoreHeader {
                name: "play".to_string(),
                params: vec![],
                deps: vec!["build".to_string(), "setup".to_string()],
            },
        );
    }

    #[test]
    fn test_chore_prefix_is_content() {
        let tokens = tokenize("chores_cleanup").unwrap();
        assert_eq!(tokens[0].value, Token::Content("chores_cleanup".to_string()));
    }

    #[test]
    fn test_indented_chore_is_content() {
        let tokens = tokenize("    chore inner").unwrap();
        assert_eq!(tokens[0].value, Token::Content("chore inner".to_string()));
    }

    #[test]
    fn test_chore_reserved_name_rejected() {
        for reserved in &["stem", "name", "ext", "dir", "in", "out", "all"] {
            let input = format!("chore {}\n", reserved);
            let result = tokenize(&input);
            assert!(result.is_err(), "expected error for chore name '{}'", reserved);
        }
    }

    #[test]
    fn test_reserved_recipe_name_rejected() {
        for reserved in &["stem", "name", "ext", "dir", "in", "out", "all"] {
            let input = format!("recipe {}\n    echo hi\n", reserved);
            let result = crate::parse(&input);
            assert!(
                result.is_err(),
                "expected error for reserved recipe name '{}', got ok",
                reserved
            );
        }
    }

    #[test]
    fn test_dotted_declared_recipe_name_rejected() {
        let input = "recipe backend.build\n    echo hi\n";
        let result = tokenize(input);
        match result {
            Err(LexError::DottedDeclaredRecipeName { ref name, line: 1 }) if name == "backend.build" => {}
            other => panic!("expected DottedDeclaredRecipeName for 'backend.build', got: {:?}", other),
        }
    }

    #[test]
    fn test_dotted_declared_recipe_name_quoted_rejected() {
        let input = "recipe \"backend.build\"\n    echo hi\n";
        let result = tokenize(input);
        match result {
            Err(LexError::DottedDeclaredRecipeName { ref name, line: 1 }) if name == "backend.build" => {}
            other => panic!("expected DottedDeclaredRecipeName for quoted 'backend.build', got: {:?}", other),
        }
    }

    #[test]
    fn test_dotted_declared_chore_name_rejected() {
        let input = "chore tools.fmt\n    echo hi\n";
        let result = tokenize(input);
        match result {
            Err(LexError::DottedDeclaredChoreName { ref name, line: 1 }) if name == "tools.fmt" => {}
            other => panic!("expected DottedDeclaredChoreName for 'tools.fmt', got: {:?}", other),
        }
    }

    #[test]
    fn test_undotted_recipe_with_dotted_dep_accepted() {
        // The no-dots rule is at the *declaration* site; dotted dep references
        // remain legal because they resolve through `import` aliases.
        let input = "recipe ship: backend.build frontend.build\n    echo deploy\n";
        let result = tokenize(input);
        assert!(result.is_ok(), "expected ok for undotted recipe with dotted deps, got: {:?}", result.err());
    }

    #[test]
    fn test_register_header_bare() {
        let tokens = tokenize("register").unwrap();
        assert_eq!(tokens[0].value, Token::RegisterHeader);
    }

    #[test]
    fn test_register_header_with_trailing_whitespace() {
        let tokens = tokenize("register   ").unwrap();
        assert_eq!(tokens[0].value, Token::RegisterHeader);
    }

    #[test]
    fn test_register_header_followed_by_content_is_still_register() {
        // Lexer admits the RegisterHeader; the parser rejects `register foo`.
        let tokens = tokenize("register foo").unwrap();
        assert_eq!(tokens[0].value, Token::RegisterHeader);
    }

    #[test]
    fn test_register_header_with_tab_separator() {
        let tokens = tokenize("register\tfoo").unwrap();
        assert_eq!(tokens[0].value, Token::RegisterHeader);
    }

    #[test]
    fn test_indented_register_is_content() {
        let tokens = tokenize("    register").unwrap();
        assert_eq!(tokens[0].value, Token::Content("register".to_string()));
    }

    #[test]
    fn test_indented_register_keyword_with_arg_is_content() {
        let tokens = tokenize("    register foo").unwrap();
        assert_eq!(tokens[0].value, Token::Content("register foo".to_string()));
    }

    #[test]
    fn test_register_prefix_is_content() {
        // `registers_cleanup` starts with `register` but is a bareword.
        let tokens = tokenize("registers_cleanup").unwrap();
        assert_eq!(tokens[0].value, Token::Content("registers_cleanup".to_string()));
    }

    #[test]
    fn test_register_underscore_is_content() {
        let tokens = tokenize("register_foo").unwrap();
        assert_eq!(tokens[0].value, Token::Content("register_foo".to_string()));
    }
}
