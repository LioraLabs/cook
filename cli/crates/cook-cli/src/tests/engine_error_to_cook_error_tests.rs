use super::*;

/// COOK-426 / CS-0215. This file used to re-assert `strip_set_e`'s own two
/// cases from here, which pinned a call this renderer no longer makes. The
/// target moves rather than disappearing: what belongs to this renderer is
/// that it shows a composed body through `displayed_command`, including the
/// empty-block edge it now inherits instead of re-deciding.
#[test]
fn command_failed_render_of_an_empty_block_quotes_nothing() {
    let failure = cook_contracts::CommandFailure::new(
        3,
        1,
        cook_contracts::shell_block::compose(&[]),
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
        "Cookfile:3: command failed (exit 1): "
    );
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

/// COOK-426 / CS-0215: the register phase has no `TaskFailures` to be decoded
/// out of. Its `cook.sh` failure arrives as a `PipelineError::Other` wrapping
/// the Lua host's message, and before this it was printed verbatim — sentinel,
/// JSON and all. Both catch-alls now share one consuming end.
#[test]
fn register_phase_wire_is_decoded_not_printed() {
    let failure = cook_contracts::CommandFailure::new(
        2,
        1,
        "false",
        cook_contracts::CapturedStream::from_bytes(b""),
        cook_contracts::CapturedStream::from_bytes(b""),
    );
    let e = cook_plan::PipelineError::Other(format!(
        "lua error: runtime error: {}",
        failure.to_wire()
    ));
    let err = pipeline_error_to_cook_error(e);
    assert_eq!(err.to_string(), "Cookfile:2: command failed (exit 1): false");
    assert_eq!(err.code(), "command-failed");
}

/// A pipeline error carrying no wire is untouched by the decoder.
#[test]
fn ordinary_pipeline_error_passes_through_the_decoder() {
    let e = cook_plan::PipelineError::Other("something else went wrong".to_string());
    let err = pipeline_error_to_cook_error(e);
    assert_eq!(err.to_string(), "something else went wrong");
    assert_eq!(err.code(), "error");
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
