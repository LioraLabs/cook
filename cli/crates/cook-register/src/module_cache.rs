use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Persistent JSON key-value cache scoped to a single module.
pub struct ModuleCache {
    module_name: String,
    cache_dir: PathBuf,
    data: BTreeMap<String, serde_json::Value>,
    dirty: bool,
}

impl ModuleCache {
    pub fn load(cache_dir: &Path, module_name: &str, source_hash: u64) -> Self {
        let path = cache_dir.join(format!("{}.json", module_name));
        let data = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    match serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&contents) {
                        Ok(mut map) => {
                            let stored_hash = map.get("_source_hash").and_then(|v| v.as_u64()).unwrap_or(0);
                            if stored_hash != source_hash {
                                map.clear();
                            }
                            map
                        }
                        Err(_) => BTreeMap::new(),
                    }
                }
                Err(_) => BTreeMap::new(),
            }
        } else {
            BTreeMap::new()
        };
        Self { module_name: module_name.to_string(), cache_dir: cache_dir.to_path_buf(), data, dirty: false }
    }

    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        if key == "_source_hash" { return None; }
        self.data.get(key)
    }

    pub fn set(&mut self, key: &str, value: serde_json::Value) {
        self.data.insert(key.to_string(), value);
        self.dirty = true;
    }

    pub fn set_source_hash(&mut self, hash: u64) {
        self.data.insert("_source_hash".to_string(), serde_json::Value::Number(serde_json::Number::from(hash)));
        self.dirty = true;
    }

    pub fn invalidate(&mut self, key: &str) {
        self.data.remove(key);
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.dirty = true;
    }

    pub fn flush(&self) -> std::io::Result<()> {
        if !self.dirty { return Ok(()); }
        std::fs::create_dir_all(&self.cache_dir)?;
        let path = self.cache_dir.join(format!("{}.json", self.module_name));
        let json = serde_json::to_string_pretty(&self.data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
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
    fn test_invalidate_key() {
        let dir = TempDir::new().unwrap();
        let mut cache = ModuleCache::load(dir.path(), "mymod", 0);
        cache.set("key", serde_json::json!(42));
        cache.invalidate("key");
        assert!(cache.get("key").is_none());
    }

    #[test]
    fn test_clear() {
        let dir = TempDir::new().unwrap();
        let mut cache = ModuleCache::load(dir.path(), "mymod", 0);
        cache.set("a", serde_json::json!(1));
        cache.set("b", serde_json::json!(2));
        cache.clear();
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_none());
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
}
