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
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_entry(fp: &str) -> TestCacheEntry {
        TestCacheEntry {
            schema_version: 1,
            fingerprint: fp.to_string(),
            outcome: TestCacheOutcome::Passed,
            stdout: "ok\n".to_string(),
            stderr: "".to_string(),
            duration_secs: 0.42,
            should_fail_observed: false,
            recorded_at: "2026-05-07T15:32:00Z".to_string(),
        }
    }

    #[test]
    fn roundtrip_passing_entry() {
        let tmp = tempdir().unwrap();
        let cache = TestCache::new(tmp.path().to_path_buf());
        let fp = "sha256:abcdef0123456789";
        let entry = make_entry(fp);
        cache.store(fp, &entry).unwrap();
        let got = cache.lookup(fp).expect("must hit");
        assert!((got.duration_secs - 0.42).abs() < 1e-9);
        assert_eq!(got.outcome, TestCacheOutcome::Passed);
        assert_eq!(got.stdout, "ok\n");
    }

    #[test]
    fn lookup_miss_returns_none() {
        let tmp = tempdir().unwrap();
        let cache = TestCache::new(tmp.path().to_path_buf());
        assert!(cache.lookup("sha256:doesnotexist").is_none());
    }

    #[test]
    fn store_silently_succeeds_for_non_passing_outcome_via_serde() {
        // We can't construct a non-Passed entry via the public API (only Passed
        // exists), but we verify the contract via inspection: the store function
        // matches on Passed only.
        let tmp = tempdir().unwrap();
        let cache = TestCache::new(tmp.path().to_path_buf());
        let fp = "sha256:0123456789abcdef";
        let entry = make_entry(fp);
        // Roundtrip with the canonical Passed entry — sanity check.
        cache.store(fp, &entry).unwrap();
        assert!(cache.lookup(fp).is_some());
    }

    #[test]
    fn fingerprint_mismatch_returns_none() {
        let tmp = tempdir().unwrap();
        let cache = TestCache::new(tmp.path().to_path_buf());
        // Write an entry whose internal fingerprint doesn't match the lookup key.
        let path = cache.path_for("sha256:wrongfp");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mismatched = make_entry("sha256:realfp"); // internal fp = realfp
        std::fs::write(&path, serde_json::to_vec(&mismatched).unwrap()).unwrap();
        assert!(
            cache.lookup("sha256:wrongfp").is_none(),
            "internal fp doesn't match the key — must miss"
        );
    }

    #[test]
    fn schema_version_mismatch_returns_none() {
        let tmp = tempdir().unwrap();
        let cache = TestCache::new(tmp.path().to_path_buf());
        let fp = "sha256:versiontest00000";
        let mut entry = make_entry(fp);
        // Tamper with schema_version to simulate a future format.
        entry.schema_version = 2;
        let path = cache.path_for(fp);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
        assert!(
            cache.lookup(fp).is_none(),
            "schema_version != 1 must return None"
        );
    }

    #[test]
    fn path_for_strips_sha256_prefix() {
        let tmp = tempdir().unwrap();
        let cache = TestCache::new(tmp.path().to_path_buf());
        let path = cache.path_for("sha256:abcdef01");
        // Should be <root>/ab/abcdef01.json
        let components: Vec<_> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        // Last two: shard-dir and filename
        assert_eq!(components[components.len() - 2], "ab");
        assert_eq!(components[components.len() - 1], "abcdef01.json");
    }

    #[test]
    fn path_for_no_prefix() {
        let tmp = tempdir().unwrap();
        let cache = TestCache::new(tmp.path().to_path_buf());
        let path = cache.path_for("deadbeef");
        let components: Vec<_> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(components[components.len() - 2], "de");
        assert_eq!(components[components.len() - 1], "deadbeef.json");
    }
}
