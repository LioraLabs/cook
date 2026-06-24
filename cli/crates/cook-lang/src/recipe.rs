use crate::ast::*;
use crate::brace_scan::LuaScanner;
use crate::cook_line::*;
use crate::disposition::{classify_disposition_line, fold_decorator, DispLine};
use crate::lexer::*;
use crate::lua_block::collect_lua_block;
use crate::ParseError;

/// Returns true if `text` looks like a module function call: `ident.ident...`
pub(crate) fn is_module_call(text: &str) -> bool {
    let bytes = text.as_bytes();
    // Must start with an ASCII letter or underscore
    if bytes.is_empty() || !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_') {
        return false;
    }
    // Find the dot
    let mut i = 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'.' {
        return false;
    }
    // After the dot, must have at least one ident char
    i += 1;
    i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_')
}

/// Collects a module call that may span multiple lines (when braces are unbalanced).
///
/// A module call's body is Lua source, so the brace counter uses the stateful
/// [`LuaScanner`] (CS-0035): braces inside multi-line long strings or block
/// comments are treated as data and do not prematurely close the call.
pub(crate) fn collect_module_call(
    first_line_text: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(String, usize), ParseError> {
    let mut scanner = LuaScanner::new();
    let mut depth = scanner.scan_line(first_line_text);

    if depth <= 0 {
        // Single-line call (balanced or no braces)
        return Ok((first_line_text.to_string(), current_pos + 1));
    }

    // Multi-line: collect subsequent source lines until braces balance
    let mut code_lines = vec![first_line_text.to_string()];
    // line is 1-indexed; source_lines is 0-indexed
    let mut line_idx = line; // next source line (0-indexed)

    while line_idx < source_lines.len() {
        let raw_line = source_lines[line_idx];
        depth += scanner.scan_line(raw_line);
        code_lines.push(raw_line.to_string());

        if depth <= 0 {
            break;
        }
        line_idx += 1;
    }

    if depth > 0 {
        return Err(ParseError::Parse {
            line,
            message: "unclosed brace in module call".to_string(),
        });
    }

    let code = code_lines.join("\n");
    let close_line_1indexed = line_idx + 1;

    // Skip all tokens whose line is <= the closing line
    let mut new_pos = current_pos + 1;
    while new_pos < tokens.len() && tokens[new_pos].line <= close_line_1indexed {
        new_pos += 1;
    }

    Ok((code, new_pos))
}

pub(crate) fn parse_config_block_lua(
    tokens: &[Located<Token>],
    start: usize,
    open_line: usize,
    source_lines: &[&str],
) -> Result<(String, usize), ParseError> {
    // CS-0019: scan to the next column-0 top-level keyword or EOF.
    // The terminating token is left in place for parse() to dispatch.
    let mut pos = start;
    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::RecipeHeader { .. }
            | Token::ChoreHeader { .. }
            | Token::ConfigHeader { .. }
            | Token::UseDecl { .. }
            | Token::ImportDecl { .. }
            | Token::RegisterHeader
            | Token::ProbeHeader { .. } => break,
            // Top-level module_call (column-0 Content matching the module-call
            // shape) is also a terminator as of CS-0072. Check the raw source
            // line to distinguish column-0 from indented Content.
            Token::Content(text) if is_module_call(text) => {
                let raw = source_lines
                    .get(tok.line.saturating_sub(1))
                    .copied()
                    .unwrap_or("");
                if !raw.starts_with(|c: char| c.is_whitespace()) {
                    break;
                }
                pos += 1;
            }
            _ => pos += 1,
        }
    }

    let end_line = if pos < tokens.len() {
        tokens[pos].line
    } else {
        source_lines.len() + 1
    };

    let start_idx = open_line; // 1-indexed line of header; body starts at the next line
    let end_idx = end_line.saturating_sub(1);
    let body = if start_idx < end_idx && end_idx <= source_lines.len() {
        // Trim trailing blank lines so that a blank separator between the
        // config block body and the next keyword does not become part of
        // the body (consistent with v0.3 explicit-`end` behaviour).
        let lines = &source_lines[start_idx..end_idx];
        let trimmed_end = lines.iter().rposition(|l| !l.trim().is_empty())
            .map(|i| i + 1)
            .unwrap_or(0);
        lines[..trimmed_end].join("\n")
    } else {
        String::new()
    };

    Ok((body, pos))
}

