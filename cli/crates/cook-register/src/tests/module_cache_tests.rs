use super::*;
use tempfile::TempDir;

#[test]
fn test_set_and_get() {
    let dir = TempDir::new().unwrap();
    let mut cache = ModuleCache::load(dir.path(), "mymod", 42);
    cache.set("key1", serde_json::Value::String("hello".to_string()));
    let val = cache.get("key1").unwrap();
    assert_eq!(val, &serde_json::Value::String("hello".to_string()));
}

#[test]
fn test_flush_and_reload() {
    let dir = TempDir::new().unwrap();
    let hash = 123u64;
    {
        let mut cache = ModuleCache::load(dir.path(), "mymod", hash);
        cache.set("greeting", serde_json::Value::String("world".to_string()));
        cache.set_source_hash(hash);
        cache.flush().unwrap();
    }
    let cache2 = ModuleCache::load(dir.path(), "mymod", hash);
    let val = cache2.get("greeting").unwrap();
    assert_eq!(val, &serde_json::Value::String("world".to_string()));
}

#[test]
fn test_source_hash_change_invalidates() {
    let dir = TempDir::new().unwrap();
    let hash_v1 = 1u64;
    let hash_v2 = 2u64;
    {
        let mut cache = ModuleCache::load(dir.path(), "mymod", hash_v1);
        cache.set("key", serde_json::Value::String("value".to_string()));
        cache.set_source_hash(hash_v1);
        cache.flush().unwrap();
    }
    // Reload with different hash — cache should be invalidated
    let cache2 = ModuleCache::load(dir.path(), "mymod", hash_v2);
    assert!(cache2.get("key").is_none());
}

#[test]
fn test_modules_have_separate_caches() {
    let dir = TempDir::new().unwrap();
    let hash = 99u64;
    {
        let mut cache_a = ModuleCache::load(dir.path(), "mod_a", hash);
        cache_a.set("key", serde_json::json!("from_a"));
        cache_a.set_source_hash(hash);
        cache_a.flush().unwrap();

        let mut cache_b = ModuleCache::load(dir.path(), "mod_b", hash);
        cache_b.set("key", serde_json::json!("from_b"));
        cache_b.set_source_hash(hash);
        cache_b.flush().unwrap();
    }
    let cache_a = ModuleCache::load(dir.path(), "mod_a", hash);
    let cache_b = ModuleCache::load(dir.path(), "mod_b", hash);
    assert_eq!(cache_a.get("key").unwrap(), &serde_json::json!("from_a"));
    assert_eq!(cache_b.get("key").unwrap(), &serde_json::json!("from_b"));
}
