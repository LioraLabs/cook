use std::fs;
use std::io::{Read, Seek};

use cook_probe::store::materialize_value;

#[test]
fn creates_directory_and_writes_exact_bytes_to_canonical_name() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("nested/probes");

    let path = materialize_value(&dir, "cc/a_b", b"{\n  \"ok\": true\n}\n").unwrap();

    assert_eq!(path, dir.join("cc_2fa_5fb.json"));
    assert_eq!(fs::read(path).unwrap(), b"{\n  \"ok\": true\n}\n");
}

#[cfg(unix)]
#[test]
fn replaces_destination_by_rename_instead_of_truncating_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = materialize_value(temp.path(), "cc:zlib", b"old").unwrap();
    let mut old_handle = fs::File::open(&path).unwrap();

    materialize_value(temp.path(), "cc:zlib", b"new").unwrap();

    old_handle.rewind().unwrap();
    let mut old_bytes = Vec::new();
    old_handle.read_to_end(&mut old_bytes).unwrap();
    assert_eq!(old_bytes, b"old");
    assert_eq!(fs::read(path).unwrap(), b"new");
}

#[test]
fn removes_temporary_file_when_rename_fails() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = temp.path().join("cc:zlib.json");
    fs::create_dir(&canonical).unwrap();

    assert!(materialize_value(temp.path(), "cc:zlib", b"value").is_err());

    let siblings: Vec<_> = fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(siblings, vec![canonical.file_name().unwrap()]);
}