pub(crate) fn parse_register_block_lua(
    tokens: &[Located<Token>],
    start: usize,
    open_line: usize,
    source_lines: &[&str],
) -> Result<(String, usize), ParseError> {
    // Mirror parse_config_block_lua: scan to the next column-0 top-level
    // keyword OR top-level module_call shape OR EOF. The terminating
    // token is left in place for parse() to dispatch.
    let mut pos = start;
    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::RecipeHeader { .. }
            | Token::ChoreHeader { .. }
            | Token::ConfigHeader { .. }
            | Token::UseDecl { .. }
            | Token::ImportDecl { .. }
            | Token::RegisterHeader
            | Token::ProbeHeader { .. } => break,
            // Top-level module_call (Content matching <id>.<id>(...) shape)
            // is also a terminator (CS-0072 §4.1.1 clause b).
            // Only column-0 Content can be top-level: check the raw source line.
            Token::Content(text) if is_module_call(text) => {
                let raw = source_lines
                    .get(tok.line.saturating_sub(1))
                    .copied()
                    .unwrap_or("");
                if !raw.starts_with(|c: char| c.is_whitespace()) {
                    break;
                }
                pos += 1;
            }
            _ => pos += 1,
        }
    }

    let end_line = if pos < tokens.len() {
        tokens[pos].line
    } else {
        source_lines.len() + 1
    };

    let start_idx = open_line;
    let end_idx = end_line.saturating_sub(1);
    let body = if start_idx < end_idx && end_idx <= source_lines.len() {
        let lines = &source_lines[start_idx..end_idx];
        let trimmed_end = lines
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map(|i| i + 1)
            .unwrap_or(0);
        lines[..trimmed_end].join("\n")
    } else {
        String::new()
    };

    Ok((body, pos))
}

struct TestModifierTail {
    as_name: Option<String>,
    timeout: Option<u64>,
    should_fail: bool,
}

/// Parse a single-quoted string from the beginning of `s`.
/// Returns `(content, rest_after_closing_quote)` on success.
fn parse_single_quoted(s: &str, line: usize) -> Result<(String, &str), ParseError> {
    // `s` should start with `'`
    let inner = &s[1..];
    match inner.find('\'') {
        Some(end) => Ok((inner[..end].to_string(), &inner[end + 1..])),
        None => Err(ParseError::Parse {
            line,
            message: "test: unterminated single-quoted string in `as` modifier".to_string(),
        }),
    }
}

/// Parse the trailing modifier suffix on a `test` line — the text that
/// follows the closing brace of the body. Accepts (canonical order per §4.8):
///     as 'NAME'
///     timeout N
///     should_fail
///     as 'NAME' timeout N should_fail
///     (and all valid subsets in canonical order)
/// Out-of-order sequences are rejected.
fn parse_test_modifier_tail(line_text: &str, line: usize) -> Result<TestModifierTail, ParseError> {
    let suffix = match line_text.rfind('}') {
        Some(idx) => &line_text[idx + 1..],
        None => "",
    };
    let mut rest = suffix.trim();

    // ── as 'NAME' ────────────────────────────────────────────────────
    let as_name = if rest.starts_with("as") && rest[2..].starts_with(|c: char| c.is_whitespace() || c == '\'') {
        let after_as = rest[2..].trim_start();
        if !after_as.starts_with('\'') {
            return Err(ParseError::Parse {
                line,
                message: "test: `as` requires a single-quoted string argument".to_string(),
            });
        }
        let (name, remaining) = parse_single_quoted(after_as, line)?;
        rest = remaining.trim_start();
        Some(name)
    } else {
        None
    };

    // ── timeout N ────────────────────────────────────────────────────
    let timeout = if rest.starts_with("timeout") && rest[7..].starts_with(|c: char| c.is_whitespace() || c.is_ascii_digit()) {
        // Check `as` is not lurking after timeout (order enforcement handled below)
        let after_timeout = rest[7..].trim_start();
        let (num_str, remaining) = match after_timeout.split_once(|c: char| c.is_whitespace()) {
            Some((n, r)) => (n, r.trim_start()),
            None => (after_timeout, ""),
        };
        let n = num_str.parse::<u64>().map_err(|_| ParseError::Parse {
            line,
            message: format!("test: invalid timeout value: {}", num_str),
        })?;
        rest = remaining;
        Some(n)
    } else {
        None
    };

    // Reject `as` appearing after `timeout` (canonical order violation).
    if rest.starts_with("as") && (rest.len() == 2 || rest[2..].starts_with(|c: char| c.is_whitespace() || c == '\'')) {
        return Err(ParseError::Parse {
            line,
            message: "test: modifier `as` must precede `timeout` in test_step \
                      (canonical order: as → timeout → should_fail)".to_string(),
        });
    }

    // ── should_fail ──────────────────────────────────────────────────
    let should_fail = if rest == "should_fail" || rest.starts_with("should_fail ") {
        let remaining = rest["should_fail".len()..].trim_start();
        rest = remaining;
        true
    } else {
        false
    };

    // Reject `as` or `timeout` appearing after `should_fail` (order violations).
    if should_fail {
        if rest.starts_with("as") && (rest.len() == 2 || rest[2..].starts_with(|c: char| c.is_whitespace() || c == '\'')) {
            return Err(ParseError::Parse {
                line,
                message: "test: modifier `as` must precede `should_fail` in test_step \
                          (canonical order: as → timeout → should_fail)".to_string(),
            });
        }
        if rest.starts_with("timeout") && (rest.len() == 7 || rest[7..].starts_with(|c: char| c.is_whitespace())) {
            return Err(ParseError::Parse {
                line,
                message: "test: modifier `timeout` must precede `should_fail` in test_step \
                          (canonical order: as → timeout → should_fail)".to_string(),
            });
        }
    }

    // ── Reject anything remaining ────────────────────────────────────
    if !rest.is_empty() {
        return Err(ParseError::Parse {
            line,
            message: format!("test: unexpected modifier `{}`", rest.split_whitespace().next().unwrap_or(rest)),
        });
    }

    Ok(TestModifierTail { as_name, timeout, should_fail })
}

