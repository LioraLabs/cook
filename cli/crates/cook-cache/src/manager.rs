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

/// Public wrapper for [`collect_records`] used by the engine's post-execution
/// augmentation path.
pub fn collect_records_public(
    paths: &[String],
    working_dir: &Path,
) -> Result<Vec<FileRecord>, String> {
    collect_records(paths, working_dir)
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
        // COOK-92 one-time sweep: drop orphaned pre-v4 `.bin` indexes on the
        // first touch of this cache dir. No-op once they are gone.
        crate::store::sweep_orphaned_bin_indexes(&cache_dir);
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

    /// Drop in-memory steps for which `keep(cache_key, step)` returns false,
    /// marking the recipe dirty if anything was removed so the next
    /// [`Self::flush_all`] persists the pruned set.
    ///
    /// Used by stale-output reconciliation (§17.7) to advance a recipe's
    /// recorded output set: steps whose every output is no longer declared
    /// are removed so the cache stops claiming swept artifacts.
    pub fn retain_steps<F>(&self, recipe_name: &str, keep: F)
    where
        F: Fn(&str, &StepEntry) -> bool,
    {
        let mut caches = self.caches.lock().unwrap();
        if let Some(cache) = caches.get_mut(recipe_name) {
            let before = cache.steps.len();
            cache.steps.retain(|k, v| keep(k, v));
            let changed = cache.steps.len() != before;
            drop(caches);
            if changed {
                self.dirty.lock().unwrap().insert(recipe_name.to_string());
            }
        }
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

        let mut new_outputs = new_outputs;
        if let Some(di) = &meta.discovered_inputs {
            // Append the depfile as an implicit output. If the file is
            // missing on disk post-execution, skip silently — the engine's
            // augmentation block (Task 10) handles the warning.
            if let Ok(records) = collect_records(
                &[di.from.clone()],
                working_dir,
            ) {
                if let Some(rec) = records.into_iter().next() {
                    new_outputs.push(rec);
                }
            }
        }

        let entry = StepEntry {
            inputs: new_inputs,
            outputs: new_outputs,
            command_hash: meta.command_hash,
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
    fn record_completion_appends_depfile_to_outputs() {
        use cook_contracts::DiscoveredInputs;

        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("a.c"), b"src").expect("a.c");
        std::fs::write(wd.join("a.o"), b"obj").expect("a.o");
        std::fs::create_dir_all(wd.join(".cook/deps")).expect("mkdir");
        std::fs::write(wd.join(".cook/deps/a.d"), b"a.o: a.c\n").expect("dep");

        let cache_dir = wd.join(".cook/cache");
        std::fs::create_dir_all(&cache_dir).expect("cachedir");
        let mgr = ThreadSafeCacheManager::new(cache_dir.clone());

        let mut meta = make_cache_meta(vec!["a.c".into()], vec!["a.o".into()]);
        meta.discovered_inputs = Some(DiscoveredInputs {
            from: ".cook/deps/a.d".into(),
            format: "make".into(),
        });

        let entry = mgr.record_completion("rec", "k", &meta, wd).expect("rec");

        let output_paths: Vec<&str> =
            entry.outputs.iter().map(|fr| fr.path.as_str()).collect();
        assert!(output_paths.contains(&"a.o"), "user output present");
        assert!(output_paths.contains(&".cook/deps/a.d"),
            "depfile appended to outputs when discovered_inputs is set");
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

    #[test]
    fn retain_steps_drops_and_persists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());
        cm.update_step("rec", "keep", make_step_entry(0x1));
        cm.update_step("rec", "drop", make_step_entry(0x2));
        cm.flush_all().expect("flush 1");

        // Reload into a fresh manager, then retain only "keep".
        let cm2 = ThreadSafeCacheManager::new(dir.path().to_path_buf());
        cm2.get_or_load("rec");
        cm2.retain_steps("rec", |k, _| k == "keep");
        cm2.flush_all().expect("flush 2");

        let loaded = store::RecipeCache::load(dir.path(), "rec").expect("load");
        assert!(loaded.steps.contains_key("keep"));
        assert!(!loaded.steps.contains_key("drop"), "stale step pruned");
    }

    #[test]
    fn manager_construction_sweeps_orphaned_bin_indexes() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Legacy bincode index + torn tmp from an interrupted pre-v4 write.
        std::fs::write(dir.path().join("old_recipe.bin"), b"\x03legacy").expect("bin");
        std::fs::write(dir.path().join("old_recipe.bin.tmp"), b"torn").expect("tmp");
        // Things the sweep must NOT touch: the TOML index, and subdirs
        // (the tests/ JSON cache lives under the same root).
        store::RecipeCache::new().save(dir.path(), "current").expect("save");
        std::fs::create_dir_all(dir.path().join("tests/ab")).expect("mkdir");
        std::fs::write(dir.path().join("tests/ab/abcd1234.json"), b"{}").expect("json");
        let _mgr = ThreadSafeCacheManager::new(dir.path().to_path_buf());
        assert!(!dir.path().join("old_recipe.bin").exists(), ".bin swept");
        assert!(!dir.path().join("old_recipe.bin.tmp").exists(), ".bin.tmp swept");
        assert!(dir.path().join("current.toml").exists(), "toml index untouched");
        assert!(dir.path().join("tests/ab/abcd1234.json").exists(), "test cache untouched");
    }

    #[test]
    fn manager_construction_tolerates_missing_cache_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");
        let _mgr = ThreadSafeCacheManager::new(missing); // must not panic or create the dir
        assert!(!dir.path().join("does-not-exist").exists());
    }

    #[test]
    fn retain_steps_keeps_all_is_not_dirty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());
        cm.update_step("rec", "a", make_step_entry(0x1));
        cm.retain_steps("rec", |_, _| true);
        // Nothing removed; flush still succeeds and the step survives.
        cm.flush_all().expect("flush");
        let loaded = store::RecipeCache::load(dir.path(), "rec").expect("load");
        assert!(loaded.steps.contains_key("a"));
    }
}
