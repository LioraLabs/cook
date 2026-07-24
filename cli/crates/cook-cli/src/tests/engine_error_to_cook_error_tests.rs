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
    let e = cook_engine::EngineError::TaskFailures {
        count: 1,
        failures: vec![(
            0,
            "build".to_string(),
            "COOK_CMD_FAILED:3:1:set -e\nfalse".to_string(),
        )],
        partial_test_results: vec![],
    };
    let err = engine_error_to_cook_error(e);
    let msg = err.to_string();
    assert!(!msg.contains("set -e"), "{msg}");
    assert!(msg.contains("false"), "{msg}");
}
