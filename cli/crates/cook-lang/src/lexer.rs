use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Comment(String),
    RecipeHeader { name: String, deps: Vec<String> },
    ChoreHeader { name: String, deps: Vec<String> },
    ConfigHeader { name: Option<String> },
    UseDecl { name: String },
    ImportDecl { name: String, path: String },
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
    #[error("line {line}: '{segment}' is a reserved word and cannot be used as a recipe name (or final segment of a dotted recipe name)")]
    ReservedRecipeName { line: usize, segment: String },
    #[error("line {line}: a run of three or more `>` characters at line start is reserved (§{{lexical.line-prefixes}})")]
    ReservedTripleArrow { line: usize },
}

const RESERVED_RECIPE_SEGMENTS: &[&str] = &["stem", "name", "ext", "dir", "in", "out", "all"];

fn check_reserved_recipe_name(name: &str, line: usize) -> Result<(), LexError> {
    let segment = name.rsplit('.').next().unwrap_or(name);
    if RESERVED_RECIPE_SEGMENTS.contains(&segment) {
        return Err(LexError::ReservedRecipeName {
            line,
            segment: segment.to_string(),
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
            check_reserved_recipe_name(&name, line_num)?;

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

            let deps = if let Some(after_colon) = after_name.strip_prefix(':') {
                parse_names(after_colon.trim(), line_num)?
            } else {
                vec![]
            };

            Token::ChoreHeader { name, deps }
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
            && trimmed.starts_with("use")
            && trimmed.len() > 3
            && (trimmed.as_bytes()[3] == b' '
                || trimmed.as_bytes()[3] == b'\t'
                || trimmed.as_bytes()[3] == b'"')
        {
            let rest = trimmed["use".len()..].trim();
            let (name, _) = parse_name(rest, line_num)?;
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
            Token::ChoreHeader { name: "clean".to_string(), deps: vec![] },
        );
    }

    #[test]
    fn test_chore_header_quoted_name() {
        let tokens = tokenize(r#"chore "play""#).unwrap();
        assert_eq!(
            tokens[0].value,
            Token::ChoreHeader { name: "play".to_string(), deps: vec![] },
        );
    }

    #[test]
    fn test_chore_header_with_deps() {
        let tokens = tokenize("chore play: build setup").unwrap();
        assert_eq!(
            tokens[0].value,
            Token::ChoreHeader {
                name: "play".to_string(),
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
    fn test_reserved_name_in_dotted_recipe_rejected() {
        let input = "recipe backend.stem\n    echo hi\n";
        let result = crate::parse(input);
        assert!(result.is_err(), "expected error for recipe named 'backend.stem'");
    }

    #[test]
    fn test_non_reserved_dotted_name_accepted() {
        let input = "recipe backend.build\n    echo hi\n";
        let result = crate::parse(input);
        assert!(result.is_ok(), "expected ok for recipe named 'backend.build', got: {:?}", result.err());
    }
}
