use super::*;

#[test]
fn strip_set_e_removes_exact_prefix() {
    assert_eq!(strip_set_e("set -e\nmkdir -p build"), "mkdir -p build");
}

#[test]
fn strip_set_e_leaves_unprefixed_command_unchanged() {
    assert_eq!(strip_set_e("mkdir -p build"), "mkdir -p build");
}

#[test]
fn command_failed_render_strips_set_e_prelude() {
    let failure = cook_contracts::CommandFailure::new(
        3,
        1,
        "set -e\nfalse",
        cook_contracts::CapturedStream::from_bytes(b"before\n"),
        cook_contracts::CapturedStream::from_bytes(b"after\n"),
    );
    let e = cook_engine::EngineError::TaskFailures {
        count: 1,
        failures: vec![(
            0,
            "build".to_string(),
            format!("runtime error: {}", failure.to_wire()),
        )],
        partial_test_results: vec![],
    };
    let err = engine_error_to_cook_error(e);
    let msg = err.to_string();
    assert_eq!(
        msg,
        "Cookfile:3: command failed (exit 1): false\n--- stdout ---\nbefore\n--- stderr ---\nafter\n"
    );
}

#[test]
fn command_failed_render_omits_zero_location() {
    let failure = cook_contracts::CommandFailure::new(
        0,
        7,
        "exit 7",
        cook_contracts::CapturedStream::from_bytes(b""),
        cook_contracts::CapturedStream::from_bytes(b""),
    );
    let e = cook_engine::EngineError::TaskFailures {
        count: 1,
        failures: vec![(0, "build".to_string(), failure.to_wire())],
        partial_test_results: vec![],
    };
    assert_eq!(
        engine_error_to_cook_error(e).to_string(),
        "command failed (exit 7): exit 7"
    );
}

#[test]
fn command_failed_render_preserves_stderr_without_trailing_newline() {
    let failure = cook_contracts::CommandFailure::new(
        3,
        1,
        "false",
        cook_contracts::CapturedStream::from_bytes(b""),
        cook_contracts::CapturedStream::from_bytes(b"after"),
    );
    let e = cook_engine::EngineError::TaskFailures {
        count: 1,
        failures: vec![(0, "build".to_string(), failure.to_wire())],
        partial_test_results: vec![],
    };
    assert_eq!(
        engine_error_to_cook_error(e).to_string(),
        "Cookfile:3: command failed (exit 1): false\n--- stderr ---\nafter"
    );
}

#[test]
fn malformed_tagged_failure_uses_ordinary_error_path() {
    let message = "runtime error: COOK_CMD_FAILED:{not json}";
    let e = cook_engine::EngineError::TaskFailures {
        count: 1,
        failures: vec![(0, "build".to_string(), message.to_string())],
        partial_test_results: vec![],
    };
    assert_eq!(engine_error_to_cook_error(e).to_string(), message);
}