/// Check that no `as` modifier follows the closing `}` of a cook or plate
/// step body.  Returns an error if `as` is found; the modifier is only valid
/// on `test_step` (CS-0061 §3.1.3).
fn reject_as_modifier_on_non_test(
    line_text: &str,
    step_keyword: &str,
    line: usize,
) -> Result<(), ParseError> {
    let suffix = match line_text.rfind('}') {
        Some(idx) => &line_text[idx + 1..],
        None => "",
    };
    let trimmed = suffix.trim();
    if trimmed.starts_with("as") && (trimmed.len() == 2 || trimmed[2..].starts_with(|c: char| c.is_whitespace() || c == '\'')) {
        return Err(ParseError::Parse {
            line,
            message: format!(
                "{}: modifier `as` is only valid on test_step (CS-0061 §3.1.3)",
                step_keyword
            ),
        });
    }
    Ok(())
}

/// §8.4.3 rule 1: a disposition decorator line must be immediately followed by
/// a `cook` step. `line` is the first pending decorator's source line.
fn dangling_decorator(line: usize) -> ParseError {
    ParseError::Parse {
        line,
        message: "disposition line must be immediately followed by a `cook` step".to_string(),
    }
}

/// §8.4.3: parse a disposition block (`seal … { … }`, `local { }`, `pinned { }`,
/// `record { }`). `opener` is the classified block-opener line at `open_pos`
/// (source line `open_line`); `inherited` is the disposition contributed by any
/// enclosing block(s). Folds the opener onto a clone of `inherited` to form the
/// block disposition, then parses enclosed items until the matching `}` line,
/// applying that disposition (additively unioned with each cook's own decorator
/// lines) onto every enclosed `cook` step. Recurses on nested openers. Returns
/// the token-stream position just past the closing `}` line.
fn parse_disposition_block(
    opener: DispLine,
    steps: &mut Vec<Step>,
    open_line: usize,
    tokens: &[Located<Token>],
    open_pos: usize,
    source_lines: &[&str],
    inherited: Disposition,
) -> Result<usize, ParseError> {
    // Build this block's disposition by folding the opener onto `inherited`.
    let mut block_disp = inherited;
    match &opener {
        DispLine::SealBlockOpen(refs) => {
            fold_decorator(&mut block_disp, &DispLine::Seal(refs.clone()), open_line)?;
        }
        DispLine::LocalBlockOpen => {
            fold_decorator(&mut block_disp, &DispLine::Local, open_line)?;
        }
        DispLine::PinnedBlockOpen => {
            fold_decorator(&mut block_disp, &DispLine::Pinned, open_line)?;
        }
        DispLine::RecordBlockOpen => {
            fold_decorator(&mut block_disp, &DispLine::Record, open_line)?;
        }
        _ => {
            return Err(ParseError::Parse {
                line: open_line,
                message: "internal: parse_disposition_block called on a non-opener".to_string(),
            });
        }
    }

    let mut pos = open_pos + 1; // step past the opener line's token
    let mut saw_cook = false;
    // Decorator lines accumulated for the next cook inside this block.
    let mut pending = block_disp.clone();
    let mut pending_active = false;
    let mut pending_line = 0usize;

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::Comment(_) | Token::Blank => {
                pos += 1;
            }
            Token::Content(text) => {
                let trimmed = text.trim();
                // Closing brace terminates the block.
                if trimmed == "}" {
                    if pending_active {
                        return Err(dangling_decorator(pending_line));
                    }
                    if !saw_cook {
                        return Err(ParseError::Parse {
                            line: open_line,
                            message: "disposition block must contain at least one `cook` step"
                                .to_string(),
                        });
                    }
                    return Ok(pos + 1);
                }

                if let Some(disp) = classify_disposition_line(text) {
                    match disp {
                        DispLine::SealBlockOpen(_)
                        | DispLine::LocalBlockOpen
                        | DispLine::PinnedBlockOpen
                        | DispLine::RecordBlockOpen => {
                            if pending_active {
                                return Err(dangling_decorator(pending_line));
                            }
                            // Nested block inherits THIS block's disposition.
                            let new_pos = parse_disposition_block(
                                disp,
                                steps,
                                tok.line,
                                tokens,
                                pos,
                                source_lines,
                                block_disp.clone(),
                            )?;
                            saw_cook = true; // rule 11 satisfied transitively
                            pos = new_pos;
                            continue;
                        }
                        decorator => {
                            if !pending_active {
                                pending_line = tok.line;
                                pending_active = true;
                            }
                            fold_decorator(&mut pending, &decorator, tok.line)?;
                            pos += 1;
                            continue;
                        }
                    }
                }

                if let Some(rest) = strip_keyword(text, "cook") {
                    let (mut cook_step, new_pos) =
                        parse_cook_line(rest, tok.line, tokens, pos, source_lines)?;
                    cook_step.disposition = pending.clone();
                    // Reset the decorator accumulator to the block baseline.
                    pending = block_disp.clone();
                    pending_active = false;
                    let close_line_text = if new_pos > 0 {
                        match tokens.get(new_pos - 1) {
                            Some(t) => source_lines
                                .get(t.line.saturating_sub(1))
                                .copied()
                                .unwrap_or(""),
                            None => "",
                        }
                    } else {
                        source_lines
                            .get(tok.line.saturating_sub(1))
                            .copied()
                            .unwrap_or("")
                    };
                    reject_as_modifier_on_non_test(close_line_text, "cook", tok.line)?;
                    steps.push(Step::Cook {
                        step: cook_step,
                        line: tok.line,
                    });
                    saw_cook = true;
                    pos = new_pos;
                    continue;
                }

                // Rule 11: only cook steps / nested blocks / decorators are
                // permitted inside a disposition block.
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: format!(
                        "disposition block admits only `cook` steps and nested disposition blocks, found: {trimmed}"
                    ),
                });
            }
            // Any non-Content token (Lua lines/blocks, top-level headers) is
            // not a permitted disposition-block item.
            _ => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "disposition block admits only `cook` steps and nested disposition blocks".to_string(),
                });
            }
        }
    }

    // Reached EOF without a closing `}`.
    Err(ParseError::Parse {
        line: open_line,
        message: "unterminated disposition block (missing closing `}`)".to_string(),
    })
}

