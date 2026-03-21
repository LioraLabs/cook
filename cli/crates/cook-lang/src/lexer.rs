use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Comment(String),
    RecipeHeader { name: String, deps: Vec<String> },
    ConfigHeader { name: String },
    VarDecl { name: String, value: String },
    UseDecl { name: String },
    ImportDecl { name: String, path: String },
    RecipeEnd,
    LuaLine(String),
    LuaBlockOpen,
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
}

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

fn try_parse_var_decl(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let space_pos = line.find(|c: char| c == ' ' || c == '\t')?;
    let name = &line[..space_pos];

    // Var names must not be keywords
    if matches!(name, "recipe" | "config" | "end" | "ingredients" | "cook" | "plate" | "using" | "use" | "import" | "test") {
        return None;
    }

    let rest = line[space_pos..].trim();
    if !rest.starts_with('"') {
        return None;
    }
    let inner = &rest[1..];
    let end = inner.find('"')?;
    let after = inner[end + 1..].trim();
    if !after.is_empty() {
        return None;
    }
    Some((name.to_string(), inner[..end].to_string()))
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
        } else if trimmed.starts_with(">{") {
            Token::LuaBlockOpen
        } else if trimmed.starts_with('>') {
            let code = &trimmed[1..];
            let code = code.strip_prefix(' ').unwrap_or(code);
            Token::LuaLine(code.to_string())
        } else if trimmed == "end" {
            Token::RecipeEnd
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

            let deps = if let Some(after_colon) = after_name.strip_prefix(':') {
                parse_quoted_strings(after_colon.trim(), line_num)?
            } else {
                vec![]
            };

            Token::RecipeHeader { name, deps }
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
        } else if trimmed.starts_with("use")
            && trimmed.len() > 3
            && (trimmed.as_bytes()[3] == b' '
                || trimmed.as_bytes()[3] == b'\t'
                || trimmed.as_bytes()[3] == b'"')
        {
            let rest = trimmed["use".len()..].trim();
            if !rest.starts_with('"') {
                return Err(LexError::MissingRecipeName { line: line_num });
            }
            let rest = &rest[1..];
            let end = rest
                .find('"')
                .ok_or(LexError::UnterminatedString { line: line_num })?;
            let name = rest[..end].to_string();
            Token::UseDecl { name }
        } else if trimmed.starts_with("import")
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
        } else if !trimmed.starts_with('@') {
            if let Some(var) = try_parse_var_decl(trimmed) {
                Token::VarDecl { name: var.0, value: var.1 }
            } else {
                Token::Content(trimmed.to_string())
            }
        } else {
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
    fn test_recipe_end() {
        let tokens = tokenize("end").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::RecipeEnd);
    }

    #[test]
    fn test_indented_end() {
        let tokens = tokenize("   end").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::RecipeEnd);
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
end
"#;
        let tokens = tokenize(source).unwrap();
        assert_eq!(tokens.len(), 4);
        assert_eq!(
            tokens[0].value,
            Token::Comment(" header comment".to_string())
        );
        assert_eq!(tokens[1].value, Token::RecipeHeader { name: "build".to_string(), deps: vec![] });
        assert_eq!(
            tokens[2].value,
            Token::Content("gcc -o main main.c".to_string())
        );
        assert_eq!(tokens[3].value, Token::RecipeEnd);
    }

    #[test]
    fn test_missing_recipe_name() {
        let result = tokenize("recipe build");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, LexError::MissingRecipeName { line: 1 }));
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

    #[test]
    fn test_use_decl() {
        let tokens = tokenize(r#"use "cpp""#).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::UseDecl { name: "cpp".to_string() });
    }

    #[test]
    fn test_use_decl_not_var() {
        let tokens = tokenize(r#"use "cpp""#).unwrap();
        assert!(!matches!(tokens[0].value, Token::VarDecl { .. }));
    }

    #[test]
    fn test_use_prefix_is_content() {
        let tokens = tokenize("useful").unwrap();
        assert_eq!(tokens[0].value, Token::Content("useful".to_string()));
    }

    #[test]
    fn test_use_missing_name() {
        let result = tokenize("use foo");
        assert!(result.is_err());
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
}
