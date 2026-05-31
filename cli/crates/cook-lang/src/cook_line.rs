use crate::ast::*;
use crate::lexer::*;
use crate::lua_block::collect_lua_block;
use crate::ParseError;

/// Try to extract a complete Lua block from the text immediately following
/// the opening `>{`. `after_open` is the slice of the source line that comes
/// after the `>{` that started the block.
///
/// Scans forward tracking brace depth (starting at 1 because the opening `{`
/// has already been consumed). If depth reaches 0 on the same span, returns
/// the content between the opening `{` and the matching `}` (trimmed).
/// Returns `None` if the line does not contain a complete balanced block
/// (i.e., the block spans multiple lines).
///
/// Handles the following Lua constructs that may contain literal `{`/`}`
/// characters which must not affect the brace counter:
/// * `"`/`'`-quoted strings (with `\`-escape handling).
/// * `--` line comments — a `--` on the `>{` line means the closing `}` (if
///   any) is commented out; the block must therefore continue on subsequent
///   lines, so `None` is returned and the multiline collector takes over.
/// * Lua long-bracket strings (`[[…]]`, `[=[…]=]`, `[==[…]==]` etc.):
///   the function skips to the matching close bracket; if no matching close
///   exists on this line, it returns `None` (the long string is unterminated
///   on this line → multiline collector).
///
/// This is the Lua counterpart to `shell_block::try_inline_shell_block`.
pub(crate) fn try_inline_lua_block(after_open: &str) -> Option<String> {
    use crate::brace_scan::{match_close_long_bracket, match_open_long_bracket};

    let chars: Vec<char> = after_open.chars().collect();
    let len = chars.len();
    let mut depth: i32 = 1;
    let mut i = 0;

    while i < len {
        let c = chars[i];

        match c {
            '{' => { depth += 1; i += 1; }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    // Reconstruct the content up to (not including) this `}`.
                    let content: String = chars[..i].iter().collect();
                    return Some(content.trim().to_string());
                }
                i += 1;
            }
            '"' | '\'' => {
                let quote = c;
                i += 1;
                while i < len && chars[i] != quote {
                    if chars[i] == '\\' && i + 1 < len { i += 2; } else { i += 1; }
                }
                if i < len { i += 1; } // skip closing quote
            }
            '-' if i + 1 < len && chars[i + 1] == '-' => {
                // Lua line comment: rest of line is comment, no closing brace
                // can be found on this span. A `--` on the `>{` line means
                // the closing `}` (if any) is commented out; the multiline
                // collector must handle the block.
                return None;
            }
            '[' => {
                // Lua long-bracket string: `[[…]]`, `[=[…]=]`, etc.
                if let Some((level, after_open_lb)) = match_open_long_bracket(&chars, i) {
                    // Scan forward character by character looking for the matching
                    // close bracket on this line.
                    let mut j = after_open_lb;
                    let mut found_close = false;
                    while j < len {
                        if chars[j] == ']' {
                            if let Some(after_close) = match_close_long_bracket(&chars, j, level) {
                                // Close found on same line — skip past it and continue.
                                i = after_close;
                                found_close = true;
                                break;
                            }
                        }
                        j += 1;
                    }
                    if !found_close {
                        // Long string continues past this line — multiline.
                        return None;
                    }
                } else {
                    i += 1;
                }
            }
            _ => { i += 1; }
        }
    }
    None // no matching `}` on this span
}

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
        let after_open = &trimmed[2..]; // text after `>{`
        if let Some(code) = try_inline_lua_block(after_open) {
            // Single-line Lua block: `>{ … }` all on one line.
            let mut new_pos = current_pos + 1;
            while new_pos < tokens.len() && tokens[new_pos].line <= line {
                new_pos += 1;
            }
            return Ok((Body::LuaBlock(code), new_pos));
        }
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

