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
//! **Index format (v7+, CS-0166).** Each recipe is stored as a binary file at
//! `<cache_dir>/<basename>.idx`, where `<basename>` is the recipe name with
//! the two path-hostile bytes percent-encoded (`%` → `%25`, `/` → `%2F` —
//! see [`cache_file_basename`]). The layout, and why it is hand-rolled rather
//! than a serde codec, are documented on [`crate::index_bin`]. Readability
//! moved from the file to `cook why` and `cook cache dump`.
//!
//! v4..v6 stored a human-readable TOML file at `<basename>.toml`. That format
//! spent 48.8% of its bytes restating each step key once per input record (a
//! `toml::to_string` array-of-tables artifact, not a decision anyone made) and
//! stored every path in full despite a ~49x redundancy across translation
//! units. On a 1,711-node DuckDB build it reached 69 MB and cost 0.75s to
//! parse on a run that executed nothing. Neither `.toml` nor the pre-v4
//! bincode `.bin` is ever opened by this loader.
//!
//! **Read policy (CS-0048).** A recipe cache whose `schema_version` exceeds
//! `CACHE_VERSION` is refused — the file was written by a future cook, and
//! the current binary cannot reason about its layout. A cache whose
//! `schema_version` is *less than* `CACHE_VERSION` is also refused today
//! because any schema mismatch is non-additive pre-v1.0. Both rejection
//! paths surface as a cache-miss (the file is regeneratable; no hard error
//! is needed), as does a failed checksum or any structural corruption.
//!
//! **No migration (CS-0166).** The index is regeneratable by definition, so
//! nothing is migrated across a format change: superseded index files are
//! deleted by [`sweep_superseded_indexes`] and the graph rebuilds once.
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
        let path = cache_dir.join(format!("{}{INDEX_EXT}", cache_file_basename(recipe_name)));
        let bytes = std::fs::read(&path).ok()?;
        // Every decode failure is a cache-miss: the index is regeneratable, so
        // there is nothing to gain by failing the build over one. The version
        // check lives inside `decode` (CS-0048 exact-equality read policy).
        crate::index_bin::decode(&bytes)
            .map_err(|e| {
                tracing::debug!(
                    path = %path.display(),
                    error = %e,
                    "recipe index decode failed — treating as cache miss"
                );
                e
            })
            .ok()
    }

    pub fn save(&self, cache_dir: &Path, recipe_name: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(cache_dir)?;
        let base = cache_file_basename(recipe_name);
        let target = cache_dir.join(format!("{base}{INDEX_EXT}"));
        let tmp = cache_dir.join(format!("{base}{INDEX_EXT}.tmp"));
        std::fs::write(&tmp, crate::index_bin::encode(self))?;
        std::fs::rename(&tmp, &target)?;
        Ok(())
    }
}

impl RecipeCache {
    /// Render the index as TOML for `cook cache dump` (CS-0166).
    ///
    /// Hand-rendered rather than `toml::to_string` on purpose. Serialising
    /// `Vec<FileRecord>` through the `toml` crate emits an array-of-tables,
    /// which restates the full step key as a `[[steps."<key>".inputs]]` header
    /// before every single record — on DuckDB that shape was 48.8% of a 69 MB
    /// file, and it is exactly what nobody wanted to read. Inlining each
    /// record onto one line is both smaller and what a reader actually wants.
    ///
    /// Output is for human eyes; there is no reader and no round-trip
    /// requirement. Values are escaped through `toml::Value` so a path with a
    /// quote or a backslash still renders as valid TOML.
    pub fn to_readable_toml(&self) -> String {
        fn quoted(s: &str) -> String {
            toml::Value::String(s.to_string()).to_string()
        }
        fn record_line(r: &FileRecord) -> String {
            format!(
                "  {{ path = {}, mtime = {}, hash = \"{:016x}\" }},",
                quoted(&r.path),
                r.mtime,
                r.hash
            )
        }

        let distinct: BTreeSet<&str> = self
            .steps
            .values()
            .flat_map(|s| s.inputs.iter().chain(s.outputs.iter()))
            .map(|r| &*r.path)
            .collect();
        let records: usize = self
            .steps
            .values()
            .map(|s| s.inputs.len() + s.outputs.len())
            .sum();

        let mut out = String::new();
        out.push_str(&format!("schema_version = {}\n", self.schema_version));
        out.push_str(&format!(
            "# {} step(s), {} record(s), {} distinct path(s)\n",
            self.steps.len(),
            records,
            distinct.len()
        ));

        if !self.globs.is_empty() {
            out.push_str("\n[globs]\n");
            for (pattern, members) in &self.globs {
                let list: Vec<String> = members.iter().map(|m| quoted(m)).collect();
                out.push_str(&format!("{} = [{}]\n", quoted(pattern), list.join(", ")));
            }
        }

        for (key, step) in &self.steps {
            out.push_str(&format!("\n[steps.{}]\n", quoted(key)));
            out.push_str(&format!("command_hash = \"{:016x}\"\n", step.command_hash));
            out.push_str(&format!(
                "env_contribution = \"{:016x}\"\n",
                step.env_contribution
            ));
            out.push_str(&format!(
                "seal_contribution = \"{:016x}\"\n",
                step.seal_contribution
            ));
            for (label, records) in [("inputs", &step.inputs), ("outputs", &step.outputs)] {
                if records.is_empty() {
                    out.push_str(&format!("{label} = []\n"));
                    continue;
                }
                out.push_str(&format!("{label} = [\n"));
                for r in records.iter() {
                    out.push_str(&record_line(r));
                    out.push('\n');
                }
                out.push_str("]\n");
            }
        }
        out
    }
}

/// Extensions of index files this cook no longer reads: pre-v4 bincode
/// (`.bin`), v4..v6 TOML (`.toml`), and the torn temp files either could
/// leave behind. `.idx.tmp` is included so a crash mid-save does not leave a
/// partial file lying next to the real one forever.
///
/// Deliberately a denylist of *known superseded index* extensions rather than
/// an allowlist of `.idx`. The cache dir is not exclusively ours: modules keep
/// state beside the indexes (`cook_cc.json` is in every cc-built project's
/// `.cook/cache/`), and an allowlist sweep would delete it.
const SUPERSEDED_INDEX_EXTS: &[&str] = &[".bin", ".bin.tmp", ".toml", ".toml.tmp", ".idx.tmp"];

/// The on-disk extension of a v7+ recipe index.
const INDEX_EXT: &str = ".idx";

/// Hygiene sweep (COOK-92, extended by COOK-313/CS-0166): delete index files
/// written by a cook whose format this binary no longer reads, sitting
/// directly inside `cache_dir`. There is no migration — the index is
/// regeneratable, so a superseded file is dead weight, not data.
///
/// Non-recursive on purpose: `.cook/cache/tests/` (the JSON test cache) and
/// any other subdirectory are never touched. The artifact store lives under
/// a different root entirely (`~/.cache/cook/...`) and is out of scope.
/// Idempotent and infallible: a missing dir or a failed unlink is ignored
/// (the next construction retries).
pub fn sweep_superseded_indexes(cache_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SUPERSEDED_INDEX_EXTS.iter().any(|ext| name.ends_with(ext)) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
#[path = "tests/store_tests.rs"]
mod tests;
