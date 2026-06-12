//! Per-run probe-value store (§22.5.8). The canonical value of a probe is
//! the file `.cook/probes/<key>.json` (CS-0102); this store is a
//! read-through byte cache of that file, shared by the engine scheduler
//! and every worker's `cook.cache.get`. It is NOT a cross-VM shared-memory
//! channel: writers (engine scheduler, register pre-pass) write the file
//! first and seed this cache with the same bytes.
//!
//! Locking: one mutex guards the whole map, held across the read-through
//! file load. Probe files are tiny; simplicity beats contention here, and
//! double-checked locking is how the historical pool nil-index race
//! (W13/pool.rs:1208) family of bugs happens — don't.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct ProbeValueStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    dir: Option<PathBuf>,
    map: BTreeMap<String, Vec<u8>>,
}

impl ProbeValueStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Point the read-through at a `.cook/probes` directory. Called once
    /// per execute_dag run by the engine.
    pub fn attach_dir(&self, dir: PathBuf) {
        self.inner.lock().unwrap().dir = Some(dir);
    }

    pub fn insert(&self, key: &str, bytes: Vec<u8>) {
        self.inner.lock().unwrap().map.insert(key.to_string(), bytes);
    }

    /// Map lookup, then `.cook/probes/<key>.json` fallback (caching the
    /// file bytes on success).
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(b) = inner.map.get(key) {
            return Some(b.clone());
        }
        let dir = inner.dir.clone()?;
        let bytes = std::fs::read(dir.join(cook_contracts::probe_value::probe_file_name(key))).ok()?;
        inner.map.insert(key.to_string(), bytes.clone());
        Some(bytes)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
        cook_contracts::probe_value::write_probe_file(tmp.path(), "cc:zlib", b"42\n").unwrap();
        let store = ProbeValueStore::new();
        store.attach_dir(tmp.path().to_path_buf());
        assert_eq!(store.get("cc:zlib"), Some(b"42\n".to_vec()));
        // Remove the file — should still be cached in memory.
        std::fs::remove_file(tmp.path().join("cc:zlib.json")).unwrap();
        assert_eq!(store.get("cc:zlib"), Some(b"42\n".to_vec())); // cached
    }
}
