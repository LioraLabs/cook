use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;

use cook_contracts::CacheMeta;
use cook_fingerprint::{hash_file, stat_mtime, FileRecord, StepEntry};

use crate::store::RecipeCache;

/// Build FileRecord vec for a list of relative paths. Bails on the first
/// path whose mtime or content cannot be read. Returning Err from here
/// causes record_completion to skip the cache write entirely.
fn collect_records(paths: &[String], working_dir: &Path) -> Result<Vec<FileRecord>, String> {
    let mut out = Vec::with_capacity(paths.len());
    for rel in paths {
        let abs = working_dir.join(rel);
        let mtime = stat_mtime(&abs).ok_or_else(|| rel.clone())?;
        let hash = hash_file(&abs).ok_or_else(|| rel.clone())?;
        out.push(FileRecord { path: rel.clone(), mtime, hash });
    }
    Ok(out)
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("cache record skipped: input file missing or unreadable: {0}")]
    MissingFile(String),
    #[error("cache record skipped: output file missing or unreadable: {0}")]
    UnreadableFile(String),
}

pub struct CacheState {
    pub cache: RecipeCache,
    pub cache_dir: PathBuf,
    pub recipe_name: String,
    pub dirty: bool,
}

impl CacheState {
    pub fn new(cache: RecipeCache, cache_dir: PathBuf, recipe_name: String) -> Self {
        Self {
            cache,
            cache_dir,
            recipe_name,
            dirty: false,
        }
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        if self.dirty {
            self.cache.save(&self.cache_dir, &self.recipe_name)?;
            self.dirty = false;
        }
        Ok(())
    }

    // Returns the resolved files per glob pattern
    pub fn files_per_glob(&self) -> &BTreeMap<String, BTreeSet<String>> {
        &self.cache.globs
    }
}

pub type SharedCacheState = Rc<RefCell<CacheState>>;

pub struct ThreadSafeCacheManager {
    caches: Mutex<HashMap<String, RecipeCache>>,
    cache_dir: PathBuf,
    dirty: Mutex<HashSet<String>>,
}

