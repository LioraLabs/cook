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
#[path = "tests/module_cache_tests.rs"]
mod tests;
