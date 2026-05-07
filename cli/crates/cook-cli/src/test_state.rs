//! Persistence of last-run test outcomes for `--rerun-failed`.
//!
//! Layout: `.cook/test-state.json` at the project root. Schema:
//! ```json
//! {
//!   "schema_version": 1,
//!   "ran_at": "2026-05-07T15:32:00Z",
//!   "results": [
//!     { "id": "frontend.unit:vitest-suite", "outcome": "failed", ... }
//!   ]
//! }
//! ```
//!
//! `load_failed_set` returns the subset of TestIds whose last-run outcome was
//! `failed`, `blocked`, or `timed_out` — the set `--rerun-failed` should re-run.
//!
//! Per docs/superpowers/specs/2026-05-07-test-runner-design.md §4.7.

use std::collections::BTreeSet;
use std::path::Path;
use serde::{Serialize, Deserialize};
use cook_engine::{TestId, TestOutcome, TestResult};

const STATE_FILE: &str = ".cook/test-state.json";
const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct StateFile {
    schema_version: u32,
    ran_at: String,
    results: Vec<StateEntry>,
}

#[derive(Serialize, Deserialize)]
struct StateEntry {
    id: String,
    outcome: String,
    duration_secs: f64,
    from_cache: bool,
}

/// Load the set of test IDs that failed (or were blocked/timed-out) during the
/// most recent `cook --test` run.
///
/// Returns `Err(NotFound)` when no state file exists yet.
/// Returns an empty set (with a warning) when the schema version doesn't match.
pub fn load_failed_set(project_root: &Path) -> std::io::Result<BTreeSet<TestId>> {
    let path = project_root.join(STATE_FILE);
    if !path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no previous test run recorded at {STATE_FILE}"),
        ));
    }
    let bytes = std::fs::read(&path)?;
    let state: StateFile = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if state.schema_version != SCHEMA_VERSION {
        eprintln!(
            "warning: {} is schema_version {}; expected {} — ignoring",
            STATE_FILE, state.schema_version, SCHEMA_VERSION,
        );
        return Ok(BTreeSet::new());
    }
    Ok(state
        .results
        .iter()
        .filter(|e| matches!(e.outcome.as_str(), "failed" | "blocked" | "timed_out"))
        .map(|e| TestId(e.id.clone()))
        .collect())
}

/// Persist test results so `--rerun-failed` can read them on the next run.
///
/// Writes atomically via a temp file + rename so crashes mid-write leave the
/// previous state file intact.
pub fn save(project_root: &Path, results: &[TestResult]) -> std::io::Result<()> {
    let path = project_root.join(STATE_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let state = StateFile {
        schema_version: SCHEMA_VERSION,
        ran_at: now_iso8601(),
        results: results
            .iter()
            .map(|r| StateEntry {
                id: r.id.0.clone(),
                outcome: outcome_to_str(r.outcome).to_string(),
                duration_secs: r.duration.as_secs_f64(),
                from_cache: r.from_cache,
            })
            .collect(),
    };
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(&state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn outcome_to_str(o: TestOutcome) -> &'static str {
    match o {
        TestOutcome::Passed => "passed",
        TestOutcome::Failed => "failed",
        TestOutcome::Blocked => "blocked",
        TestOutcome::TimedOut => "timed_out",
    }
}

fn now_iso8601() -> String {
    // Manually format SystemTime as YYYY-MM-DDTHH:MM:SSZ without a chrono dep.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let (year, month, day) = days_to_ymd(days as i64);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, min, sec
    )
}

fn days_to_ymd(days_since_epoch: i64) -> (i32, u32, u32) {
    // Algorithm from Howard Hinnant's "date" library (public domain).
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 {
        days / 146_097
    } else {
        (days - 146_096) / 146_097
    };
    let doe = (days - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::time::Duration;

    fn mk(id: &str, outcome: TestOutcome) -> TestResult {
        TestResult {
            id: TestId(id.to_string()),
            namespace: String::new(),
            recipe: id.split(':').next().unwrap_or(id).to_string(),
            name: id.split(':').nth(1).unwrap_or("").to_string(),
            suite: String::new(),
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
}
