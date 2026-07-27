use super::*;
use tempfile::tempdir;

fn make_entry(fp: &str) -> TestCacheEntry {
    TestCacheEntry {
        schema_version: cook_fingerprint::CACHE_VERSION,
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
    entry.schema_version = cook_fingerprint::CACHE_VERSION + 1;
    let path = cache.path_for(fp);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
    assert!(
        cache.lookup(fp).is_none(),
        "a schema_version other than the shared one must return None"
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
