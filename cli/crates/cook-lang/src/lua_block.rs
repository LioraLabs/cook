use crate::brace_scan::LuaScanner;
use crate::lexer::*;
use crate::shell_block::skip_past_line;
use crate::ParseError;

/// Collects a Lua block's body as the character span between the opening
/// `>{` and its matching `}` (§3.9, CS-0154).
///
/// `after_open` is the remainder of the opening line — the text immediately
/// following the `>{` — and is the block's first body segment (`>{ return {`
/// keeps its `return {`). Subsequent segments are full raw source lines,
/// preserving original whitespace; the final segment is the closing line's
/// prefix (the text before the matching `}`). A boundary segment that is
/// empty or whitespace-only is dropped, so the canonical `>{`-alone /
/// `}`-alone layout assembles the same chunk as before CS-0154.
///
/// Brace counting uses the stateful [`LuaScanner`]: braces inside strings,
/// line comments, multi-line long strings (`[==[ … ]==]`), and multi-line
/// block comments (`--[==[ … ]==]`) are data, with long-bracket state carried
/// across lines (CS-0035).
///
/// Returns the assembled chunk, the trimmed text following the closing `}`
/// on its line (the enclosing production's trailer — a cook/test step admits
/// its modifier tail there, every other context rejects stray text), and the
/// token position past the closing line.
pub(crate) fn collect_lua_block(
    open_line: usize,
    after_open: &str,
    tokens: &[Located<Token>],
    token_pos: usize,
    source_lines: &[&str],
) -> Result<(String, String, usize), ParseError> {
    let mut scanner = LuaScanner::new();
    let mut depth: i32 = 1;
    let mut code_lines: Vec<String> = Vec::new();

    let push_boundary = |code_lines: &mut Vec<String>, segment: &str| {
        if !segment.trim().is_empty() {
            code_lines.push(segment.to_string());
        }
    };

    // Opening-line remainder: the first body segment (CS-0154).
    if let Some(close) = scanner.scan_to_close(after_open, &mut depth) {
        push_boundary(&mut code_lines, &after_open[..close]);
        let code = code_lines.join("\n").trim().to_string();
        let tail = after_open[close + 1..].trim().to_string();
        return Ok((code, tail, skip_past_line(tokens, token_pos, open_line)));
    }
    push_boundary(&mut code_lines, after_open);

    // Interior lines (raw, whitespace preserved), then the closing line.
    let mut line_idx = open_line; // 0-indexed line after the opening line
    while line_idx < source_lines.len() {
        let raw_line = source_lines[line_idx];
        if let Some(close) = scanner.scan_to_close(raw_line, &mut depth) {
            let close_line = line_idx + 1; // 1-indexed
            push_boundary(&mut code_lines, &raw_line[..close]);
            let code = code_lines.join("\n");
            let tail = raw_line[close + 1..].trim().to_string();
            return Ok((code, tail, skip_past_line(tokens, token_pos, close_line)));
        }
        code_lines.push(raw_line.to_string());
        line_idx += 1;
    }

    Err(ParseError::Parse {
        line: open_line,
        message: "unclosed Lua block (missing closing '}')".to_string(),
    })
}
