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
use crate::iso8601::now_iso8601;

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

// now_iso8601 is imported from crate::iso8601 (see top-level use).

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
            line: 0,
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
