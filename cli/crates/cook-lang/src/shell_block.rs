use crate::brace_scan::ShellScanner;
use crate::lexer::{Located, Token};
use crate::ParseError;

/// Try to extract an inline shell block from the text immediately following
/// the opening `{`.  `after_open` is the slice of the source line that comes
/// after the `{` that started the block.
///
/// Scans forward tracking brace depth (starting at 1 because the opening `{`
/// has already been consumed).  If depth reaches 0 on the same span, returns
/// the content between the opening `{` and the matching `}` (trimmed) as a
/// one-element (or zero-element) command list.
///
/// Returns `None` if the line does not contain a complete balanced block.
pub(crate) fn try_inline_shell_block(after_open: &str) -> Option<Vec<String>> {
    let mut depth: i32 = 1;
    let chars: Vec<char> = after_open.chars().collect();
    let mut byte_pos: usize = 0;
    let mut char_idx: usize = 0;

    while char_idx < chars.len() {
        let c = chars[char_idx];
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    // Found the matching close.
                    let content = &after_open[..byte_pos];
                    let trimmed = content.trim();
                    let commands: Vec<String> = if trimmed.is_empty() {
                        Vec::new()
                    } else {
                        vec![trimmed.to_string()]
                    };
                    return Some(commands);
                }
            }
            _ => {}
        }
        byte_pos += c.len_utf8();
        char_idx += 1;
    }

    None // no matching `}` on this span
}

/// Collects raw source lines for a plain-shell block, tracking brace depth.
/// Starts after the `{` token (the opening brace is on `open_line`, 1-indexed).
/// Returns the canonical list of commands: per-line trim, blank lines dropped.
/// Returns how many tokens to skip past the closing brace line.
///
/// Brace counting uses a stateful [`ShellScanner`] that ignores braces inside
/// single- and double-quoted strings and inside POSIX heredocs (`<<TAG`,
/// `<<-TAG`, `<<'TAG'`, `<<"TAG"`). Heredoc state carries across lines so
/// that a `}` byte inside heredoc body text is treated as data rather than
/// the block's closing delimiter (CS-0035).
///
/// One-line form: the caller should detect and handle inline blocks using
/// `try_inline_shell_block` before calling this function.  This function
/// handles the multi-line case only (opening `{` on its own line or without
/// a closing `}` on the same text span).
pub(crate) fn collect_shell_block(
    open_line: usize,
    tokens: &[Located<Token>],
    token_pos: usize,
    source_lines: &[&str],
) -> Result<(Vec<String>, usize), ParseError> {
    let start_source_line = open_line; // 0-indexed line after the `{`
    let mut depth: i32 = 1;
    let mut commands: Vec<String> = Vec::new();
    let mut line_idx = start_source_line;
    let mut scanner = ShellScanner::new();

    while line_idx < source_lines.len() {
        let raw_line = source_lines[line_idx];
        depth += scanner.scan_line(raw_line);
        if depth <= 0 {
            break;
        }
        let trimmed = raw_line.trim();
        if !trimmed.is_empty() {
            commands.push(trimmed.to_string());
        }
        line_idx += 1;
    }

    if depth > 0 {
        return Err(ParseError::Parse {
            line: open_line,
            message: "unclosed shell block (missing closing '}')".to_string(),
        });
    }

    let close_line_1indexed = line_idx + 1;
    let mut new_pos = token_pos;
    while new_pos < tokens.len() && tokens[new_pos].line <= close_line_1indexed {
        new_pos += 1;
    }

    Ok((commands, new_pos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn run(src: &str) -> Result<Vec<String>, ParseError> {
        let source_lines: Vec<&str> = src.lines().collect();
        let tokens = tokenize(src).expect("tokenize");
        // The first line of src should be the `{`; the block starts at line 2 (1-indexed).
        let (cmds, _) = collect_shell_block(1, &tokens, 0, &source_lines)?;
        Ok(cmds)
    }

    #[test]
    fn collects_three_lines() {
        let src = "{\n    wasm-pack build\n    cp a b\n    cp c d\n}\n";
        let cmds = run(src).expect("ok");
        assert_eq!(cmds, vec!["wasm-pack build", "cp a b", "cp c d"]);
    }

    #[test]
    fn drops_blank_lines() {
        let src = "{\n    line1\n\n    line2\n}\n";
        let cmds = run(src).expect("ok");
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
        let cmds = run(src).expect("ok");
        assert_eq!(cmds.len(), 2);
    }

    // ── CS-0022 Phase G Item 5: try_inline_shell_block unit tests ──

    #[test]
    fn cs_0022_inline_block_single_command() {
        // `after_open` is the text after the opening `{`
        let result = try_inline_shell_block(" wasm-pack build }");
        assert_eq!(result, Some(vec!["wasm-pack build".to_string()]));
    }

    #[test]
    fn cs_0022_inline_block_empty() {
        let result = try_inline_shell_block(" }");
        assert_eq!(result, Some(Vec::<String>::new()));
    }

    #[test]
    fn cs_0022_inline_block_with_inner_braces() {
        // gcc {in} -o {out} } — depth track must handle inner {}
        let result = try_inline_shell_block(" gcc {in} -o {out} }");
        assert_eq!(result, Some(vec!["gcc {in} -o {out}".to_string()]));
    }

    #[test]
    fn cs_0022_inline_block_no_close_returns_none() {
        let result = try_inline_shell_block(" wasm-pack build");
        assert_eq!(result, None);
    }

    // ── CS-0035: heredoc state carries across shell-block lines ──

    #[test]
    fn cs_0035_heredoc_with_brace_inside_body() {
        // The `}` on line 3 is heredoc body, not the block close.
        let src = "{\n    cat <<EOF\n    } not a closer\n    EOF\n    echo done\n}\n";
        let cmds = run(src).expect("ok");
        assert_eq!(cmds.len(), 4);
        assert_eq!(cmds[0], "cat <<EOF");
        assert_eq!(cmds[1], "} not a closer");
        assert_eq!(cmds[2], "EOF");
        assert_eq!(cmds[3], "echo done");
    }

    #[test]
    fn cs_0035_heredoc_quoted_delim() {
        let src = "{\n    cat <<'END'\n    } literal\n    END\n}\n";
        let cmds = run(src).expect("ok");
        assert_eq!(cmds, vec!["cat <<'END'", "} literal", "END"]);
    }

    #[test]
    fn cs_0035_heredoc_dash_form() {
        let src = "{\n    cat <<-EOF\n\t} body\n\tEOF\n}\n";
        let cmds = run(src).expect("ok");
        assert_eq!(cmds.len(), 3);
    }
}
