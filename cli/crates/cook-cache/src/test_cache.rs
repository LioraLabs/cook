//! Content-addressed test-result cache (CS-0061 §3.3).
//!
//! Only `Passed` outcomes are persisted. Failed / timed-out / blocked results
//! are excluded so a subsequent `cook test` always re-runs them.
//!
//! Layout on disk:
//! ```text
//! <local_root>/cache/tests/<fp_prefix>/<fp>.json
//! ```
//! where `fp_prefix` is the first two hex characters of the fingerprint
//! (after stripping the `sha256:` scheme prefix) and `fp` is the full
//! fingerprint with the scheme stripped.  This mirrors the shard layout used
//! by the artifact cache to keep directory fan-out bounded.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// TestCacheOutcome
// ---------------------------------------------------------------------------

/// The outcome stored in a cache entry. Only `Passed` entries are written;
/// see `TestCache::store` for the enforcement gate.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TestCacheOutcome {
    Passed,
}

// ---------------------------------------------------------------------------
// TestCacheEntry
// ---------------------------------------------------------------------------

/// One serialised test-result cache entry.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TestCacheEntry {
    /// Incremented when the on-disk format changes. Readers reject entries
    /// whose `schema_version != 1`.
    pub schema_version: u32,
    /// The fingerprint that addresses this entry (`sha256:<hex>`). Validated
    /// on `lookup` against the key used to look up the file so corrupted or
    /// mis-placed entries are rejected.
    pub fingerprint: String,
    pub outcome: TestCacheOutcome,
    pub stdout: String,
    pub stderr: String,
    /// Wall-clock seconds the test command ran for on the machine that wrote
    /// this entry. Used to surface realistic durations to the reporter on a
    /// cache hit.
    pub duration_secs: f64,
    /// Whether the test had `should_fail` set when it produced this entry.
    /// The reporter uses this to annotate cached results correctly.
    pub should_fail_observed: bool,
    /// ISO-8601 timestamp of when this entry was written.
    pub recorded_at: String,
}

// ---------------------------------------------------------------------------
// TestCache
// ---------------------------------------------------------------------------

/// Filesystem-backed content-addressed cache for test results.
pub struct TestCache {
    root: PathBuf,
}

impl TestCache {
    /// Construct a `TestCache` rooted at `<local_root>/cache/tests/`.
    ///
    /// `local_root` is typically the project's `.cook/` directory. The
    /// directory is created lazily on first `store` call.
    pub fn new(local_root: PathBuf) -> Self {
        Self {
            root: local_root.join("cache").join("tests"),
        }
    }

    /// Look up a cached test result by fingerprint.
    ///
    /// Returns `None` when:
    /// - the on-disk file does not exist,
    /// - the file cannot be read or is not valid JSON,
    /// - `schema_version != 1`, or
    /// - the stored fingerprint does not match `fingerprint` (tamper / rename guard).
    pub fn lookup(&self, fingerprint: &str) -> Option<TestCacheEntry> {
        let path = self.path_for(fingerprint);
        if !path.exists() {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        let entry: TestCacheEntry = serde_json::from_slice(&bytes).ok()?;
        if entry.schema_version != 1 {
            return None;
        }
        if entry.fingerprint != fingerprint {
            return None;
        }
        Some(entry)
    }

    /// Persist a test-result entry to the cache.
    ///
    /// Only `Passed` entries are written (CS-0061 §3.3). Calling this with a
    /// non-Passed entry is a no-op and returns `Ok(())` so callers do not need
    /// an additional guard.
    ///
    /// Writes are atomic: the JSON is written to a `.tmp` sibling, then
    /// renamed over the final path. This prevents readers from observing a
    /// partially-written file if the process is killed mid-write.
    pub fn store(&self, fingerprint: &str, entry: &TestCacheEntry) -> std::io::Result<()> {
        // Only Passed entries are cached per CS-0061 §3.3.
        if !matches!(entry.outcome, TestCacheOutcome::Passed) {
            return Ok(());
        }
        let path = self.path_for(fingerprint);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(entry)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Derive the on-disk path for `fingerprint`.
    ///
    /// Strips the `sha256:` scheme prefix then shards on the first two hex
    /// characters: `.cook/cache/tests/<prefix>/<full>.json`.
    pub fn path_for(&self, fingerprint: &str) -> PathBuf {
        let stripped = fingerprint.strip_prefix("sha256:").unwrap_or(fingerprint);
        let prefix_len = 2.min(stripped.len());
        let prefix = &stripped[..prefix_len];
        self.root.join(prefix).join(format!("{stripped}.json"))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/test_cache_tests.rs"]
mod tests;
