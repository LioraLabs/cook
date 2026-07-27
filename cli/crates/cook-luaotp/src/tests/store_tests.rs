use super::*;

#[test]
fn insert_then_get_returns_bytes() {
    let store = ProbeValueStore::new();
    store.insert("cc:zlib", b"42\n".to_vec());
    assert_eq!(store.get("cc:zlib"), Some(b"42\n".to_vec()));
}

#[test]
fn get_miss_without_dir_is_none() {
    assert_eq!(ProbeValueStore::new().get("cc:zlib"), None);
}

#[test]
fn get_reads_through_to_probe_file() {
    let tmp = tempfile::tempdir().unwrap();
    cook_probe::store::materialize_value(tmp.path(), "cc:zlib", b"42\n").unwrap();
    let store = ProbeValueStore::new();
    store.attach_dir(tmp.path().to_path_buf());
    assert_eq!(store.get("cc:zlib"), Some(b"42\n".to_vec()));
    // Remove the file — should still be cached in memory.
    std::fs::remove_file(tmp.path().join("cc:zlib.json")).unwrap();
    assert_eq!(store.get("cc:zlib"), Some(b"42\n".to_vec())); // cached
}
