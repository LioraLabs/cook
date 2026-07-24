use super::*;

#[test]
fn anchor_globs_joins_relative_and_keeps_absolute() {
    let dir = std::path::Path::new("/ws/apps/rust");
    let got = anchor_globs(
        vec!["src/*.c".to_string(), "/abs/x/*.h".to_string()],
        dir,
    );
    assert_eq!(got, vec!["/ws/apps/rust/src/*.c".to_string(), "/abs/x/*.h".to_string()]);
}
