use crate::lexer::{Located, Token};
use crate::lua_block::count_brace_delta;
use crate::ParseError;

/// Collects raw source lines for a plain-shell block, tracking brace depth.
/// Starts after the `{` token (the opening brace is on `open_line`, 1-indexed).
/// Returns the canonical list of commands: per-line trim, blank lines dropped.
/// Returns how many tokens to skip past the closing brace line.
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

    while line_idx < source_lines.len() {
        let raw_line = source_lines[line_idx];
        depth += count_brace_delta(raw_line);
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
}
