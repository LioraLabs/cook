use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;

use crate::check::{hash_file, stat_mtime};
use crate::resolve_glob;
use crate::store::{FileRecord, RecipeCache, StepEntry};

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
    pub fn invalidate_recipe(
        &mut self,
        env_hash: u64,
        secondary_inputs_hash: u64,
        ingredient_patterns: &[String],
        working_dir: &Path,
    ) {
        // 1. Update the env and secondary inputs hashes
        // set the dirty flag if the hashes have changed
        self.dirty = self.cache.env_hash != env_hash
            || self.cache.secondary_inputs_hash != secondary_inputs_hash;
        self.cache.env_hash = env_hash;
        self.cache.secondary_inputs_hash = secondary_inputs_hash;

        // 2. Process each glob pattern
        for pattern in ingredient_patterns {
            let current_files = resolve_glob(working_dir, pattern);

            // Only proceed if the files for this pattern have actually changed
            if self.cache.globs.get(pattern) != Some(&current_files) {
                // If we have old data, find what was removed and invalidate those steps
                if let Some(old_files) = self.cache.globs.get(pattern) {
                    let removed: BTreeSet<_> = old_files.difference(&current_files).collect();

                    if !removed.is_empty() {
                        self.cache.steps.retain(|_, entry| {
                            !entry.inputs.iter().any(|f| removed.contains(&f.path))
                        });
                    }
                }

                // Update the cache with the new file list
                self.cache.globs.insert(pattern.clone(), current_files);
                self.dirty = true;
            }
        }
    }

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

    /// Check if the environment has changed since the last build. If so,
    /// clear all cached steps for this recipe (forcing a full rebuild).
    pub fn invalidate_if_env_changed(&self, recipe_name: &str, env_hash: u64) {
        let mut caches = self.caches.lock().unwrap();
        let cache = caches
            .entry(recipe_name.to_string())
            .or_insert_with(|| RecipeCache::load(&self.cache_dir, recipe_name).unwrap_or_default());

        if cache.env_hash != env_hash {
            cache.steps.clear();
            cache.env_hash = env_hash;
            drop(caches);
            let mut dirty = self.dirty.lock().unwrap();
            dirty.insert(recipe_name.to_string());
        }
    }

    pub fn record_completion(
        &self,
        recipe_name: &str,
        cache_key: &str,
        command_hash: u64,
        input_paths: &[String],
        output_path: Option<&String>,
        working_dir: &Path,
    ) {
        let new_inputs: Vec<FileRecord> = input_paths
            .iter()
            .map(|rel| {
                let abs = working_dir.join(rel);
                FileRecord {
                    path: rel.clone(),
                    mtime: stat_mtime(&abs).unwrap_or(0),
                    hash: hash_file(&abs).unwrap_or(0),
                }
            })
            .collect();

        let new_outputs: Vec<FileRecord> = output_path
            .map(|rel| {
                let abs = working_dir.join(rel);
                FileRecord {
                    path: rel.clone(),
                    mtime: stat_mtime(&abs).unwrap_or(0),
                    hash: hash_file(&abs).unwrap_or(0),
                }
            })
            .into_iter()
            .collect();

        self.update_step(
            recipe_name,
            cache_key,
            StepEntry {
                inputs: new_inputs,
                outputs: new_outputs,
                command_hash,
            },
        );
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
        assert_eq!(cache.version, store::CACHE_VERSION);
    }

    #[test]
    fn test_invalidate_if_env_changed_clears_steps() {
        let dir = tempfile::tempdir().unwrap();
        let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        // Establish env_hash before populating steps
        cm.invalidate_if_env_changed("build", 100);

        // Populate cache with a step
        cm.update_step("build", "main.o", make_step_entry(0x1234));

        // Same env hash — steps should survive
        cm.invalidate_if_env_changed("build", 100);
        let cache = cm.get_or_load("build");
        assert!(cache.steps.contains_key("main.o"), "step should survive same env hash");

        // Different env hash — steps should be cleared
        cm.invalidate_if_env_changed("build", 999);
        let cache = cm.get_or_load("build");
        assert!(cache.steps.is_empty(), "steps should be cleared on env hash change");
        assert_eq!(cache.env_hash, 999);
    }

    #[test]
    fn test_invalidate_if_env_changed_no_op_on_match() {
        let dir = tempfile::tempdir().unwrap();
        let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        // Establish env_hash before populating steps
        cm.invalidate_if_env_changed("build", 42);

        cm.update_step("build", "main.o", make_step_entry(0x1234));

        // Same env hash — steps should survive
        cm.invalidate_if_env_changed("build", 42);
        let cache = cm.get_or_load("build");
        assert_eq!(cache.steps.len(), 1, "steps should survive when env hash matches");

        // Same hash again — still a no-op
        cm.invalidate_if_env_changed("build", 42);
        let cache = cm.get_or_load("build");
        assert_eq!(cache.steps.len(), 1);
    }
}
