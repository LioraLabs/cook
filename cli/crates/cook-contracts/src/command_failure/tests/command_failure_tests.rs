use super::CommandFailure;
use crate::CapturedStream;

#[test]
fn wire_shape_is_marker_followed_by_compact_json() {
    let failure = CommandFailure::new(
        42,
        1,
        "false",
        CapturedStream::from_bytes(b""),
        CapturedStream::from_bytes(b"failed"),
    );
    assert_eq!(
        failure.to_wire(),
        r#"COOK_CMD_FAILED:{"line":42,"exit_code":1,"command":"false","stdout":"","stderr":"failed"}"#
    );
}

#[test]
fn parses_wire_inside_surrounding_text() {
    let parsed = CommandFailure::from_wire(
        "lua error: COOK_CMD_FAILED:{\"line\":7,\"exit_code\":2,\"command\":\"no\",\"stdout\":\"out\",\"stderr\":\"err\"}",
    )
    .expect("wrapped failure should parse");
    assert_eq!(parsed.line(), 7);
    assert_eq!(parsed.exit_code(), 2);
    assert_eq!(parsed.command(), "no");
    assert_eq!(parsed.stdout().as_str(), "out");
    assert_eq!(parsed.stderr().as_str(), "err");
}

#[test]
fn missing_or_malformed_wire_is_rejected() {
    assert!(CommandFailure::from_wire("ordinary error").is_none());
    assert!(CommandFailure::from_wire("COOK_CMD_FAILED:not json").is_none());
    assert!(CommandFailure::from_wire("COOK_CMD_FAILED:{\"line\":1}").is_none());
}

#[test]
fn every_field_round_trips_without_delimiter_ambiguity() {
    let command = "printf ':\\n\"COOK_CMD_FAILED:'";
    let stdout = "first:\n\"COOK_CMD_FAILED:{x}\"";
    let stderr = "last:\nquoted \"value\"";
    let original = CommandFailure::new(
        9,
        -1,
        command,
        CapturedStream::from_bytes(stdout.as_bytes()),
        CapturedStream::from_bytes(stderr.as_bytes()),
    );

    let parsed = CommandFailure::from_wire(&original.to_wire()).expect("round trip");
    assert_eq!(parsed.line(), 9);
    assert_eq!(parsed.exit_code(), -1);
    assert_eq!(parsed.command(), command);
    assert_eq!(parsed.stdout().as_str(), stdout);
    assert_eq!(parsed.stderr().as_str(), stderr);
}

/// CS-0211: a producer that could not determine a line stores `0`, and
/// `located` is how every renderer learns there is no location — so that
/// "0" is never printed at a reader as though it were one.
#[test]
fn zero_is_the_absence_of_a_line_not_a_line() {
    let unlocated = CommandFailure::new(
        0,
        1,
        "false",
        CapturedStream::from_bytes(b""),
        CapturedStream::from_bytes(b""),
    );
    assert_eq!(unlocated.located(), None);

    let located = CommandFailure::new(
        7,
        1,
        "false",
        CapturedStream::from_bytes(b""),
        CapturedStream::from_bytes(b""),
    );
    assert_eq!(located.located(), Some(7));
}

fn failed(command: &str) -> CommandFailure {
    CommandFailure::new(
        0,
        1,
        command,
        CapturedStream::from_bytes(b""),
        CapturedStream::from_bytes(b""),
    )
}

/// CS-0215: what a reader is shown is the body they wrote, never the text
/// `shell_block::compose` made of it. The prelude is the implementation's,
/// so no renderer may print it and none may decide that for itself.
#[test]
fn displayed_command_is_the_body_the_author_wrote() {
    let composed = crate::shell_block::compose(&["echo a".to_string(), "false".to_string()]);
    assert_eq!(failed(&composed).displayed_command(), "echo a\nfalse");
}

/// The empty block is the edge the per-crate strippers kept getting wrong:
/// `compose(&[])` is the bare `set -e` with no trailing newline, and its
/// inverse is the empty body — not the prelude printed at the user.
#[test]
fn displayed_command_of_an_empty_block_is_empty() {
    let composed = crate::shell_block::compose(&[]);
    assert_eq!(failed(&composed).displayed_command(), "");
}

/// A command that was never composed — a `cook.sh` argument, say — is shown
/// exactly as it was given.
#[test]
fn displayed_command_passes_uncomposed_text_through() {
    assert_eq!(failed("false").displayed_command(), "false");
    assert_eq!(failed("echo set -e").displayed_command(), "echo set -e");
}
