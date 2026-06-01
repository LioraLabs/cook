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

use cook_fingerprint::hash_file;

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
mod tests {
    use super::*;

    fn write(p: &Path, contents: &str) {
        std::fs::write(p, contents).unwrap();
    }

    #[test]
    fn sweeps_unmodified_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let orphan = dir.path().join("jack.txt");
        write(&orphan, "Jack of Diamonds\n");
        let hash = hash_file(&orphan).unwrap();

        let mut prior = BTreeMap::new();
        prior.insert(orphan.clone(), hash);
        let live = BTreeSet::new(); // no longer declared

        let report = sweep(&prior, &live);
        assert_eq!(report.swept(), &[orphan.clone()]);
        assert!(report.kept_modified().is_empty());
        assert!(!orphan.exists(), "unmodified orphan must be removed");
    }

    #[test]
    fn keeps_modified_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let orphan = dir.path().join("jack.txt");
        write(&orphan, "Jack of Diamonds\n");
        let recorded = hash_file(&orphan).unwrap();
        // User edits the file after Cook wrote it.
        write(&orphan, "HAND EDITED\n");

        let mut prior = BTreeMap::new();
        prior.insert(orphan.clone(), recorded);
        let report = sweep(&prior, &BTreeSet::new());

        assert!(report.swept().is_empty());
        assert_eq!(report.kept_modified(), &[orphan.clone()]);
        assert!(orphan.exists(), "modified orphan must be kept");
    }

    #[test]
    fn live_output_is_not_swept() {
        let dir = tempfile::tempdir().unwrap();
        let still_here = dir.path().join("ace.txt");
        write(&still_here, "Ace of Spades\n");
        let hash = hash_file(&still_here).unwrap();

        let mut prior = BTreeMap::new();
        prior.insert(still_here.clone(), hash);
        let mut live = BTreeSet::new();
        live.insert(still_here.clone()); // still declared this run

        let report = sweep(&prior, &live);
        assert!(report.is_empty());
        assert!(still_here.exists());
    }

    #[test]
    fn absent_orphan_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone.txt");
        let mut prior = BTreeMap::new();
        prior.insert(gone.clone(), 12345);
        let report = sweep(&prior, &BTreeSet::new());
        assert!(report.is_empty());
    }

    #[test]
    fn directory_orphan_is_left_in_place() {
        // Files only (§17.7): a directory matching a prior path is never swept.
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("build");
        std::fs::create_dir(&subdir).unwrap();
        let mut prior = BTreeMap::new();
        prior.insert(subdir.clone(), 0);
        let report = sweep(&prior, &BTreeSet::new());
        assert!(report.is_empty());
        assert!(subdir.exists());
    }
}
