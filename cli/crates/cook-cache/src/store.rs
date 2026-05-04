//! On-disk recipe-cache file format. The fingerprint state types
//! (`StepEntry`, `FileRecord`, `CACHE_VERSION`) live in `cook-fingerprint`
//! and are re-exported here for callers that already use `cook_cache::store::*`.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub use cook_fingerprint::record::{FileRecord, StepEntry, CACHE_VERSION};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecipeCache {
    pub version: u32,
    pub globs: BTreeMap<String, BTreeSet<String>>,
    pub steps: BTreeMap<String, StepEntry>,
    // REMOVED: secondary_inputs_hash (SHI-145) — dead code path.
    // REMOVED: env_hash (SHI-142) — folded into per-step env_contribution.
}

impl Default for RecipeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RecipeCache {
    pub fn new() -> Self {
        Self {
            version: CACHE_VERSION,
            globs: BTreeMap::new(),
            steps: BTreeMap::new(),
        }
    }

    pub fn load(cache_dir: &Path, recipe_name: &str) -> Option<Self> {
        let path = cache_dir.join(format!("{}.bin", recipe_name));
        let bytes = std::fs::read(&path).ok()?;
        let cache: Self = bincode::deserialize(&bytes).ok()?;
        if cache.version != CACHE_VERSION {
            return None;
        }
        Some(cache)
    }

    pub fn save(&self, cache_dir: &Path, recipe_name: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(cache_dir)?;
        let target = cache_dir.join(format!("{}.bin", recipe_name));
        let tmp = cache_dir.join(format!("{}.bin.tmp", recipe_name));
        let bytes = bincode::serialize(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &target)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_populated_cache() -> RecipeCache {
        let mut cache = RecipeCache::new();

        let mut globs = BTreeMap::new();
        globs.insert(
            "src/*.c".to_string(),
            BTreeSet::from(["src/main.c".to_string(), "src/util.c".to_string()]),
        );
        cache.globs = globs;

        let step = StepEntry {
            inputs: vec![
                FileRecord {
                    path: "src/main.c".to_string(),
                    mtime: 1700000000,
                    hash: 0x1234567890abcdef,
                },
                FileRecord {
                    path: "src/util.c".to_string(),
                    mtime: 1700000001,
                    hash: 0xfedcba9876543210,
                },
            ],
            outputs: vec![FileRecord {
                path: "build/main.o".to_string(),
                mtime: 1700000100,
                hash: 0xabcdef1234567890,
            }],
            command_hash: 0x0102030405060708,
            context_hash: 0x1111111111111111,
            env_contribution: 0x2222222222222222,
        };
        cache.steps.insert("compile_main".to_string(), step);

        cache
    }

    #[test]
    fn version_is_three() {
        assert_eq!(CACHE_VERSION, 3);
    }

    #[test]
    fn round_trip_with_new_fields() {
        let original = make_populated_cache();
        let bytes = bincode::serialize(&original).expect("serialize");
        let restored: RecipeCache = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(original, restored);
        assert_eq!(restored.version, CACHE_VERSION);
        let step = restored.steps.get("compile_main").unwrap();
        assert_eq!(step.command_hash, 0x0102030405060708);
        assert_eq!(step.context_hash, 0x1111111111111111);
        assert_eq!(step.env_contribution, 0x2222222222222222);
    }

    #[test]
    fn empty_cache_round_trip() {
        let original = RecipeCache::new();
        let bytes = bincode::serialize(&original).expect("serialize");
        let restored: RecipeCache = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn plate_step_no_output() {
        let step = StepEntry {
            inputs: vec![FileRecord {
                path: "src/main.c".to_string(),
                mtime: 1700000000,
                hash: 0x1234567890abcdef,
            }],
            outputs: vec![],
            command_hash: 0xdeadbeefcafe,
            context_hash: 0xc0c0c0c0,
            env_contribution: 0xe0e0e0e0,
        };
        let bytes = bincode::serialize(&step).expect("serialize");
        let restored: StepEntry = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(step, restored);
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let original = make_populated_cache();
        original.save(dir.path(), "my_recipe").expect("save");
        let loaded = RecipeCache::load(dir.path(), "my_recipe").expect("load");
        assert_eq!(original, loaded);
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(RecipeCache::load(dir.path(), "nonexistent").is_none());
    }

    #[test]
    fn load_corrupted_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("bad.bin"), b"not bincode").expect("write");
        assert!(RecipeCache::load(dir.path(), "bad").is_none());
    }

    #[test]
    fn load_v2_returns_none_via_version_check() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Hand-craft a "v2" cache: just write a struct with version=2.
        // We use a minimal serde value that bincode would accept as the v3 layout
        // but with version=2 — the version check rejects it before any field
        // mismatch matters.
        let mut wrong_version = RecipeCache::new();
        wrong_version.version = 2;
        let bytes = bincode::serialize(&wrong_version).expect("serialize");
        std::fs::write(dir.path().join("old.bin"), &bytes).expect("write");
        assert!(RecipeCache::load(dir.path(), "old").is_none());
    }
}
