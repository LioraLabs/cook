//! The crate's per-run memos: input `mtime` lookups (COOK-306) and tool-binary
//! content hashes (COOK-414).
//!
//! Both exist for the same reason — a build asks the filesystem the same
//! question thousands of times per run — and they live together so that the
//! rule each one keeps is readable against the other's. They do NOT keep the
//! same rule, and the reason is worth stating once, at the top, because two
//! memos with two disciplines and no acknowledgement between them is what this
//! module was reorganised to stop being:
//!
//! > A `stat` memo cannot revalidate itself. The `stat` IS the cheap check it
//! > exists to avoid, so re-checking costs exactly what it saves, and the only
//! > available discipline is the blunt one: serve nothing after cook writes
//! > anything ([`StatMemo`], below). A hash memo can. One `metadata` call is
//! > nothing against re-reading a 60 MB binary, so [`ToolHashMemo`] pays it on
//! > every lookup and never has to be told when cook wrote something.
//!
//! Arm/disarm is therefore deliberately NOT extended to the hash memo, and the
//! attempt would be worse than the status quo: [`disarm`] fires on the first
//! executed command, and the register phase — where module code calls
//! `cook.tools.id` — runs entirely disarmed, so a gated hash memo would be dead
//! in exactly the workloads it was written for.
//!
//! # The mtime memo (COOK-306)
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

// ---------------------------------------------------------------------------
// The tool-hash memo (COOK-414)
// ---------------------------------------------------------------------------

/// What makes a memoised digest still true: the file's modification time and
/// its length, as one value. Cheap to re-read (`metadata`), and moved by every
/// write cook performs.
type FileIdentity = (std::time::SystemTime, u64);

/// A per-run memo for the SHA-256 of a resolved tool binary, revalidated on
/// every lookup.
///
/// # Why it exists
///
/// The same tool is fingerprinted once per probe NODE — five recipes sealing
/// one `web:tools` probe hash its binaries five times — and a binary like
/// `node` is ~60 MB. Without a memo, an all-cached workspace build spends
/// seconds re-hashing the same toolchain, and a module calling `cook.tools.id`
/// re-reads a binary the fingerprint pass already read.
///
/// # Why it revalidates rather than arms and disarms
///
/// Its predecessor memoised for the life of the process, justified by "one run
/// = one process". That justification does not hold: probe units are DAG nodes
/// evaluated inside `execute_dag`, so a `tools { }` input can name a binary an
/// upstream node rebuilt earlier in the same run, and an execute-phase module
/// can call `cook.tools.id` on one. Serving the pre-build hash there folds a
/// tool that no longer exists into a probe fingerprint, and into any sealed
/// probe value derived from it — a false hit on a store that crosses machines,
/// which is the worst failure this codebase has.
///
/// So a lookup re-reads the file's [`FileIdentity`] and serves the memoised
/// digest only while it is unchanged. Order matters: identity is read BEFORE
/// the bytes. A write that lands between the two stores the OLD identity
/// against the new digest, so the next lookup sees a moved identity and
/// re-reads; reading identity afterwards would store the new identity against
/// possibly-old bytes and pin the mistake for the rest of the run.
///
/// An unreadable path is not memoised at all: it hashes to all-zero, and a
/// tool that reappears must be read rather than remembered as missing.
pub struct ToolHashMemo {
    entries: Mutex<HashMap<PathBuf, (FileIdentity, [u8; 32])>>,
}

impl ToolHashMemo {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// SHA-256 of `path`'s current bytes, from the memo when the file's
    /// identity has not moved. Identical in result to
    /// [`crate::probe::hash_file_sha256`], including all-zero when the path
    /// cannot be read.
    pub fn hash(&self, path: &Path) -> [u8; 32] {
        let identity = file_identity(path);
        if let Some(current) = identity {
            if let Some((seen, hash)) = self.entries.lock().unwrap().get(path) {
                if *seen == current {
                    return *hash;
                }
            }
        }
        // The lock is not held across the read: hashing a large binary is the
        // expensive thing this memo exists to avoid, and holding it would
        // serialise every other tool lookup behind this one.
        let hash = crate::probe::hash_file_sha256(path);
        if let Some(current) = identity {
            self.entries
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), (current, hash));
        }
        hash
    }
}

impl Default for ToolHashMemo {
    fn default() -> Self {
        Self::new()
    }
}

fn file_identity(path: &Path) -> Option<FileIdentity> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// The one instance the tool-hashing paths share. Needs no arming: unlike
/// [`GLOBAL`], it holds no run-scoped state, only answers it can re-check.
static TOOL_HASHES: std::sync::LazyLock<ToolHashMemo> =
    std::sync::LazyLock::new(ToolHashMemo::new);

/// Memoised [`crate::probe::hash_file_sha256`] against the process-wide memo.
pub fn tool_hash_memo(path: &Path) -> [u8; 32] {
    TOOL_HASHES.hash(path)
}

#[cfg(test)]
#[path = "tests/statmemo_tests.rs"]
mod tests;
