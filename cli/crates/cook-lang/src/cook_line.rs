use crate::ast::*;
use crate::lexer::*;
use crate::lua_block::collect_lua_block;
use crate::ParseError;

pub(crate) fn strip_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
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

/// Parse an ingredients line into (includes, excludes).
/// Includes are bare `"pattern"`, excludes are `!"pattern"`.
pub(crate) fn parse_ingredients_line(text: &str, line: usize) -> Result<(Vec<String>, Vec<String>), ParseError> {
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    let mut remaining = text.trim();
    while !remaining.is_empty() {
        let is_exclude = remaining.starts_with('!');
        if is_exclude {
            remaining = &remaining[1..];
        }
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
        let value = rest[..end].to_string();
        if is_exclude {
            excludes.push(value);
        } else {
            includes.push(value);
        }
        remaining = rest[end + 1..].trim();
    }
    Ok((includes, excludes))
}

pub(crate) fn parse_single_quoted_string(text: &str, line: usize) -> Result<String, ParseError> {
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

pub(crate) fn parse_test_command<'a>(text: &'a str, line: usize) -> Result<(String, &'a str), ParseError> {
    let text = text.trim();
    if !text.starts_with('"') {
        return Err(ParseError::Parse {
            line,
            message: format!("test: expected '\"', found: {}", text),
        });
    }
    let rest = &text[1..];
    let end = rest.find('"').ok_or(ParseError::Parse {
        line,
        message: "test: unterminated string".to_string(),
    })?;
    Ok((rest[..end].to_string(), rest[end + 1..].trim()))
}

pub(crate) fn parse_test_timeout(text: &str) -> (Option<u64>, &str) {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("timeout") {
        let rest = rest.trim();
        // Split on next whitespace or take all remaining
        if let Some(space_pos) = rest.find(|c: char| c.is_whitespace()) {
            if let Ok(n) = rest[..space_pos].parse::<u64>() {
                return (Some(n), rest[space_pos..].trim());
            }
        } else if let Ok(n) = rest.parse::<u64>() {
            return (Some(n), "");
        }
    }
    (None, text)
}

pub(crate) fn parse_cook_line(
    rest: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(CookStep, usize), ParseError> {
    let rest = rest.trim();
    if !rest.starts_with('"') {
        return Err(ParseError::Parse {
            line,
            message: "cook: expected quoted output pattern".to_string(),
        });
    }

    // Collect all leading quoted strings.
    let mut outputs: Vec<String> = Vec::new();
    let mut cursor = rest;
    while cursor.starts_with('"') {
        let after_quote = &cursor[1..];
        let end = after_quote.find('"').ok_or(ParseError::Parse {
            line,
            message: "cook: unterminated output pattern".to_string(),
        })?;
        outputs.push(after_quote[..end].to_string());
        cursor = after_quote[end + 1..].trim_start();
    }
    let after_pattern = cursor.trim();

    if after_pattern.is_empty() {
        return Ok((
            CookStep {
                outputs,
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

    // App. A.4 "`using >>{` is rejected": register-phase Lua block is incoherent
    // as a using-clause payload (the using clause produces work for execute, not
    // register-time orchestration). Diagnose sharply.
    if after_using.starts_with(">>{") {
        return Err(ParseError::Parse {
            line,
            message: "cook using: `>>{ … }` (register-phase Lua block) is not a valid using-clause payload — use `>{ … }` for an execute-phase Lua block".to_string(),
        });
    }

    if after_using.starts_with(">{") {
        let (code, new_pos) = collect_lua_block(line, tokens, current_pos + 1, source_lines)?;
        Ok((
            CookStep {
                outputs,
                using_clause: Some(UsingClause::LuaBlock(code)),
            },
            new_pos,
        ))
    } else if after_using.starts_with('{') {
        let (commands, new_pos) = crate::shell_block::collect_shell_block(
            line,
            tokens,
            current_pos + 1,
            source_lines,
        )?;
        Ok((
            CookStep {
                outputs,
                using_clause: Some(UsingClause::ShellBlock(commands)),
            },
            new_pos,
        ))
    } else if after_using.starts_with('"') {
        Err(ParseError::Parse {
            line,
            message: "cook using: the bare-string form `using \"cmd\"` was removed in CS-0022; \
                      rewrite as `using { cmd }` (one-line shell block)"
                .to_string(),
        })
    } else {
        Err(ParseError::Parse {
            line,
            message: format!(
                "cook using: expected `>{{ Lua block }}` or `{{ shell block }}`, found: {}",
                after_using
            ),
        })
    }
}
