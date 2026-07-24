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
//! at `<cache_dir>/<basename>.toml`, where `<basename>` is the recipe name
//! with the two path-hostile bytes percent-encoded (`%` → `%25`, `/` → `%2F`
//! — see [`cache_file_basename`]). Names without those bytes keep their
//! historical file names unchanged. The u64 hash fields (`command_hash`,
//! `env_contribution`, `FileRecord.hash`) are serialised as
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

/// Filesystem-safe basename for a recipe's cache file. Recipe names may
/// contain `/` — npm-scoped package names minted by modules produce recipes
/// like `@cap/env:build` — and a raw join would put the file under a
/// directory that never exists (the write failed with ENOENT and, until the
/// flush callsites started warning, was silently swallowed: the recipe
/// simply never cached). Only `%` (the escape itself) and `/` are encoded,
/// so every name without them keeps its historical file name.
fn cache_file_basename(recipe_name: &str) -> String {
    recipe_name.replace('%', "%25").replace('/', "%2F")
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
        let path = cache_dir.join(format!("{}.toml", cache_file_basename(recipe_name)));
        let text = std::fs::read_to_string(&path).ok()?;
        let cache: Self = toml::from_str(&text).map_err(|e| {
            tracing::debug!(
                path = %path.display(),
                error = %e,
                "recipe cache TOML parse failed — treating as cache miss"
            );
            e
        }).ok()?;
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
        let base = cache_file_basename(recipe_name);
        let target = cache_dir.join(format!("{}.toml", base));
        let tmp = cache_dir.join(format!("{}.toml.tmp", base));
        let text = toml::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&tmp, &text)?;
        std::fs::rename(&tmp, &target)?;
        Ok(())
    }
}

/// One-time hygiene sweep (COOK-92): delete orphaned pre-v4 bincode indexes
/// (`*.bin`) and torn temp files (`*.bin.tmp`) sitting directly inside
/// `cache_dir`. There is no migration — the loader only reads `.toml` — so
/// these files are dead weight left by older cook versions.
///
/// Non-recursive on purpose: `.cook/cache/tests/` (the JSON test cache) and
/// any other subdirectory are never touched. The artifact store lives under
/// a different root entirely (`~/.cache/cook/...`) and is out of scope.
/// Idempotent and infallible: a missing dir or a failed unlink is ignored
/// (the next construction retries).
pub fn sweep_orphaned_bin_indexes(cache_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".bin") || name.ends_with(".bin.tmp") {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
#[path = "tests/store_tests.rs"]
mod tests;