/// Collect a run of quoted patterns from `initial_text`, then keep
/// collecting from subsequent physical source lines as long as each
/// continuation line begins with `"` (or `!"` when `allow_exclude` is true).
///
/// Returns:
///  * `patterns` — collected pattern values (without the surrounding quotes)
///  * `excludes` — parallel `bool` array marking exclude (`!"…"`) entries.
///    Always `false` when `allow_exclude` is false; the caller may ignore.
///  * `leftover` — the unparsed remainder of the line where collection
///    stopped. For `cook`, this is the text starting at `using`. For
///    `ingredients`, this is empty (no trailing clause).
///  * `new_pos` — token-stream position pointing at the first token of the
///    line where collection stopped (e.g. the line containing `using` for
///    `cook`, or the line that broke the pattern run). NOTE: this differs
///    from `collect_lua_block`/`collect_shell_block`, which return a
///    position PAST the consumed block. Callers that want "past the stop
///    line" must advance one more line themselves.
///
/// The caller must inspect `leftover` to decide whether the stopping line
/// is well-formed (e.g. begins with `using` for cook, is empty for
/// ingredients).
pub(crate) fn collect_quoted_patterns_multiline(
    initial_text: &str,
    initial_line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
    allow_exclude: bool,
) -> Result<(Vec<String>, Vec<bool>, String, usize), ParseError> {
    let mut patterns: Vec<String> = Vec::new();
    let mut excludes: Vec<bool> = Vec::new();
    let mut cursor = initial_text.trim().to_string();
    let mut cur_line = initial_line;
    let mut pos = current_pos;

    loop {
        // Consume as many quoted patterns as possible from `cursor`.
        loop {
            let trimmed = cursor.trim_start();
            let is_exclude = allow_exclude && trimmed.starts_with('!');
            let after_bang = if is_exclude { &trimmed[1..] } else { trimmed };
            if !after_bang.starts_with('"') {
                cursor = trimmed.to_string();
                break;
            }
            let rest = &after_bang[1..];
            let end = rest.find('"').ok_or_else(|| ParseError::Parse {
                line: cur_line,
                message: "unterminated string".to_string(),
            })?;
            patterns.push(rest[..end].to_string());
            excludes.push(is_exclude);
            cursor = rest[end + 1..].to_string();
        }

        // If the cursor is non-empty here, we hit a non-quote token. Stop.
        if !cursor.trim().is_empty() {
            return Ok((patterns, excludes, cursor.trim().to_string(), pos));
        }

        // Look at the next physical line. If it starts with `"` (or `!"`
        // when allow_exclude), consume it and continue.
        let next_line_num = cur_line + 1;
        let next_line_text = match source_lines.get(next_line_num.saturating_sub(1)) {
            Some(t) => t.trim_start(),
            None => return Ok((patterns, excludes, String::new(), pos)),
        };
        let starts_pattern = next_line_text.starts_with('"')
            || (allow_exclude && next_line_text.starts_with('!')
                && next_line_text.get(1..2) == Some("\""));
        if !starts_pattern {
            return Ok((patterns, excludes, String::new(), pos));
        }

        // Advance pos past every token on lines <= cur_line.
        while pos < tokens.len() && tokens[pos].line <= cur_line {
            pos += 1;
        }
        cur_line = next_line_num;
        cursor = next_line_text.to_string();
    }
}

