use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecipeCache {
    pub version: u32,
    pub globs: BTreeMap<String, BTreeSet<String>>,
    pub secondary_inputs_hash: u64,
    pub env_hash: u64,
    pub steps: BTreeMap<String, StepEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepEntry {
    pub inputs: Vec<FileRecord>,
    pub output: Option<FileRecord>,
    pub command_hash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileRecord {
    pub path: String,
    pub mtime: u64,
    pub hash: u64,
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
            secondary_inputs_hash: 0,
            env_hash: 0,
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
        cache.secondary_inputs_hash = 0xdeadbeef;
        cache.env_hash = 0xcafebabe;

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
            output: Some(FileRecord {
                path: "build/main.o".to_string(),
                mtime: 1700000100,
                hash: 0xabcdef1234567890,
            }),
            command_hash: 0x0102030405060708,
        };
        cache.steps.insert("compile_main".to_string(), step);

        cache
    }

    #[test]
    fn test_recipe_cache_round_trip() {
        let original = make_populated_cache();
        let bytes = bincode::serialize(&original).expect("serialization failed");
        let restored: RecipeCache = bincode::deserialize(&bytes).expect("deserialization failed");
        assert_eq!(original, restored);
        assert_eq!(restored.version, CACHE_VERSION);
        assert_eq!(restored.secondary_inputs_hash, 0xdeadbeef);
        assert_eq!(restored.env_hash, 0xcafebabe);
        assert_eq!(restored.globs.len(), 1);
        assert_eq!(restored.steps.len(), 1);
        let step = restored.steps.get("compile_main").unwrap();
        assert_eq!(step.inputs.len(), 2);
        assert!(step.output.is_some());
        assert_eq!(step.command_hash, 0x0102030405060708);
    }

    #[test]
    fn test_empty_cache_round_trip() {
        let original = RecipeCache::new();
        let bytes = bincode::serialize(&original).expect("serialization failed");
        let restored: RecipeCache = bincode::deserialize(&bytes).expect("deserialization failed");
        assert_eq!(original, restored);
        assert_eq!(restored.version, CACHE_VERSION);
        assert_eq!(restored.secondary_inputs_hash, 0);
        assert_eq!(restored.env_hash, 0);
        assert!(restored.globs.is_empty());
        assert!(restored.steps.is_empty());
    }

    #[test]
    fn test_plate_step_no_output() {
        let step = StepEntry {
            inputs: vec![FileRecord {
                path: "src/main.c".to_string(),
                mtime: 1700000000,
                hash: 0x1234567890abcdef,
            }],
            output: None,
            command_hash: 0xdeadbeefcafe,
        };
        let bytes = bincode::serialize(&step).expect("serialization failed");
        let restored: StepEntry = bincode::deserialize(&bytes).expect("deserialization failed");
        assert_eq!(step, restored);
        assert!(restored.output.is_none());
        assert_eq!(restored.inputs.len(), 1);
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let cache_dir = dir.path();
        let original = make_populated_cache();
        original
            .save(cache_dir, "my_recipe")
            .expect("save failed");
        let loaded = RecipeCache::load(cache_dir, "my_recipe").expect("load returned None");
        assert_eq!(original, loaded);
    }

    #[test]
    fn test_load_missing_returns_none() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let result = RecipeCache::load(dir.path(), "nonexistent_recipe");
        assert!(result.is_none());
    }

    #[test]
    fn test_load_corrupted_returns_none() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let cache_dir = dir.path();
        let path = cache_dir.join("bad_recipe.bin");
        std::fs::write(&path, b"this is not valid bincode data at all!!!")
            .expect("write failed");
        let result = RecipeCache::load(cache_dir, "bad_recipe");
        assert!(result.is_none());
    }

    #[test]
    fn test_load_wrong_version_returns_none() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let cache_dir = dir.path();
        // Construct a cache with a different version number
        let mut cache = make_populated_cache();
        cache.version = CACHE_VERSION + 1;
        let bytes = bincode::serialize(&cache).expect("serialization failed");
        let path = cache_dir.join("versioned_recipe.bin");
        std::fs::write(&path, &bytes).expect("write failed");
        let result = RecipeCache::load(cache_dir, "versioned_recipe");
        assert!(result.is_none());
    }
}
