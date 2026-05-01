use crate::ast::*;
use crate::lexer::*;
use crate::lua_block::collect_lua_block;
use crate::ParseError;

/// Parse a body payload — the `body` production from App. A.4 (CS-0024).
///
/// `after_kw` is the line text after the body's introducer keyword
/// (`using` for cook, the empty string for plate/test). `kw_for_diag` is
/// the keyword name used in error messages (`cook using`, `plate`, `test`).
///
/// Returns the parsed `Body` plus the new token-stream position.
pub(crate) fn parse_body_payload(
    after_kw: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
    kw_for_diag: &str,
) -> Result<(Body, usize), ParseError> {
    let trimmed = after_kw.trim_start();

    if trimmed.starts_with(">>{") {
        return Err(ParseError::Parse {
            line,
            message: format!(
                "{}: `>>{{ … }}` (register-phase Lua block) is not a valid body — use `>{{ … }}` for an execute-phase Lua block",
                kw_for_diag
            ),
        });
    }

    if trimmed.starts_with(">{") {
        let (code, new_pos) = collect_lua_block(line, tokens, current_pos + 1, source_lines)?;
        return Ok((Body::LuaBlock(code), new_pos));
    }

    if trimmed.starts_with('{') {
        let after_open = &trimmed[1..];
        if let Some(commands) = crate::shell_block::try_inline_shell_block(after_open) {
            let mut new_pos = current_pos + 1;
            while new_pos < tokens.len() && tokens[new_pos].line <= line {
                new_pos += 1;
            }
            return Ok((Body::ShellBlock(commands), new_pos));
        }
        let (commands, new_pos) =
            crate::shell_block::collect_shell_block(line, tokens, current_pos + 1, source_lines)?;
        return Ok((Body::ShellBlock(commands), new_pos));
    }

    if trimmed.starts_with('"') {
        return Err(ParseError::Parse {
            line,
            message: format!(
                "{}: the bare-string form `\"cmd\"` was removed in CS-0024; rewrite as `{{ cmd }}` (one-line shell block)",
                kw_for_diag
            ),
        });
    }

    Err(ParseError::Parse {
        line,
        message: format!(
            "{}: expected `>{{ Lua block }}` or `{{ shell block }}`, found: {}",
            kw_for_diag, trimmed
        ),
    })
}

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

    let after_using = after_pattern["using".len()..].trim_start();

    let (body, new_pos) =
        parse_body_payload(after_using, line, tokens, current_pos, source_lines, "cook using")?;
    Ok((
        CookStep {
            outputs,
            using_clause: Some(body),
        },
        new_pos,
    ))
}