/// Parse an ingredients declaration. Patterns may span multiple physical
/// lines as long as each continuation line begins with `"` or `!"`.
/// Returns (includes, excludes, new_pos).
pub(crate) fn parse_ingredients_line(
    text: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(Vec<String>, Vec<String>, usize), ParseError> {
    let (patterns, excludes_flags, leftover, pos_after) =
        collect_quoted_patterns_multiline(
            text, line, tokens, current_pos, source_lines, /*allow_exclude=*/ true,
        )?;
    if !leftover.trim().is_empty() {
        return Err(ParseError::Parse {
            line,
            message: format!("ingredients: expected '\"' or '!\"', found: {}", leftover),
        });
    }
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    for (pat, is_exc) in patterns.into_iter().zip(excludes_flags.into_iter()) {
        if is_exc { excludes.push(pat); } else { includes.push(pat); }
    }
    // Advance past every token on the line where collection stopped. Explicit
    // walk (matches the pattern used in `parse_cook_line`'s declaration-only
    // branch) so this stays correct if the lexer ever emits more than one
    // token per source line.
    let stop_line = tokens
        .get(pos_after)
        .map(|t| t.line)
        .unwrap_or(line);
    let mut pos = pos_after;
    while pos < tokens.len() && tokens[pos].line <= stop_line {
        pos += 1;
    }
    Ok((includes, excludes, pos))
}


/// Parse a `for_each` step line (§8.3): `for_each <source> ("as" "lines")?`.
///
/// `rest` is the line text AFTER the `for_each` keyword. The source forms are
/// (per the §8.3 grammar `for_each_source ::= probe_ref | "$(" SHELL ")" |
/// "(" LUA_EXPR ")"`):
///
///  * `$(cmd)` — a register-time shell capture; balanced-paren-scanned, stored
///    without the surrounding `$( )`.
///  * `(lua)`  — the reserved Lua-expression source; parses now, rejected at
///    codegen with a "not yet supported" diagnostic.
///  * `probe_ref` — `IDENT (":" IDENT)?` (e.g. `cards`, `cards:items`); stored
///    verbatim (the codegen splits the optional `:field`).
///
/// The optional `as lines` modifier disables JSON parsing of a `$(cmd)` source;
/// §8.3 requires it to be rejected for a `probe_ref`. Returns the parsed
/// [`ForEachStep`] and the token-stream position past this line.
pub(crate) fn parse_for_each_line(
    rest: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
) -> Result<(ForEachStep, usize), ParseError> {
    let rest = rest.trim();

    let (source, leftover): (ForEachSource, &str) = if let Some(after) = rest.strip_prefix("$(") {
        let (cmd, tail) = scan_balanced_paren_expr(after, line)?;
        (ForEachSource::ShellCapture(cmd.trim().to_string()), tail)
    } else if let Some(after) = rest.strip_prefix('(') {
        let (expr, tail) = scan_balanced_paren_expr(after, line)?;
        (ForEachSource::LuaExpr(expr.trim().to_string()), tail)
    } else {
        // probe_ref ::= IDENT (":" IDENT)? — a run of ident chars plus the
        // single `:` field separator. A `.` here is not a valid probe_ref
        // character, so it surfaces below as trailing content.
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | ':')))
            .unwrap_or(rest.len());
        if end == 0 {
            return Err(ParseError::Parse {
                line,
                message: "for_each: expected a probe key, $(cmd), or (lua-expr) source"
                    .to_string(),
            });
        }
        (ForEachSource::ProbeKey(rest[..end].to_string()), &rest[end..])
    };

    let leftover = leftover.trim();
    let (as_lines, leftover) = match leftover.strip_prefix("as lines") {
        Some(tail) => (true, tail.trim()),
        None => (false, leftover),
    };

    if !leftover.is_empty() {
        return Err(ParseError::Parse {
            line,
            message: format!("for_each: unexpected trailing content '{leftover}'"),
        });
    }

    // §8.3: `as lines` is only meaningful for a `$(cmd)` source; a probe_ref's
    // members are already typed values, so `as lines` on one MUST be rejected.
    if as_lines && matches!(source, ForEachSource::ProbeKey(_)) {
        return Err(ParseError::Parse {
            line,
            message: "for_each: `as lines` is only valid for a $(cmd) source, not a probe key"
                .to_string(),
        });
    }

    // Advance the token cursor past every token on this line.
    let mut pos = current_pos + 1;
    while pos < tokens.len() && tokens[pos].line <= line {
        pos += 1;
    }
    Ok((ForEachStep { source, as_lines }, pos))
}

/// Brace-balanced scan for a `cook (LUA_EXPR)` payload. `text` is the
/// content AFTER the opening `(`. Scans until the matching `)` is found,
/// honouring nested parens and basic double/single-quoted string literals.
/// Returns `(trimmed_expr, text_after_close_paren)`.
///
/// Mirrors `lexer::scan_balanced_paren` (used for chore default-param
/// `=(EXPR)` per §7.1.1) but returns `ParseError` instead of `LexError`
/// since cook-step parsing happens post-lex.
///
/// **Limitation (v1).** Lua long-bracket strings (`[[...]]`) inside the
/// expression are NOT handled; treat them as ordinary characters. This
/// matches the §7.1.1 chore-param scanner and is documented in §8.4.2.
fn scan_balanced_paren_expr<'a>(
    text: &'a str,
    line: usize,
) -> Result<(String, &'a str), ParseError> {
    let bytes = text.as_bytes();
    let mut depth = 1usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((text[..i].trim().to_string(), &text[i + 1..]));
                }
                i += 1;
            }
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i >= bytes.len() {
                    return Err(ParseError::Parse {
                        line,
                        message: "cook (LUA_EXPR): unclosed string literal in expression"
                            .to_string(),
                    });
                }
                i += 1; // skip closing quote
            }
            _ => {
                i += 1;
            }
        }
    }
    Err(ParseError::Parse {
        line,
        message: "cook (LUA_EXPR): unclosed '(' in output expression".to_string(),
    })
}

