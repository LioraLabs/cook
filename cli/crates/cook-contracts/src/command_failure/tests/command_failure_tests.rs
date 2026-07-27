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
