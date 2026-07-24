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
        seal_contribution: u64,
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
            // COOK-161: the effective seal set's execute-phase value fold,
            // computed by the engine from the materialised probe values and
            // passed in (the CacheMeta carries only the seal *key set*).
            seal_contribution,
        };
        self.update_step(recipe_name, cache_key, entry.clone());
        Ok(entry)
    }
}

#[cfg(test)]
#[path = "tests/manager_tests.rs"]
mod tests;