pub(crate) fn parse_recipe(
    name: String,
    deps: Vec<String>,
    recipe_line: usize,
    tokens: &[Located<Token>],
    start: usize,
    source_lines: &[&str],
) -> Result<(Recipe, usize), ParseError> {
    let mut pos = start;
    let mut ingredients = Vec::new();
    let mut excludes: Vec<String> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    // §{steps.ingredients}: glob-pattern `ingredients` and `ingredients <probe>`
    // (probe member source) are mutually exclusive within a recipe, and at most
    // one probe source is allowed per recipe.
    let mut for_each_seen = false;

    // Track the line on which the imperative region began (the first
    // imperative-region step). None until we see one. App. A.3 "Region
    // ordering rule" / §{recipes.step-kinds} Note 4.4.2: once set, no
    // declarative-region step may follow.
    let mut imperative_began: Option<usize> = None;

    let region_violation = |kind: &str, step_line: usize, started_line: usize| ParseError::Parse {
        line: step_line,
        message: format!(
            "{} step on line {} is not allowed after the imperative region began on line {}",
            kind, step_line, started_line
        ),
    };

    // §8.4.3: accumulated decorator-line disposition awaiting the next `cook`.
    // `pending_active` is set the moment a decorator line is seen and cleared
    // when a `cook` consumes it; a non-cook step (or recipe end) while active
    // is a dangling-decorator error.
    let mut pending = Disposition::default();
    let mut pending_active = false;
    let mut pending_line: usize = 0;

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            // CS-0019: implicit termination — the next column-0 top-level
            // keyword closes the body. The token is left in place so that
            // parse() dispatches it as the next toplevel_item.
            Token::RecipeHeader { .. }
            | Token::ChoreHeader { .. }
            | Token::ConfigHeader { .. }
            | Token::UseDecl { .. }
            | Token::ImportDecl { .. }
            | Token::RegisterHeader
            | Token::ProbeHeader { .. } => {
                if pending_active {
                    return Err(dangling_decorator(pending_line));
                }
                return Ok((
                    Recipe {
                        name,
                        deps,
                        ingredients,
                        excludes,
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
                // CS-0072: a column-0 Content token matching the module-call
                // shape is a top-level module_call, not a recipe step.
                // Terminate the recipe body and leave the token for parse()
                // to dispatch as a top-level module_call.
                if is_module_call(text) {
                    let raw = source_lines
                        .get(tok.line.saturating_sub(1))
                        .copied()
                        .unwrap_or("");
                    if !raw.starts_with(|c: char| c.is_whitespace()) {
                        if pending_active {
                            return Err(dangling_decorator(pending_line));
                        }
                        return Ok((
                            Recipe {
                                name,
                                deps,
                                ingredients,
                                excludes,
                                steps,
                                line: recipe_line,
                            },
                            pos,
                        ));
                    }
                }
                // §8.4.3: disposition decorator lines and block openers. A
                // decorator line accumulates onto `pending` for the next
                // `cook`; a block opener recurses into `parse_disposition_block`.
                if let Some(disp) = classify_disposition_line(text) {
                    if let Some(started) = imperative_began {
                        return Err(region_violation("disposition", tok.line, started));
                    }
                    match disp {
                        DispLine::SealBlockOpen(_)
                        | DispLine::LocalBlockOpen
                        | DispLine::PinnedBlockOpen
                        | DispLine::RecordBlockOpen => {
                            if pending_active {
                                // A decorator line directly above a block opener
                                // has no `cook` to attach to.
                                return Err(dangling_decorator(pending_line));
                            }
                            let new_pos = parse_disposition_block(
                                disp,
                                &mut steps,
                                tok.line,
                                tokens,
                                pos,
                                source_lines,
                                Disposition::default(),
                            )?;
                            pos = new_pos;
                            continue;
                        }
                        decorator => {
                            if !pending_active {
                                pending_line = tok.line;
                                pending_active = true;
                            }
                            fold_decorator(&mut pending, &decorator, tok.line)?;
                            pos += 1;
                            continue;
                        }
                    }
                }
                // §8.4.3 rule 1: a pending decorator run must be immediately
                // followed by a `cook`. Any other recipe-body Content line
                // here (this token is not a disposition line) is a dangling
                // decorator unless it is the `cook` keyword.
                if pending_active && strip_keyword(text, "cook").is_none() {
                    return Err(dangling_decorator(pending_line));
                }
                if let Some(rest) = strip_keyword(text, "ingredients") {
                    if let Some(started) = imperative_began {
                        return Err(region_violation("ingredients", tok.line, started));
                    }
                    if for_each_seen {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message:
                                "a recipe may declare at most one `ingredients <probe>` source"
                                    .to_string(),
                        });
                    }
                    let head = rest.trim_start();
                    if head.starts_with('"') || head.starts_with('!') {
                        // Glob ingredients (existing path).
                        if !ingredients.is_empty() || !excludes.is_empty() {
                            return Err(ParseError::Parse {
                                line: tok.line,
                                message: "duplicate 'ingredients' line".to_string(),
                            });
                        }
                        let (inc, exc, new_pos) =
                            parse_ingredients_line(rest, tok.line, tokens, pos, source_lines)?;
                        ingredients = inc;
                        excludes = exc;
                        pos = new_pos;
                        continue;
                    } else {
                        // COOK-88: bare identifier => probe member source. Desugar to ForEach.
                        if !ingredients.is_empty() || !excludes.is_empty() {
                            return Err(ParseError::Parse {
                                line: tok.line,
                                message: "ingredients: cannot mix glob patterns with a probe source"
                                    .to_string(),
                            });
                        }
                        let (fe, new_pos) =
                            crate::cook_line::parse_ingredients_probe_source(rest, tok.line, tokens, pos)?;
                        for_each_seen = true;
                        steps.push(Step::ForEach { step: fe, line: tok.line });
                        pos = new_pos;
                        continue;
                    }
                } else if let Some(rest) = strip_keyword(text, "cook") {
                    if let Some(started) = imperative_began {
                        return Err(region_violation("cook", tok.line, started));
                    }
                    let (mut cook_step, new_pos) =
                        parse_cook_line(rest, tok.line, tokens, pos, source_lines)?;
                    // §8.4.3: fold any accumulated decorator-line disposition
                    // onto this cook, then clear it.
                    cook_step.disposition = std::mem::take(&mut pending);
                    pending_active = false;
                    // Reject `as` modifier on cook_step (CS-0061 §3.1.3).
                    let close_line_text = if new_pos > 0 {
                        match tokens.get(new_pos - 1) {
                            Some(t) => source_lines.get(t.line.saturating_sub(1)).copied().unwrap_or(""),
                            None => "",
                        }
                    } else {
                        source_lines.get(tok.line.saturating_sub(1)).copied().unwrap_or("")
                    };
                    reject_as_modifier_on_non_test(close_line_text, "cook", tok.line)?;
                    steps.push(Step::Cook {
                        step: cook_step,
                        line: tok.line,
                    });
                    pos = new_pos;
                    continue;
                } else if let Some(rest) = strip_keyword(text, "plate") {
                    if let Some(started) = imperative_began {
                        return Err(region_violation("plate", tok.line, started));
                    }
                    let (body, new_pos) = crate::cook_line::parse_body_payload(
                        rest, tok.line, tokens, pos, source_lines, "plate",
                    )?;
                    // Reject `as` modifier on plate_step (CS-0061 §3.1.3).
                    let close_line_text = if new_pos > 0 {
                        match tokens.get(new_pos - 1) {
                            Some(t) => source_lines.get(t.line.saturating_sub(1)).copied().unwrap_or(""),
                            None => "",
                        }
                    } else {
                        source_lines.get(tok.line.saturating_sub(1)).copied().unwrap_or("")
                    };
                    reject_as_modifier_on_non_test(close_line_text, "plate", tok.line)?;
                    steps.push(Step::Plate {
                        step: PlateStep { body },
                        line: tok.line,
                    });
                    pos = new_pos;
                    continue;
                } else if let Some(rest) = strip_keyword(text, "test") {
                    if let Some(started) = imperative_began {
                        return Err(region_violation("test", tok.line, started));
                    }
                    let (body, new_pos) = crate::cook_line::parse_body_payload(
                        rest, tok.line, tokens, pos, source_lines, "test",
                    )?;
                    let modifier_line = if new_pos > 0 && new_pos <= tokens.len() {
                        match tokens.get(new_pos - 1) {
                            Some(t) => source_lines[t.line.saturating_sub(1)],
                            None => "",
                        }
                    } else {
                        ""
                    };
                    let modifier_tail = parse_test_modifier_tail(modifier_line, tok.line)?;
                    steps.push(Step::Test {
                        step: TestStep {
                            body,
                            as_name: modifier_tail.as_name,
                            timeout: modifier_tail.timeout,
                            should_fail: modifier_tail.should_fail,
                        },
                        line: tok.line,
                    });
                    pos = new_pos;
                    continue;
                } else if let Some(cmd) = text.strip_prefix('@') {
                    let cmd = cmd.to_string();
                    if cmd.is_empty() {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message: "interactive '@' prefix requires a command".to_string(),
                        });
                    }
                    if imperative_began.is_none() {
                        imperative_began = Some(tok.line);
                    }
                    steps.push(Step::Shell {
                        command: cmd,
                        line: tok.line,
                        interactive: true,
                    });
                } else {
                    // SHI-216 / CS-0072 §3.9: reject `register` + separator inside a recipe body.
                    // An indented `register <args>` cannot be a RegisterHeader (column-0 only
                    // per §2.10) and is also not a permitted shell command in this position.
                    // The bare `register` identifier (trimmed == "register", no separator)
                    // remains a shell_command per the post-CS-0072 rule 6.
                    {
                        let trimmed = text.trim();
                        if trimmed.starts_with("register")
                            && trimmed.len() > 8
                            && {
                                let b = trimmed.as_bytes()[8];
                                b == b' ' || b == b'\t'
                            }
                        {
                            return Err(ParseError::Parse {
                                line: tok.line,
                                message:
                                    "`register` blocks are top-level only; move this outside the recipe body"
                                        .to_string(),
                            });
                        }
                    }
                    if imperative_began.is_none() {
                        imperative_began = Some(tok.line);
                    }
                    steps.push(Step::Shell {
                        command: text.clone(),
                        line: tok.line,
                        interactive: false,
                    });
                }
                pos += 1;
            }
            Token::LuaLine(code) => {
                if pending_active {
                    return Err(dangling_decorator(pending_line));
                }
                if imperative_began.is_none() {
                    imperative_began = Some(tok.line);
                }
                steps.push(Step::Lua {
                    code: code.clone(),
                    line: tok.line,
                });
                pos += 1;
            }
            Token::LuaBlockOpen => {
                if pending_active {
                    return Err(dangling_decorator(pending_line));
                }
                if imperative_began.is_none() {
                    imperative_began = Some(tok.line);
                }
                let block_line = tok.line;
                pos += 1;
                let (code, new_pos) = collect_lua_block(block_line, tokens, pos, source_lines)?;
                steps.push(Step::LuaBlock {
                    code,
                    line: block_line,
                });
                pos = new_pos;
            }
            Token::InlineLuaLine(code) => {
                if pending_active {
                    return Err(dangling_decorator(pending_line));
                }
                if let Some(started) = imperative_began {
                    return Err(region_violation("inline-lua (`>>`)", tok.line, started));
                }
                steps.push(Step::InlineLua {
                    code: code.clone(),
                    line: tok.line,
                });
                pos += 1;
            }
            Token::InlineLuaBlockOpen => {
                if pending_active {
                    return Err(dangling_decorator(pending_line));
                }
                if let Some(started) = imperative_began {
                    return Err(region_violation("inline-lua-block (`>>{`)", tok.line, started));
                }
                let block_line = tok.line;
                pos += 1;
                let (code, new_pos) = collect_lua_block(block_line, tokens, pos, source_lines)?;
                steps.push(Step::InlineLuaBlock {
                    code,
                    line: block_line,
                });
                pos = new_pos;
            }
        }
    }

    // §8.4.3 rule 1: a decorator run at end-of-recipe never reached a `cook`.
    if pending_active {
        return Err(dangling_decorator(pending_line));
    }

    // CS-0019: EOF terminates a body. No "missing end" error in v0.4.
    Ok((
        Recipe {
            name,
            deps,
            ingredients,
            excludes,
            steps,
            line: recipe_line,
        },
        pos,
    ))
}