impl ThreadSafeCacheManager {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            caches: Mutex::new(HashMap::new()),
            cache_dir,
            dirty: Mutex::new(HashSet::new()),
        }
    }

    pub fn load_recipe(&self, recipe_name: &str) {
        let cache = RecipeCache::load(&self.cache_dir, recipe_name).unwrap_or_default();
        let mut caches = self.caches.lock().unwrap();
        caches.insert(recipe_name.to_string(), cache);
    }

    pub fn update_step(&self, recipe_name: &str, cache_key: &str, entry: StepEntry) {
        let mut caches = self.caches.lock().unwrap();
        let recipe_cache = caches
            .entry(recipe_name.to_string())
            .or_default();
        recipe_cache.steps.insert(cache_key.to_string(), entry);
        drop(caches);
        let mut dirty = self.dirty.lock().unwrap();
        dirty.insert(recipe_name.to_string());
    }

    pub fn flush_all(&self) -> std::io::Result<()> {
        let dirty_names: Vec<String> = {
            let dirty = self.dirty.lock().unwrap();
            dirty.iter().cloned().collect()
        };
        let caches = self.caches.lock().unwrap();
        for name in &dirty_names {
            if let Some(cache) = caches.get(name) {
                cache.save(&self.cache_dir, name)?;
            }
        }
        drop(caches);
        let mut dirty = self.dirty.lock().unwrap();
        for name in &dirty_names {
            dirty.remove(name);
        }
        Ok(())
    }

    pub fn get_or_load(&self, recipe_name: &str) -> RecipeCache {
        let mut caches = self.caches.lock().unwrap();
        if let Some(cache) = caches.get(recipe_name) {
            return cache.clone();
        }
        let cache = RecipeCache::load(&self.cache_dir, recipe_name).unwrap_or_default();
        caches.insert(recipe_name.to_string(), cache.clone());
        cache
    }

    pub fn record_completion(
        &self,
        recipe_name: &str,
        cache_key: &str,
        meta: &CacheMeta,
        working_dir: &Path,
    ) -> Result<StepEntry, RecordError> {
        let new_inputs = collect_records(&meta.input_paths, working_dir)
            .map_err(|p| RecordError::MissingFile(p))?;
        let new_outputs = collect_records(&meta.output_paths, working_dir)
            .map_err(|p| RecordError::UnreadableFile(p))?;

        let entry = StepEntry {
            inputs: new_inputs,
            outputs: new_outputs,
            command_hash: meta.command_hash,
            context_hash: meta.context_hash,
            env_contribution: meta.env_contribution,
        };
        self.update_step(recipe_name, cache_key, entry.clone());
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{self, FileRecord, StepEntry};

    fn make_step_entry(command_hash: u64) -> StepEntry {
        StepEntry {
            inputs: vec![FileRecord {
                path: "src/main.c".to_string(),
                mtime: 1700000000,
                hash: 0xaabbccdd,
            }],
            outputs: vec![FileRecord {
                path: "build/main.o".to_string(),
                mtime: 1700000100,
                hash: 0x11223344,
            }],
            command_hash,
            context_hash: 0,
            env_contribution: 0,
        }
    }

    fn make_cache_meta(input_paths: Vec<String>, output_paths: Vec<String>) -> cook_contracts::CacheMeta {
        cook_contracts::CacheMeta {
            recipe_name: "test_recipe".into(),
            project_id: String::new(),
            cookfile_path: String::new(),
            cache_key: "step_one".into(),
            input_paths,
            output_paths,
            command_hash: 0xdeadbeef,
            context_hash: 0,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
            discovered_inputs: None,
        }
    }

    #[test]
    fn test_thread_safe_cache_write() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let manager = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        manager.update_step("my_recipe", "step_one", make_step_entry(0xdeadbeef));
        manager.flush_all().expect("flush_all failed");

        let loaded = store::RecipeCache::load(dir.path(), "my_recipe")
            .expect("cache not found on disk after flush");
        let step = loaded
            .steps
            .get("step_one")
            .expect("step_one not in loaded cache");
        assert_eq!(step.command_hash, 0xdeadbeef);
    }

    #[test]
    fn test_thread_safe_cache_multi_recipe() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let manager = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        manager.update_step("recipe_a", "step_a1", make_step_entry(0x1111));
        manager.update_step("recipe_b", "step_b1", make_step_entry(0x2222));
        manager.flush_all().expect("flush_all failed");

        let loaded_a =
            store::RecipeCache::load(dir.path(), "recipe_a").expect("recipe_a not found on disk");
        let loaded_b =
            store::RecipeCache::load(dir.path(), "recipe_b").expect("recipe_b not found on disk");

        assert_eq!(
            loaded_a
                .steps
                .get("step_a1")
                .expect("step_a1 missing")
                .command_hash,
            0x1111
        );
        assert_eq!(
            loaded_b
                .steps
                .get("step_b1")
                .expect("step_b1 missing")
                .command_hash,
            0x2222
        );
    }

    #[test]
    fn test_thread_safe_cache_idempotent_flush() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let manager = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        manager.update_step("recipe_x", "step_x1", make_step_entry(0xabcd));
        manager.flush_all().expect("first flush_all failed");
        manager.flush_all().expect("second flush_all failed");
    }

    #[test]
    fn test_get_or_load_missing() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let manager = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        let cache = manager.get_or_load("nonexistent_recipe");
        assert!(cache.steps.is_empty());
        assert_eq!(cache.schema_version, store::CACHE_VERSION);
    }

    #[test]
    fn record_completion_writes_full_step_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let cache_dir = dir.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("mkdir cache");
        let cm = ThreadSafeCacheManager::new(cache_dir.clone());

        let meta = make_cache_meta(vec!["in.c".into()], vec!["out.o".into()]);
        cm.record_completion("rec", "step_one", &meta, wd).expect("record ok");
        cm.flush_all().expect("flush");

        let loaded = store::RecipeCache::load(&cache_dir, "rec").expect("load");
        let entry = loaded.steps.get("step_one").expect("step");
        assert_eq!(entry.command_hash, 0xdeadbeef);
        assert_eq!(entry.inputs.len(), 1);
        assert_eq!(entry.outputs.len(), 1);
    }

    #[test]
    fn record_completion_skips_on_missing_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        // Do NOT create "in.c" — record_completion should skip.
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let cache_dir = dir.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("mkdir");
        let cm = ThreadSafeCacheManager::new(cache_dir.clone());

        let meta = make_cache_meta(vec!["in.c".into()], vec!["out.o".into()]);
        let err = cm.record_completion("rec", "step_one", &meta, wd).unwrap_err();
        assert!(matches!(err, RecordError::MissingFile(_)));

        // Verify nothing was written.
        cm.flush_all().expect("flush");
        let loaded = store::RecipeCache::load(&cache_dir, "rec");
        assert!(loaded.is_none() || loaded.unwrap().steps.is_empty());
    }

    #[test]
    fn record_completion_preserves_prior_entry_on_skip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let cache_dir = dir.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("mkdir");
        let cm = ThreadSafeCacheManager::new(cache_dir.clone());

        // First successful record.
        let meta = make_cache_meta(vec!["in.c".into()], vec!["out.o".into()]);
        cm.record_completion("rec", "step_one", &meta, wd).expect("record 1");
        cm.flush_all().expect("flush 1");

        // Now remove the input and try again — must err and leave prior entry intact.
        std::fs::remove_file(wd.join("in.c")).expect("rm");
        let err = cm.record_completion("rec", "step_one", &meta, wd).unwrap_err();
        assert!(matches!(err, RecordError::MissingFile(_)));
        cm.flush_all().expect("flush 2");

        let loaded = store::RecipeCache::load(&cache_dir, "rec").expect("load");
        let entry = loaded.steps.get("step_one").expect("prior entry survives");
        assert_eq!(entry.command_hash, 0xdeadbeef);
    }
}
