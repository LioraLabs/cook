use crate::lexer::*;
use crate::ParseError;

/// Collects raw source lines for a Lua block, tracking brace depth.
/// Starts after the `>{` token. Reads raw source lines (not tokens) to preserve
/// original formatting. Tracks brace depth, ignoring braces inside strings and comments.
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

    while line_idx < source_lines.len() {
        let raw_line = source_lines[line_idx];
        // Update depth based on this line
        depth += count_brace_delta(raw_line);

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

/// Counts the net brace delta ({  = +1, } = -1) on a line,
/// ignoring braces inside double-quoted strings, single-quoted strings,
/// long strings ([[...]]), and after -- Lua line comments.
pub(crate) fn count_brace_delta(line: &str) -> i32 {
    let mut delta = 0;
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];

        // Check for Lua line comment
        if c == '-' && i + 1 < len && chars[i + 1] == '-' {
            // Rest of line is a comment — stop processing
            break;
        }

        // Check for long string [[ ... ]]
        if c == '[' && i + 1 < len && chars[i + 1] == '[' {
            i += 2;
            // Skip until ]]
            while i + 1 < len {
                if chars[i] == ']' && chars[i + 1] == ']' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // Check for double-quoted string
        if c == '"' {
            i += 1;
            while i < len && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1; // skip escaped character
                }
                i += 1;
            }
            if i < len {
                i += 1; // skip closing quote
            }
            continue;
        }

        // Check for single-quoted string
        if c == '\'' {
            i += 1;
            while i < len && chars[i] != '\'' {
                if chars[i] == '\\' {
                    i += 1; // skip escaped character
                }
                i += 1;
            }
            if i < len {
                i += 1; // skip closing quote
            }
            continue;
        }

        if c == '{' {
            delta += 1;
        } else if c == '}' {
            delta -= 1;
        }

        i += 1;
    }

    delta
}
