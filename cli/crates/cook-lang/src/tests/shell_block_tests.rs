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