pub(crate) fn parse_cook_line(
    rest: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(CookStep, usize), ParseError> {
    let rest = rest.trim();

    // §8.4.2 Lua-expression form: `cook (EXPR) using ...`. Detected by a
    // leading `(`; the expression is balanced-paren-scanned (mirroring the
    // chore default-param `=(EXPR)` form, §7.1.1). The one-to-one form
    // admits exactly one output by construction — multi-output mixing with
    // quoted patterns is rejected with a Standard-§8.4.2 diagnostic.
    if rest.starts_with('(') {
        let after_open = &rest[1..];
        let (expr, after_close) = scan_balanced_paren_expr(after_open, line)?;
        let leftover = after_close.trim();

        if leftover.starts_with('"') || leftover.starts_with('(') {
            return Err(ParseError::Parse {
                line,
                message:
                    "cook (LUA_EXPR) form requires exactly one output (Cook Standard §8.4.2)"
                        .to_string(),
            });
        }

        let outputs = vec![OutputPattern::LuaExpr(expr)];

        if leftover.is_empty() {
            // Declaration-only Lua-expr cook step is meaningless: there's
            // no body to evaluate, and the unit's ingredients can't drive
            // anything. Reject early — §8.4.2 implies a `using`-bearing step.
            return Err(ParseError::Parse {
                line,
                message: "cook (LUA_EXPR): expected 'using' after expression"
                    .to_string(),
            });
        }

        if !leftover.starts_with("using") {
            return Err(ParseError::Parse {
                line,
                message: format!(
                    "cook (LUA_EXPR): expected 'using' after expression, found: {}",
                    leftover
                ),
            });
        }

        let after_using = leftover["using".len()..].trim_start();
        let (body, new_pos) = parse_body_payload(
            after_using,
            line,
            tokens,
            current_pos,
            source_lines,
            "cook using",
        )?;
        return Ok((
            CookStep { outputs, using_clause: Some(body) },
            new_pos,
        ));
    }

    if !rest.starts_with('"') {
        return Err(ParseError::Parse {
            line,
            message: "cook: expected quoted output pattern or `(LUA_EXPR)`".to_string(),
        });
    }

    let (output_strs, _excludes, leftover, pos_after_patterns) =
        collect_quoted_patterns_multiline(
            rest, line, tokens, current_pos, source_lines, /*allow_exclude=*/ false,
        )?;
    let outputs: Vec<OutputPattern> =
        output_strs.into_iter().map(OutputPattern::Quoted).collect();

    let after_pattern = leftover.trim();

    if after_pattern.is_empty() {
        // Declaration-only cook step. Advance past every token on the line
        // where collection stopped (the line containing the last quoted
        // pattern). Explicit walk instead of `pos + 1` so this stays correct
        // if the lexer ever emits more than one token per source line.
        let stop_line = tokens
            .get(pos_after_patterns)
            .map(|t| t.line)
            .unwrap_or(line);
        let mut pos = pos_after_patterns;
        while pos < tokens.len() && tokens[pos].line <= stop_line {
            pos += 1;
        }
        return Ok((
            CookStep { outputs, using_clause: None },
            pos,
        ));
    }

    if !after_pattern.starts_with("using") {
        return Err(ParseError::Parse {
            line,
            message: format!("cook: expected 'using' after output pattern, found: {}", after_pattern),
        });
    }

    let after_using = after_pattern["using".len()..].trim_start();

    // parse_body_payload needs the line that the `using` keyword sits on.
    // After our multiline pattern walk, pos_after_patterns points at the
    // first token on the line where pattern collection stopped — which is
    // the line `leftover` came from. Read its line number off the token.
    let using_line = tokens.get(pos_after_patterns).map(|t| t.line).unwrap_or(line);

    let (body, new_pos) =
        parse_body_payload(after_using, using_line, tokens, pos_after_patterns, source_lines, "cook using")?;
    Ok((
        CookStep { outputs, using_clause: Some(body), },
        new_pos,
    ))
}