pub(crate) fn parse_chore(
    name: String,
    params: Vec<ChoreParam>,
    deps: Vec<String>,
    chore_line: usize,
    tokens: &[Located<Token>],
    start: usize,
    source_lines: &[&str],
) -> Result<(Chore, usize), ParseError> {
    let mut pos = start;
    let mut steps: Vec<Step> = Vec::new();
    let mut imperative_began: Option<usize> = None;

    let region_violation = |kind: &str, step_line: usize, started_line: usize| ParseError::Parse {
        line: step_line,
        message: format!(
            "{} step on line {} is not allowed after the imperative region began on line {}",
            kind, step_line, started_line
        ),
    };

    let chore_banned = |keyword: &str, line: usize| -> ParseError {
        let kind_descriptor = match keyword {
            "ingredients" => "inputs",
            "cook"        => "outputs",
            "plate"       => "plated outputs",
            "test"        => "tested outputs",
            _             => "targets",
        };
        ParseError::Parse {
            line,
            message: format!(
                "'{}' is not allowed in a chore; use 'recipe' for build {}",
                keyword, kind_descriptor
            ),
        }
    };

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            // CS-0019/0020 implicit termination: any column-0 top-level
            // keyword closes the chore body. Token left in place for parse().
            Token::RecipeHeader { .. }
            | Token::ChoreHeader { .. }
            | Token::ConfigHeader { .. }
            | Token::UseDecl { .. }
            | Token::ImportDecl { .. }
            | Token::RegisterHeader
            | Token::ProbeHeader { .. } => {
                return Ok((
                    Chore { name, params, deps, steps, line: chore_line },
                    pos,
                ));
            }
            Token::Comment(_) | Token::Blank => {
                pos += 1;
            }
            Token::Content(text) => {
                // CS-0072: column-0 Content matching the module-call shape
                // terminates the chore body; token left for parse() dispatch.
                if is_module_call(text) {
                    let raw = source_lines
                        .get(tok.line.saturating_sub(1))
                        .copied()
                        .unwrap_or("");
                    if !raw.starts_with(|c: char| c.is_whitespace()) {
                        return Ok((
                            Chore { name, params, deps, steps, line: chore_line },
                            pos,
                        ));
                    }
                }
                let text = text.clone();
                if strip_keyword(&text, "ingredients").is_some() {
                    return Err(chore_banned("ingredients", tok.line));
                } else if strip_keyword(&text, "cook").is_some() {
                    return Err(chore_banned("cook", tok.line));
                } else if strip_keyword(&text, "plate").is_some() {
                    return Err(chore_banned("plate", tok.line));
                } else if strip_keyword(&text, "test").is_some() {
                    return Err(chore_banned("test", tok.line));
                } else if let Some(cmd) = text.strip_prefix('@') {
                    let cmd = cmd.to_string();
                    if cmd.is_empty() {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message: "interactive '@' prefix requires a command".to_string(),
                        });
                    }
                    if imperative_began.is_none() {
                        imperative_began = Some(tok.line);
                    }
                    // Default-interactive — `@` marker is no-op (always interactive)
                    steps.push(Step::Shell {
                        command: cmd,
                        line: tok.line,
                        interactive: true,
                    });
                } else {
                    // SHI-216 / CS-0072 §3.9: reject `register` + separator inside a chore body.
                    // The bare `register` identifier (no separator) remains a shell_command per
                    // the post-CS-0072 rule 6.
                    {
                        let trimmed = text.trim();
                        if trimmed.starts_with("register")
                            && trimmed.len() > 8
                            && {
                                let b = trimmed.as_bytes()[8];
                                b == b' ' || b == b'\t'
                            }
                        {
                            return Err(ParseError::Parse {
                                line: tok.line,
                                message:
                                    "`register` blocks are top-level only; move this outside the chore body"
                                        .to_string(),
                            });
                        }
                    }
                    if imperative_began.is_none() {
                        imperative_began = Some(tok.line);
                    }
                    // Default-interactive — no `@` required
                    steps.push(Step::Shell {
                        command: text.clone(),
                        line: tok.line,
                        interactive: true,
                    });
                }
                pos += 1;
            }
            Token::LuaLine(code) => {
                if imperative_began.is_none() {
                    imperative_began = Some(tok.line);
                }
                steps.push(Step::Lua { code: code.clone(), line: tok.line });
                pos += 1;
            }
            Token::LuaBlockOpen => {
                if imperative_began.is_none() {
                    imperative_began = Some(tok.line);
                }
                let block_line = tok.line;
                pos += 1;
                let (code, new_pos) = collect_lua_block(block_line, tokens, pos, source_lines)?;
                steps.push(Step::LuaBlock { code, line: block_line });
                pos = new_pos;
            }
            Token::InlineLuaLine(code) => {
                if let Some(started) = imperative_began {
                    return Err(region_violation("inline-lua (`>>`)", tok.line, started));
                }
                steps.push(Step::InlineLua { code: code.clone(), line: tok.line });
                pos += 1;
            }
            Token::InlineLuaBlockOpen => {
                if let Some(started) = imperative_began {
                    return Err(region_violation("inline-lua-block (`>>{`)", tok.line, started));
                }
                let block_line = tok.line;
                pos += 1;
                let (code, new_pos) = collect_lua_block(block_line, tokens, pos, source_lines)?;
                steps.push(Step::InlineLuaBlock { code, line: block_line });
                pos = new_pos;
            }
        }
    }

    // EOF terminates
    Ok((Chore { name, params, deps, steps, line: chore_line }, pos))
}
