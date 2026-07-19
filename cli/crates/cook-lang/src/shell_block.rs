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
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn run(src: &str) -> Result<(Vec<String>, String), ParseError> {
        let source_lines: Vec<&str> = src.lines().collect();
        let tokens = tokenize(src).expect("tokenize");
        // The first line of src is the `{` opener; pass its remainder.
        let after_open = source_lines[0]
            .split_once('{')
            .map(|(_, rest)| rest)
            .unwrap_or("");
        let (cmds, tail, _) = collect_shell_block(1, after_open, &tokens, 0, &source_lines)?;
        Ok((cmds, tail))
    }

    #[test]
    fn collects_three_lines() {
        let src = "{\n    wasm-pack build\n    cp a b\n    cp c d\n}\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(cmds, vec!["wasm-pack build", "cp a b", "cp c d"]);
    }

    #[test]
    fn drops_blank_lines() {
        let src = "{\n    line1\n\n    line2\n}\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(cmds, vec!["line1", "line2"]);
    }

    #[test]
    fn rejects_unclosed_block() {
        let src = "{\n    line1\n";
        let err = run(src).expect_err("should fail");
        match err {
            ParseError::Parse { message, .. } => assert!(message.contains("unclosed")),
            _ => panic!("wrong error"),
        }
    }

    #[test]
    fn respects_nested_braces_in_content() {
        // lines containing balanced braces don't prematurely close the block.
        let src = "{\n    echo \"hello { world }\"\n    line2\n}\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(cmds.len(), 2);
    }

    // ── CS-0022 Phase G Item 5 (reworked for CS-0154's unified span walk) ──

    #[test]
    fn cs_0022_inline_block_single_command() {
        let (cmds, _) = run("{ wasm-pack build }\n").expect("ok");
        assert_eq!(cmds, vec!["wasm-pack build".to_string()]);
    }

    #[test]
    fn cs_0022_inline_block_empty() {
        let (cmds, _) = run("{ }\n").expect("ok");
        assert_eq!(cmds, Vec::<String>::new());
    }

    #[test]
    fn cs_0022_inline_block_with_inner_braces() {
        let (cmds, _) = run("{ gcc {in} -o {out} }\n").expect("ok");
        assert_eq!(cmds, vec!["gcc {in} -o {out}".to_string()]);
    }

    #[test]
    fn cs_0022_inline_block_no_close_collects_multiline() {
        // No close on the opening line → the remainder is the first segment
        // and collection continues (unclosed here, so an error).
        let err = run("{ wasm-pack build\n").expect_err("unclosed");
        match err {
            ParseError::Parse { message, .. } => assert!(message.contains("unclosed")),
            _ => panic!("wrong error"),
        }
    }

    // ── CS-0035: heredoc state carries across shell-block lines ──

    #[test]
    fn cs_0035_heredoc_with_brace_inside_body() {
        // The `}` on line 3 is heredoc body, not the block close.
        let src = "{\n    cat <<EOF\n    } not a closer\n    EOF\n    echo done\n}\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(cmds.len(), 4);
        assert_eq!(cmds[0], "cat <<EOF");
        assert_eq!(cmds[1], "} not a closer");
        assert_eq!(cmds[2], "EOF");
        assert_eq!(cmds[3], "echo done");
    }

    #[test]
    fn cs_0035_heredoc_quoted_delim() {
        let src = "{\n    cat <<'END'\n    } literal\n    END\n}\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(cmds, vec!["cat <<'END'", "} literal", "END"]);
    }

    #[test]
    fn cs_0035_heredoc_dash_form() {
        let src = "{\n    cat <<-EOF\n\t} body\n\tEOF\n}\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(cmds.len(), 3);
    }

    // ── CS-0154: the body is the character span between the braces ──

    #[test]
    fn cs_0154_open_line_remainder_is_body() {
        let src = "{ echo start\n    echo middle\n}\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(cmds, vec!["echo start", "echo middle"]);
    }

    #[test]
    fn cs_0154_close_line_prefix_is_body() {
        let src = "{\n    echo start\n    echo end }\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(cmds, vec!["echo start", "echo end"]);
    }

    #[test]
    fn cs_0154_multiline_single_quote_carries() {
        // The single-quoted string spans lines; its braces are data, and the
        // `]' }` close line carries the string's close quote as body.
        let src = "{ echo '[\n    {\"name\": \"web\"},\n    {\"name\": \"desktop\"}\n]' }\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(
            cmds,
            vec![
                "echo '[",
                "{\"name\": \"web\"},",
                "{\"name\": \"desktop\"}",
                "]'"
            ]
        );
    }

    #[test]
    fn cs_0154_multiline_double_quote_carries() {
        let src = "{ echo \"a {\n b }\" }\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(cmds, vec!["echo \"a {", "b }\""]);
    }

    #[test]
    fn cs_0154_heredoc_opened_on_open_line() {
        // The heredoc opener sits on the block's opening line; its body lines
        // (including brace-bearing ones) are opaque until the delimiter.
        let src = "{ cat <<'J'\n{\"not\": \"a closer\"}\nJ\n}\n";
        let (cmds, _) = run(src).expect("ok");
        assert_eq!(cmds, vec!["cat <<'J'", "{\"not\": \"a closer\"}", "J"]);
    }

    #[test]
    fn cs_0154_inline_quoted_close_brace() {
        // Latent pre-CS-0154 inline bug: the quote-naive scanner closed the
        // block at the quoted `}`.
        let (cmds, _) = run("{ echo '}' }\n").expect("ok");
        assert_eq!(cmds, vec!["echo '}'"]);
    }

    #[test]
    fn cs_0154_trailer_after_close_is_returned() {
        // The post-close text is the enclosing production's trailer: a cook
        // step parses its modifier tail from it; probe producers and chore
        // Lua blocks reject stray text via `reject_stray_tail`.
        let (cmds, tail) = run("{ echo hi } nondet\n").expect("ok");
        assert_eq!(cmds, vec!["echo hi"]);
        assert_eq!(tail, "nondet");
        let (cmds, tail) = run("{\n    echo hi\n} local\n").expect("ok");
        assert_eq!(cmds, vec!["echo hi"]);
        assert_eq!(tail, "local");
        assert!(reject_stray_tail("stray", 3, "probe").is_err());
        assert!(reject_stray_tail("", 3, "probe").is_ok());
    }
}
