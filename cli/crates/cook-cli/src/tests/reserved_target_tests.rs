use super::*;

#[test]
fn double_slash_target_is_rejected() {
    let err = reject_reserved_root_target("//check").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("reserved"), "msg: {msg}");
    assert!(msg.contains("not yet supported"), "msg: {msg}");
    assert!(msg.contains("check"), "msg: {msg}");
}

#[test]
fn normal_and_qualified_targets_pass() {
    assert!(reject_reserved_root_target("build").is_ok());
    assert!(reject_reserved_root_target("rust.build").is_ok());
    // single slash is not the reserved syntax
    assert!(reject_reserved_root_target("/x").is_ok());
}
