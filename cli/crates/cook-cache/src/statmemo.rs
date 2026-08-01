//! Per-run memo for input `mtime` lookups (COOK-306).
//!
//! A large C++ graph records the same header in the input set of every
//! translation unit that includes it. DuckDB's `duckdb_lib` index holds
//! 648,153 input records that resolve to only 8,350 distinct paths — 77x
//! redundancy, inherent to how C++ headers fan out. Validating a settled
//! build therefore issued ~648k `stat` calls where ~8.4k would do (measured:
//! 0.88s versus 0.01s).
//!
//! # Why this is safe
//!
//! A memoised mtime is only wrong if cook writes the file after the mtime was
//! read. So the memo is *armed* at the start of a run and **permanently
//! disarmed by the first write cook performs** — [`disarm`] is called from
//! every point that executes a command, restores an artifact, sweeps a stale
//! output, or writes a file from Lua. The invariant is deliberately blunt and
//! auditable:
//!
//! > the memo only ever serves values read before cook wrote anything.
//!
//! Disarming (rather than selectively invalidating) also costs nothing in
//! practice: once a run is executing commands, the compile and link work
//! dwarfs the stat traffic the memo was there to remove.
//!
//! Writes cook does *not* perform — a source file edited by the user midway
//! through a run — are outside the memo's remit, exactly as they are outside
//! a plain `stat`'s: a build that races an editor has no defined input set.
//!
//! Three process-spawn sites deliberately have no hook, because none of them
//! can write to a working tree the memo has read:
//!
//! - `cook verify`'s `rerun_outputs_in_sandbox` runs its command with
//!   `current_dir` set to a private tempdir copy.
//! - the luarocks driver runs during module resolution, before [`arm`].
//! - `cook.sh` (`cook-register`'s `run_shell_command`) belongs to the
//!   register-phase Lua VM, which finishes — finalizers included — before
//!   `execute_dag` arms anything. [`arm`] also clears the map, so a second
//!   register pass in the same process cannot leave a stale entry behind.
//!   Do not "fix" this by adding a hook there: `cook-register` is a
//!   language-surface path under `.githooks/pre-commit`, and the hook would be
//!   dead code bought at the price of a spec-pairing requirement.
//!
//! The process-wide memo starts disarmed, so every consumer that has not
//! opted in (the DAG viewer, `cook verify`, unit tests) keeps issuing plain
//! `stat` calls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Two-level so a lookup allocates nothing: the outer key is the unit's
/// working directory (`&Path` borrows from `PathBuf`), the inner key the
/// recorded relative path (`&str` borrows from `String`). Only a miss pays for
/// the `working_dir.join(rel)` that the syscall needs.
type Entries = HashMap<PathBuf, HashMap<String, Option<u64>>>;

/// An arm/disarm-gated `stat` memo. The engine drives the process-wide
/// instance through the free functions below; tests construct their own so
/// they never contend on shared state.
pub struct StatMemo {
    armed: AtomicBool,
    entries: Mutex<Entries>,
}

impl StatMemo {
    pub fn new() -> Self {
        Self {
            armed: AtomicBool::new(false),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Arm for a run that has not written anything yet. Callers MUST NOT
    /// re-arm mid-run: [`Self::disarm`] is what makes the invariant hold.
    pub fn arm(&self) {
        self.entries.lock().unwrap().clear();
        self.armed.store(true, Ordering::Release);
    }

    /// Disarm permanently: cook is about to write to (or has just written to)
    /// the working tree, so no memoised mtime can be trusted for the rest of
    /// the run. Cheap enough to call unconditionally on any write path.
    pub fn disarm(&self) {
        if self.armed.swap(false, Ordering::AcqRel) {
            self.entries.lock().unwrap().clear();
        }
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Acquire)
    }

    /// Get `working_dir/rel`'s mtime, serving a memoised answer while armed.
    /// Identical in result to [`crate::stat_mtime`] on the joined path.
    pub fn stat_mtime(&self, working_dir: &Path, rel: &str) -> Option<u64> {
        if !self.is_armed() {
            return crate::check::stat_mtime(&working_dir.join(rel));
        }
        if let Some(hit) = self
            .entries
            .lock()
            .unwrap()
            .get(working_dir)
            .and_then(|by_rel| by_rel.get(rel))
        {
            return *hit;
        }
        let result = crate::check::stat_mtime(&working_dir.join(rel));
        // Re-check: a concurrent write may have disarmed us while the stat was
        // in flight, in which case this value must not be published.
        if self.is_armed() {
            self.entries
                .lock()
                .unwrap()
                .entry(working_dir.to_path_buf())
                .or_default()
                .insert(rel.to_string(), result);
        }
        result
    }
}

impl Default for StatMemo {
    fn default() -> Self {
        Self::new()
    }
}

/// The one instance the engine drives. Starts disarmed.
static GLOBAL: std::sync::LazyLock<StatMemo> = std::sync::LazyLock::new(StatMemo::new);

/// Arm the process-wide memo. Called by the engine once the DAG is built and
/// registration (with every probe capture it ran) is complete, so nothing has
/// written to the tree since the last stat.
pub fn arm() {
    GLOBAL.arm();
}

/// Disarm the process-wide memo. Called from every write and execute path.
pub fn disarm() {
    GLOBAL.disarm();
}

/// Memoised [`crate::stat_mtime`] against the process-wide memo.
pub fn stat_mtime_memo(working_dir: &Path, rel: &str) -> Option<u64> {
    GLOBAL.stat_mtime(working_dir, rel)
}

#[cfg(test)]
#[path = "tests/statmemo_tests.rs"]
mod tests;
