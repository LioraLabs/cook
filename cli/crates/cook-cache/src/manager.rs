use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cook_contracts::CacheMeta;
use cook_fingerprint::{hash_file, stat_mtime, FileRecord, StepEntry};

use crate::store::RecipeCache;

/// Build FileRecord vec for a list of relative paths. Bails on the first
/// path whose mtime or content cannot be read. Returning Err from here
/// causes record_completion to skip the cache write entirely.
pub fn collect_records(paths: &[String], working_dir: &Path) -> Result<Vec<FileRecord>, String> {
    let mut out = Vec::with_capacity(paths.len());
    for rel in paths {
        let abs = working_dir.join(rel);
        let mtime = stat_mtime(&abs).ok_or_else(|| rel.clone())?;
        let hash = hash_file(&abs).ok_or_else(|| rel.clone())?;
        out.push(FileRecord { path: rel.as_str().into(), mtime, hash });
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


/// Outcome of a single keyed step lookup against a recipe index.
///
/// See [`ThreadSafeCacheManager::lookup_step`]: this is the whole of what the
/// per-node hot path needs from the index, so it is what the manager hands
/// out there instead of the index itself.
pub struct StepLookup {
    /// The entry stored under the requested cache key, if any.
    pub entry: Option<StepEntry>,
    /// COOK-276: no entry under the requested key, but a sibling entry exists
    /// under the same output identity (`<first-output>` bare, or
    /// `<first-output>@<env:...>` under a different env suffix). Proof of
    /// history: the miss is attributable to a changed env value rather than
    /// being a cold build.
    pub env_moved_key: bool,
}

pub struct ThreadSafeCacheManager {
    /// Per-recipe indexes behind `Arc` so a reader can take a snapshot by
    /// copying a pointer. See [`Self::get_or_load`] for why by-value is not
    /// an option (COOK-306).
    caches: Mutex<HashMap<String, Arc<RecipeCache>>>,
    cache_dir: PathBuf,
    dirty: Mutex<HashSet<String>>,
}

impl ThreadSafeCacheManager {
    pub fn new(cache_dir: PathBuf) -> Self {
        // COOK-92 / CS-0166 sweep: drop index files in a format this cook no
        // longer reads (pre-v4 `.bin`, v4..v6 `.toml`, torn temps) on the
        // first touch of this cache dir. No-op once they are gone.
        crate::store::sweep_superseded_indexes(&cache_dir);
        Self {
            caches: Mutex::new(HashMap::new()),
            cache_dir,
            dirty: Mutex::new(HashSet::new()),
        }
    }

    pub fn load_recipe(&self, recipe_name: &str) {
        let cache = RecipeCache::load(&self.cache_dir, recipe_name).unwrap_or_default();
        let mut caches = self.caches.lock().unwrap();
        caches.insert(recipe_name.to_string(), Arc::new(cache));
    }

    /// Resolve `recipe_name` to its in-memory index, loading it from disk on
    /// first touch. Caller must already hold the `caches` lock.
    fn resolve<'a>(
        caches: &'a mut HashMap<String, Arc<RecipeCache>>,
        cache_dir: &Path,
        recipe_name: &str,
    ) -> &'a Arc<RecipeCache> {
        caches.entry(recipe_name.to_string()).or_insert_with(|| {
            Arc::new(RecipeCache::load(cache_dir, recipe_name).unwrap_or_default())
        })
    }

    pub fn update_step(&self, recipe_name: &str, cache_key: &str, entry: StepEntry) {
        let mut caches = self.caches.lock().unwrap();
        let recipe_cache = caches
            .entry(recipe_name.to_string())
            .or_default();
        // COOK-306: a settled run re-validates every step and writes back an
        // entry identical to the one it read. Storing it is a no-op; marking
        // the recipe dirty is not — it costs a full re-serialisation of the
        // index at flush time, which for DuckDB's 128 MB index is seconds of
        // TOML writing on a run that changed nothing. Comparing first is
        // O(records) with no allocation, far cheaper than the write it avoids.
        if recipe_cache.steps.get(cache_key) == Some(&entry) {
            return;
        }
        // `make_mut` copies the index only when a snapshot handed out by
        // `get_or_load` is still alive. Nothing holds one across execution
        // (every caller drops it within its own statement or block), so this
        // is an in-place insert on the hot path. Retaining a snapshot across
        // a build would silently restore the COOK-306 quadratic clone.
        Arc::make_mut(recipe_cache).steps.insert(cache_key.to_string(), entry);
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
            Arc::make_mut(cache).steps.retain(|k, v| keep(k, v));
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

    /// Snapshot a whole recipe index, loading it from disk on first touch.
    ///
    /// Returns an `Arc`, not a copy: a large real-world index is large enough
    /// that copying it per caller is a build-dominating cost (COOK-306 — a
    /// 128 MB / 648k-record DuckDB index copied once per work node was 95% of
    /// all allocation traffic and ~102s of a 107s settled no-op). The `Arc`
    /// also keeps the lock hold time to a pointer copy, so this stays cheap
    /// if cache validation is ever parallelised.
    ///
    /// The per-node cache check wants one keyed entry rather than the whole
    /// index — use [`Self::lookup_step`] there. Callers of this method should
    /// stay once-per-run: hold a snapshot across execution and
    /// [`Self::update_step`] must copy the index to mutate it.
    pub fn get_or_load(&self, recipe_name: &str) -> Arc<RecipeCache> {
        let mut caches = self.caches.lock().unwrap();
        Arc::clone(Self::resolve(&mut caches, &self.cache_dir, recipe_name))
    }

    /// Look up a single step by cache key, copying just that entry.
    ///
    /// This is the per-work-node hot path (COOK-306). `output_base` is the
    /// unit's first declared output path, used only to attribute a miss to a
    /// changed env value — see [`StepLookup::env_moved_key`].
    pub fn lookup_step(
        &self,
        recipe_name: &str,
        cache_key: &str,
        output_base: &str,
    ) -> StepLookup {
        let mut caches = self.caches.lock().unwrap();
        let cache = Self::resolve(&mut caches, &self.cache_dir, recipe_name);
        if let Some(entry) = cache.steps.get(cache_key) {
            return StepLookup { entry: Some(entry.clone()), env_moved_key: false };
        }
        let prefix = format!("{output_base}@");
        let env_moved_key = cache.steps.contains_key(output_base)
            || cache
                .steps
                .range(prefix.clone()..)
                .take_while(|(k, _)| k.starts_with(&prefix))
                .next()
                .is_some();
        StepLookup { entry: None, env_moved_key }
    }

    pub fn record_completion(
        &self,
        recipe_name: &str,
        cache_key: &str,
        meta: &CacheMeta,
        working_dir: &Path,
        seal_contribution: u64,
    ) -> Result<StepEntry, RecordError> {
        // The caller hands us a `CacheMeta` whose inputs are the RESOLVED set
        // the unit was judged against (§17.1.1.2), so every entry here names a
        // file to hash. Recording a pattern would record a path that does not
        // exist and fail the whole record on it.
        let judged_paths: Vec<String> = meta.inputs.iter().map(|i| i.path.clone()).collect();
        let new_inputs =
            collect_records(&judged_paths, working_dir).map_err(|p| RecordError::MissingFile(p))?;
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
            observed: None,
        };
        self.update_step(recipe_name, cache_key, entry.clone());
        Ok(entry)
    }
}

#[cfg(test)]
#[path = "tests/manager_tests.rs"]
mod tests;
