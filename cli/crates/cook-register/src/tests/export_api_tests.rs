use super::*;

#[test]
fn test_export_and_import() {
    let mut store = BTreeMap::new();
    let value = serde_json::json!({
        "includes": ["/usr/include"],
        "lib_path": "/usr/lib"
    });
    store.insert("mylib".to_string(), value.clone());
    let imported = store.get("mylib").unwrap();
    assert_eq!(imported, &value);
    assert_eq!(imported["includes"][0], serde_json::json!("/usr/include"));
    assert_eq!(imported["lib_path"], serde_json::json!("/usr/lib"));
}

#[test]
fn test_import_missing_returns_none() {
    let store: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    assert!(store.get("nonexistent").is_none());
}

#[test]
fn test_export_overwrites() {
    let mut store = BTreeMap::new();
    store.insert("key".to_string(), serde_json::json!("first"));
    store.insert("key".to_string(), serde_json::json!("second"));
    let val = store.get("key").unwrap();
    assert_eq!(val, &serde_json::json!("second"));
}
