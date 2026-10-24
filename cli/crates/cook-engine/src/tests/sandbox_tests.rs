use super::*;

#[test]
fn mount_paths_with_option_separators_are_refused() {
    for bad in ["/a,b", "/a:b", "/a\\b"] {
        let e = mount_safe(Path::new(bad)).unwrap_err();
        assert!(e.contains(bad), "{e}");
    }
    assert!(mount_safe(Path::new("/a b/c")).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn make_removable_unlocks_read_only_dirs() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let locked = d.path().join("a/b");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("f"), "x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    std::fs::set_permissions(d.path().join("a"), std::fs::Permissions::from_mode(0o000)).unwrap();
    make_removable(d.path());
    d.close().unwrap();
}

#[test]
fn a_frame_round_trips_and_anything_else_is_refused() {
    assert_eq!(unframe(&frame(b"{}")), Some(b"{}".to_vec()));
    assert_eq!(unframe(&frame(b"")), Some(vec![]));
    assert_eq!(unframe(b""), None, "empty");
    assert_eq!(unframe(&[2, 0, 0, 0]), None, "short header");
    let whole = frame(b"abcd");
    assert_eq!(unframe(&whole[..whole.len() - 1]), None, "short body");
    let mut extra = whole.clone();
    extra.push(b'x');
    assert_eq!(unframe(&extra), None, "trailing bytes");
    assert_eq!(unframe(&[0xff; 12]), None, "huge length");
}
