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
fn registration_command_failed_render_strips_set_e_prelude() {
    let e: EngineError = cook_register::RegisterError::CommandFailed {
        command: "set -e\nfalse".to_string(),
        line: 3,
        code: 1,
    }
    .into();
    match e {
        EngineError::RegistrationFailed { message, .. } => {
            assert!(!message.contains("set -e"), "{message}");
            assert!(message.contains("false"), "{message}");
        }
        other => panic!("expected RegistrationFailed, got {other:?}"),
    }
}
