use crate::brace_scan::ShellScanner;
use crate::lexer::{Located, Token};
use crate::ParseError;

/// Collects a shell block's body as the character span between the opening
/// `{` and its matching `}` (§3.9, CS-0154).
///
/// `after_open` is the remainder of the opening line — the text immediately
/// following the `{` — and is the block's first body segment: a heredoc or
/// quote opened there carries into the following lines, and content before an
/// inline close (`{ cmd }`) is the whole body. Subsequent segments are full
/// source lines; the final segment is the closing line's prefix (the text
/// before the matching `}`).
///
/// Brace counting uses a stateful [`ShellScanner`] that ignores braces inside
/// single- and double-quoted strings (which span lines, CS-0154) and inside
/// POSIX heredocs (`<<TAG`, `<<-TAG`, `<<'TAG'`, `<<"TAG"`) whose state
/// carries across lines so that a `}` byte inside string or heredoc content is
/// treated as data rather than the block's closing delimiter (CS-0035).
///
/// Per App. A.5 each segment is trimmed and blank segments are dropped; the
/// result is the ordered command list.
///
/// Returns the command list, the trimmed text following the closing `}` on
/// its line (the enclosing production's trailer — a cook/test step admits its
/// modifier tail there, every other context rejects stray text), and the
/// token position past the closing line.
pub(crate) fn collect_shell_block(
    open_line: usize,
    after_open: &str,
    tokens: &[Located<Token>],
    token_pos: usize,
    source_lines: &[&str],
) -> Result<(Vec<String>, String, usize), ParseError> {
    let mut scanner = ShellScanner::new();
    let mut depth: i32 = 1;
    let mut commands: Vec<String> = Vec::new();

    let push_segment = |commands: &mut Vec<String>, segment: &str| {
        let trimmed = segment.trim();
        if !trimmed.is_empty() {
            commands.push(trimmed.to_string());
        }
    };

    // Opening-line remainder: the first body segment (CS-0154).
    if let Some(close) = scanner.scan_to_close(after_open, &mut depth) {
        push_segment(&mut commands, &after_open[..close]);
        let tail = after_open[close + 1..].trim().to_string();
        return Ok((commands, tail, skip_past_line(tokens, token_pos, open_line)));
    }
    push_segment(&mut commands, after_open);

    // Interior lines, then the closing line (whose pre-brace prefix is body).
    let mut line_idx = open_line; // 0-indexed line after the opening line
    while line_idx < source_lines.len() {
        let raw_line = source_lines[line_idx];
        if let Some(close) = scanner.scan_to_close(raw_line, &mut depth) {
            let close_line = line_idx + 1; // 1-indexed
            push_segment(&mut commands, &raw_line[..close]);
            let tail = raw_line[close + 1..].trim().to_string();
            return Ok((commands, tail, skip_past_line(tokens, token_pos, close_line)));
        }
        push_segment(&mut commands, raw_line);
        line_idx += 1;
    }

    Err(ParseError::Parse {
        line: open_line,
        message: "unclosed shell block (missing closing '}')".to_string(),
    })
}

/// Reject stray text after a block's closing `}` (§3.9, CS-0154) in contexts
/// that admit no trailer (probe producers, chore Lua blocks). Cook/test steps
/// instead parse their modifier tail from the returned trailer.
pub(crate) fn reject_stray_tail(tail: &str, line: usize, context: &str) -> Result<(), ParseError> {
    if tail.is_empty() {
        return Ok(());
    }
    Err(ParseError::Parse {
        line,
        message: format!(
            "{context}: unexpected text after the closing '}}': `{tail}`"
        ),
    })
}

/// Advance the token position past every token on or before `close_line`
/// (1-indexed), i.e. past the whole consumed block.
pub(crate) fn skip_past_line(
    tokens: &[Located<Token>],
    token_pos: usize,
    close_line: usize,
) -> usize {
    let mut new_pos = token_pos;
    while new_pos < tokens.len() && tokens[new_pos].line <= close_line {
        new_pos += 1;
    }
    new_pos
}

#[cfg(test)]
#[path = "tests/shell_block_tests.rs"]
mod tests;
