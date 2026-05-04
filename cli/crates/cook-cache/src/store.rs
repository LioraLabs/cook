//! On-disk recipe-cache file format. The fingerprint state types
//! (`StepEntry`, `FileRecord`, `CACHE_VERSION`) live in `cook-fingerprint`
//! and are re-exported here for callers that already use `cook_cache::store::*`.
//!
//! ## Wire-format schema versioning (CS-0048)
//!
//! The on-disk `RecipeCache` carries a top-level `schema_version: u32` field.
//! Its value is sourced from [`CACHE_VERSION`] — the same constant used as a
//! fingerprint input by `cook-fingerprint`. The dual role is intentional:
//! a fingerprint-side bump (anything that changes how `StepEntry` /
//! `FileRecord` / per-step keys are computed) is by definition an incompatible
//! on-disk-format change, so the two move together.
//!
//! **Read policy (CS-0048).** A recipe cache whose `schema_version` exceeds
//! `CACHE_VERSION` is refused — the file was written by a future cook, and
//! the current binary cannot reason about its layout. A cache whose
//! `schema_version` is *less than* `CACHE_VERSION` is also refused today
//! because pre-v1.0 caches use a positional bincode layout where field
//! evolution was non-additive; older files cannot be safely deserialized
//! into the current struct shape. Both rejection paths surface as a
//! cache-miss (the file is regeneratable; no hard error is needed).
//!
//! **Evolution policy (v1.0+).** Future `RecipeCache` evolution is
//! additive-only: new fields are introduced with `#[serde(default)]` and the
//! `schema_version` constant stays at its current value. An incompatible
//! structural change bumps `CACHE_VERSION` (and therefore `schema_version`)
//! and is documented in App. D.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub use cook_fingerprint::record::{FileRecord, StepEntry, CACHE_VERSION};

/// Default value used by `serde` if a persisted cache predates the explicit
/// `schema_version` field name. Bincode is positional, so the field is always
/// present in practice; this default is the belt-and-braces guard for any
/// future JSON / non-positional encoding of the same struct.
fn default_cache_schema() -> u32 { 1 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecipeCache {
    /// Wire-format schema version. CS-0048: writers always emit
    /// `CACHE_VERSION`; readers refuse `schema_version > CACHE_VERSION`
    /// (and, today, any mismatch — see crate docs).
    #[serde(default = "default_cache_schema", alias = "version")]
    pub schema_version: u32,
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
            schema_version: CACHE_VERSION,
            globs: BTreeMap::new(),
            steps: BTreeMap::new(),
        }
    }

    pub fn load(cache_dir: &Path, recipe_name: &str) -> Option<Self> {
        let path = cache_dir.join(format!("{}.bin", recipe_name));
        let bytes = std::fs::read(&path).ok()?;
        let cache: Self = bincode::deserialize(&bytes).ok()?;
        // CS-0048 read policy. See crate docs: today the check is exact
        // equality because pre-v1.0 layout evolution was non-additive; the
        // forward-compatible `<= CACHE_VERSION` form takes effect once the
        // additive-only contract starts at v1.0.
        if cache.schema_version != CACHE_VERSION {
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
        assert_eq!(restored.schema_version, CACHE_VERSION);
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
        // Hand-craft a "v2" cache: just write a struct with schema_version=2.
        // We use a minimal serde value that bincode would accept as the v3 layout
        // but with schema_version=2 — the version check rejects it before any
        // field mismatch matters.
        let mut wrong_version = RecipeCache::new();
        wrong_version.schema_version = 2;
        let bytes = bincode::serialize(&wrong_version).expect("serialize");
        std::fs::write(dir.path().join("old.bin"), &bytes).expect("write");
        assert!(RecipeCache::load(dir.path(), "old").is_none());
    }

    #[test]
    fn load_future_schema_version_returns_none() {
        // CS-0048 read policy: a cache written by a future cook (schema_version
        // > CACHE_VERSION) is refused — the layout is unknown to this binary.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut future = RecipeCache::new();
        future.schema_version = CACHE_VERSION + 1;
        let bytes = bincode::serialize(&future).expect("serialize");
        std::fs::write(dir.path().join("future.bin"), &bytes).expect("write");
        assert!(RecipeCache::load(dir.path(), "future").is_none());
    }

    #[test]
    fn schema_version_field_round_trips_at_cache_version() {
        // CS-0048: writers always emit `schema_version = CACHE_VERSION`.
        let cache = RecipeCache::new();
        assert_eq!(cache.schema_version, CACHE_VERSION);
        let bytes = bincode::serialize(&cache).expect("serialize");
        let restored: RecipeCache = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(restored.schema_version, CACHE_VERSION);
    }
}
