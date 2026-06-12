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
//! **Index format (v4+).** Each recipe is stored as a human-readable TOML file
//! at `<cache_dir>/<recipe_name>.toml`. The u64 hash fields (`command_hash`,
//! `context_hash`, `env_contribution`, `FileRecord.hash`) are serialised as
//! zero-padded 16-digit lowercase hex strings via `cook_fingerprint::record::hex_u64`.
//! The `schema_version` field is always the first key written by `toml::to_string`.
//! TOML is non-positional, so a file missing `schema_version` deserialises via
//! `default_cache_schema()` to 1 and is refused by the exact-match check.
//! Pre-v4 bincode `.bin` files are never opened by this loader.
//!
//! **Read policy (CS-0048).** A recipe cache whose `schema_version` exceeds
//! `CACHE_VERSION` is refused — the file was written by a future cook, and
//! the current binary cannot reason about its layout. A cache whose
//! `schema_version` is *less than* `CACHE_VERSION` is also refused today
//! because any schema mismatch is non-additive pre-v1.0. Both rejection
//! paths surface as a cache-miss (the file is regeneratable; no hard error
//! is needed).
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

/// Default value used by `serde` when `schema_version` is absent from the
/// TOML file. TOML is non-positional, so a missing key is plausible (e.g. a
/// hand-edited or pre-v4 file). Defaulting to 1 ensures the exact-match
/// version check refuses the file — 1 != CACHE_VERSION (currently 4).
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
        let path = cache_dir.join(format!("{}.toml", recipe_name));
        let text = std::fs::read_to_string(&path).ok()?;
        let cache: Self = toml::from_str(&text).ok()?;
        // CS-0048 read policy. See crate docs: today the check is exact
        // equality (pre-v1.0); the forward-compatible `<= CACHE_VERSION`
        // form takes effect once the additive-only contract starts at v1.0.
        if cache.schema_version != CACHE_VERSION {
            return None;
        }
        Some(cache)
    }

    pub fn save(&self, cache_dir: &Path, recipe_name: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(cache_dir)?;
        let target = cache_dir.join(format!("{}.toml", recipe_name));
        let tmp = cache_dir.join(format!("{}.toml.tmp", recipe_name));
        let text = toml::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&tmp, &text)?;
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
    fn version_is_four() {
        assert_eq!(CACHE_VERSION, 4);
    }

    #[test]
    fn round_trip_with_new_fields() {
        let original = make_populated_cache();
        let text = toml::to_string(&original).expect("serialize");
        let restored: RecipeCache = toml::from_str(&text).expect("deserialize");
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
        let text = toml::to_string(&original).expect("serialize");
        let restored: RecipeCache = toml::from_str(&text).expect("deserialize");
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
        let text = toml::to_string(&step).expect("serialize");
        let restored: StepEntry = toml::from_str(&text).expect("deserialize");
        assert_eq!(step, restored);
    }

    #[test]
    fn saved_index_is_human_readable_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        make_populated_cache().save(dir.path(), "my_recipe").expect("save");
        let path = dir.path().join("my_recipe.toml");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("schema_version = 4"), "got: {text}");
        assert!(text.contains(r#"command_hash = "0102030405060708""#), "got: {text}");
        assert!(text.contains(r#"hash = "1234567890abcdef""#), "got: {text}");
        assert!(!dir.path().join("my_recipe.toml.tmp").exists(), "tmp renamed away");
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
        std::fs::write(dir.path().join("bad.toml"), b"= not toml [").expect("write");
        assert!(RecipeCache::load(dir.path(), "bad").is_none());
    }

    #[test]
    fn load_old_schema_version_returns_none() {
        // CS-0048: a cache saved with schema_version=3 (bincode era) must be
        // refused — we save via current API then hand-patch the version field.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut old = RecipeCache::new();
        old.schema_version = 3;
        old.save(dir.path(), "old_v3").expect("save");
        assert!(RecipeCache::load(dir.path(), "old_v3").is_none());
    }

    #[test]
    fn load_future_schema_version_returns_none() {
        // CS-0048 read policy: a cache written by a future cook (schema_version
        // > CACHE_VERSION) is refused — the layout is unknown to this binary.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut future = RecipeCache::new();
        future.schema_version = CACHE_VERSION + 1;
        future.save(dir.path(), "future").expect("save");
        assert!(RecipeCache::load(dir.path(), "future").is_none());
    }

    #[test]
    fn schema_version_field_round_trips_at_cache_version() {
        // CS-0048: writers always emit `schema_version = CACHE_VERSION`.
        let cache = RecipeCache::new();
        assert_eq!(cache.schema_version, CACHE_VERSION);
        let text = toml::to_string(&cache).expect("serialize");
        let restored: RecipeCache = toml::from_str(&text).expect("deserialize");
        assert_eq!(restored.schema_version, CACHE_VERSION);
    }

    #[test]
    fn load_index_missing_schema_version_returns_none() {
        // TOML is non-positional: missing schema_version deserialises to
        // default_cache_schema() = 1, which != CACHE_VERSION, so load refuses.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("noversion.toml"), "[globs]\n\n[steps]\n")
            .expect("write");
        assert!(RecipeCache::load(dir.path(), "noversion").is_none());
    }

    #[test]
    fn load_ignores_legacy_bin_file() {
        // Pre-v4 .bin files must not be loaded — loader only opens .toml.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("legacy.bin"), b"\x00\x01junk bytes\xff")
            .expect("write");
        assert!(RecipeCache::load(dir.path(), "legacy").is_none());
    }
}
