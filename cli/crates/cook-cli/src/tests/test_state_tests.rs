use super::*;
use tempfile::tempdir;
use std::time::Duration;

fn mk(id: &str, outcome: TestOutcome) -> TestResult {
    TestResult {
        id: TestId(id.to_string()),
        namespace: String::new(),
        recipe: id.split(':').next().unwrap_or(id).to_string(),
        name: id.split(':').nth(1).unwrap_or("").to_string(),
        iteration_item: None,
        outcome,
        duration: Duration::from_millis(100),
        from_cache: false,
        stdout: String::new(),
        stderr: String::new(),
        fingerprint: None,
        blocked_by: None,
        should_fail: false,
        timed_out: false,
        line: 0,
        exit_code: None,
    }
}

#[test]
fn save_then_load_failed_returns_only_failed_blocked_timed_out() {
    let tmp = tempdir().unwrap();
    let results = vec![
        mk("r:a", TestOutcome::Passed),
        mk("r:b", TestOutcome::Failed),
        mk("r:c", TestOutcome::Blocked),
        mk("r:d", TestOutcome::TimedOut),
        mk("r:e", TestOutcome::Passed),
    ];
    save(tmp.path(), &results).unwrap();
    let failed = load_failed_set(tmp.path()).unwrap();
    assert_eq!(failed.len(), 3);
    assert!(failed.contains(&TestId("r:b".to_string())));
    assert!(failed.contains(&TestId("r:c".to_string())));
    assert!(failed.contains(&TestId("r:d".to_string())));
}

#[test]
fn load_missing_state_file_errors() {
    let tmp = tempdir().unwrap();
    let err = load_failed_set(tmp.path()).expect_err("must error");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn save_roundtrip_preserves_outcome_strings() {
        let tmp = tempdir().unwrap();
        let results = vec![
            mk("r:a", TestOutcome::Passed),
        mk("r:b", TestOutcome::Failed),
    ];
    save(tmp.path(), &results).unwrap();
    let bytes = std::fs::read(tmp.path().join(".cook/test-state.json")).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["results"][0]["outcome"], "passed");
    assert_eq!(json["results"][1]["outcome"], "failed");
}

#[test]
fn save_creates_parent_dir() {
    let tmp = tempdir().unwrap();
    // Don't pre-create .cook/
    let results = vec![mk("r:a", TestOutcome::Passed)];
    save(tmp.path(), &results).unwrap();
    assert!(tmp.path().join(".cook/test-state.json").exists());
}

#[test]
fn now_iso8601_looks_like_utc_timestamp() {
    let ts = now_iso8601();
    // Basic shape: YYYY-MM-DDTHH:MM:SSZ
    assert_eq!(ts.len(), 20, "unexpected length: {ts}");
    assert!(ts.ends_with('Z'), "must end with Z: {ts}");
    assert_eq!(&ts[4..5], "-");
    assert_eq!(&ts[7..8], "-");
    assert_eq!(&ts[10..11], "T");
}
