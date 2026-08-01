//! Stale-output reconciliation (§17.7, CS-0093).
//!
//! When a recipe's declared output set *shrinks* between runs, the outputs it
//! no longer declares must not be left behind on disk. This module holds the
//! pure, hash-guarded sweep used by [`crate::run`]: given a recipe's prior
//! recorded outputs (absolute path → recorded content hash) and the current
//! cross-recipe live output set, it removes the orphaned files Cook itself
//! wrote — and only those, leaving any user-modified file in place.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cook_cache::hash_file;

/// Outcome of a sweep: which orphans were removed and which were kept because
/// they had changed since Cook recorded them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    swept: Vec<PathBuf>,
    kept_modified: Vec<PathBuf>,
}

impl SweepReport {
    /// Files removed because their on-disk content still matched what Cook
    /// recorded when it wrote them.
    pub fn swept(&self) -> &[PathBuf] {
        &self.swept
    }

    /// Orphaned files kept in place because their content changed since Cook
    /// wrote them (the hash guard).
    pub fn kept_modified(&self) -> &[PathBuf] {
        &self.kept_modified
    }

    /// True when the sweep neither removed nor flagged anything.
    pub fn is_empty(&self) -> bool {
        self.swept.is_empty() && self.kept_modified.is_empty()
    }
}

/// Sweep orphaned outputs (§17.7).
///
/// `prior` maps each output a recipe recorded last run to the content hash
/// Cook stored for it; `live` is the set of outputs declared by any recipe
/// reached this run. For each prior path not in `live`:
///
/// - absent on disk → no action;
/// - a regular file whose current hash equals the recorded hash → removed
///   (recorded in [`SweepReport::swept`]);
/// - a regular file whose hash differs → kept (recorded in
///   [`SweepReport::kept_modified`]);
/// - anything that is not a regular file (e.g. a directory) → left in place.
///
/// Cook only ever deletes files it itself wrote and recorded, and never one a
/// user has since changed.
pub fn sweep(prior: &BTreeMap<PathBuf, u64>, live: &BTreeSet<PathBuf>) -> SweepReport {
    let mut report = SweepReport::default();
    for (path, recorded_hash) in prior {
        if live.contains(path) {
            continue; // still declared this run — live, not an orphan.
        }
        if !is_regular_file(path) {
            continue; // absent, or a directory (files only, §17.7).
        }
        match hash_file(path) {
            Some(h) if h == *recorded_hash => {
                cook_cache::statmemo::disarm();
                if std::fs::remove_file(path).is_ok() {
                    report.swept.push(path.clone());
                }
            }
            _ => report.kept_modified.push(path.clone()),
        }
    }
    report
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "tests/reconcile_tests.rs"]
mod tests;
