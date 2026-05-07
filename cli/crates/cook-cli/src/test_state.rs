// cli/crates/cook-cli/src/test_state.rs
//! Persistent test-run state (.cook/test-state.json).
//!
//! Phase 7 will implement the full read/write logic. For Phase 4 the functions
//! are no-op stubs: load always reports no prior state, save is a no-op.

use std::collections::BTreeSet;
use std::path::Path;
use cook_engine::{TestId, TestResult};

/// Load the set of test IDs that failed (or were blocked/timed-out) during the
/// most recent `cook --test` run.
///
/// Phase 7 implements this by reading `.cook/test-state.json`.
pub fn load_failed_set(_project_root: &Path) -> std::io::Result<BTreeSet<TestId>> {
    // Phase 7 implements this.
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no previous test run recorded at .cook/test-state.json",
    ))
}

/// Persist test results so `--rerun-failed` can read them on the next run.
///
/// Phase 7 implements this by writing `.cook/test-state.json`.
pub fn save(_project_root: &Path, _results: &[TestResult]) -> std::io::Result<()> {
    // Phase 7 implements this.
    Ok(())
}
