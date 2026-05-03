use crate::brace_scan::LuaScanner;
use crate::lexer::*;
use crate::ParseError;

/// Collects raw source lines for a Lua block, tracking brace depth.
/// Starts after the `>{` token. Reads raw source lines (not tokens) to preserve
/// original formatting. Tracks brace depth, ignoring braces inside strings,
/// line comments, multi-line long strings (`[==[ … ]==]`), and multi-line
/// block comments (`--[==[ … ]==]`). The scanner carries state across lines
/// so that a `}` byte appearing inside one of those spans is treated as data
/// rather than a block-closing delimiter (CS-0035).
pub(crate) fn collect_lua_block(
    open_line: usize,
    tokens: &[Located<Token>],
    token_pos: usize,
    source_lines: &[&str],
) -> Result<(String, usize), ParseError> {
    // open_line is 1-indexed, source_lines is 0-indexed
    // We start reading from the line after the `>{` line
    let start_source_line = open_line; // 0-indexed: open_line (1-indexed) maps to index open_line-1, next line is open_line
    let mut depth: i32 = 1;
    let mut code_lines = Vec::new();
    let mut line_idx = start_source_line; // 0-indexed line to read
    let mut scanner = LuaScanner::new();

    while line_idx < source_lines.len() {
        let raw_line = source_lines[line_idx];
        // Update depth based on this line
        depth += scanner.scan_line(raw_line);

        if depth <= 0 {
            // This line contains the closing brace; don't include it in the block
            break;
        }

        code_lines.push(raw_line);
        line_idx += 1;
    }

    if depth > 0 {
        return Err(ParseError::Parse {
            line: open_line,
            message: "unclosed Lua block (missing closing '}')".to_string(),
        });
    }

    let code = code_lines.join("\n");

    // Now we need to figure out how many tokens to skip.
    // The closing `}` line number is line_idx + 1 (1-indexed).
    let close_line_1indexed = line_idx + 1;

    // Skip all tokens whose line is <= close_line_1indexed
    let mut new_pos = token_pos;
    while new_pos < tokens.len() && tokens[new_pos].line <= close_line_1indexed {
        new_pos += 1;
    }

    Ok((code, new_pos))
}
