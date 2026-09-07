//! The crate's per-run memos: input `mtime` lookups (COOK-306) and tool-binary
//! content hashes (COOK-414).
//!
//! Both exist for the same reason (a build asks the filesystem the same
//! question thousands of times per run) and they live together so that the rule
//! each one keeps is readable against the other's. They do NOT keep the same
//! rule, and the reason is worth stating once, at the top, because two memos
//! with two disciplines and no acknowledgement between them is what this module
//! was reorganised to stop being:
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
//! executed command, and the register phase, where module code calls
//! `cook.tools.id`, runs entirely disarmed. A gated hash memo would be dead in
//! exactly the workloads it was written for.
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
type Entries = HashMap<PathBuf, HashMap<String, Option<FileObservation>>>;

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
        self.observe(working_dir, rel).map(|o| o.mtime)
    }

    pub fn observe(&self, working_dir: &Path, rel: &str) -> Option<FileObservation> {
        if !self.is_armed() {
            return observe_file(&working_dir.join(rel));
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
        let result = observe_file(&working_dir.join(rel));
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

pub use cook_contracts::cache::step::FileIdentity;

/// One stat, shared by mtime reporting and strong content validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileObservation {
    pub mtime: u64,
    pub identity: Option<FileIdentity>,
}

impl FileObservation {
    pub fn matches(&self, record: &crate::FileRecord) -> bool {
        self.identity.is_some() && self.identity == record.identity
    }
}

/// Observe before reading bytes: observing afterwards could attach newer
/// metadata to stale bytes and make a racing write invisible on the next run.
pub fn observe_file(path: &Path) -> Option<FileObservation> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        Some(FileIdentity {
            mtime_secs: modified.as_secs(), mtime_nanos: modified.subsec_nanos(),
            len: meta.len(), ctime_secs: meta.ctime(), ctime_nanos: meta.ctime_nsec(),
            ino: meta.ino(), dev: meta.dev(),
        })
    };
    #[cfg(not(unix))]
    let identity = None;
    Some(FileObservation {
        mtime: modified.as_millis().min(i64::MAX as u128) as u64,
        identity,
    })
}

pub fn observe_file_memo(working_dir: &Path, rel: &str) -> Option<FileObservation> {
    GLOBAL.observe(working_dir, rel)
}

/// A per-run memo for the SHA-256 of a resolved tool binary, revalidated on
/// every lookup.
///
/// # Why it exists
///
/// The same tool is digested once per probe NODE (five recipes sealing one
/// `web:tools` probe hash its binaries five times) and a binary like `node` is
/// ~60 MB. Without a memo, an all-cached workspace build spends seconds
/// re-hashing the same toolchain, and a module calling `cook.tools.id` re-reads
/// a binary a `tools` declaration already read.
///
/// # Why it revalidates rather than arms and disarms
///
/// Its predecessor memoised for the life of the process, justified by "one run
/// = one process". That justification does not hold: probe units are DAG nodes
/// evaluated inside `execute_dag`, so a top-level `tools` declaration can name a binary an
/// upstream node rebuilt earlier in the same run, and an execute-phase module
/// can call `cook.tools.id` on one. Serving the pre-build hash there folds a
/// tool that no longer exists into a probe's observed value, and into any
/// sealed consumer's key: a false hit on a store that crosses machines,
/// which is the worst failure this codebase has. §24.9 of the Standard says as
/// much normatively (CS-0212), and the predecessor did not meet it.
///
/// So a lookup re-reads the file's [`FileIdentity`] and serves the memoised
/// digest only while it is unchanged. Order matters: identity is read BEFORE
/// the bytes. A write that lands between the two stores the OLD identity
/// against the new digest, so the next lookup sees a moved identity and
/// re-reads; reading identity afterwards would store the new identity against
/// possibly-old bytes and pin the mistake for the rest of the run. That is also
/// why the insert reuses the identity read at the top rather than re-statting.
///
/// # What it still cannot see
///
/// Stated rather than asserted away, because this sits on a false-hit path. A
/// memoised digest is served whenever every field [`FileIdentity`] holds still
/// reads the same, so the memo is exactly as discriminating as the fields it
/// keeps. On unix a rewrite would have to reproduce mtime, ctime, length, inode
/// and device, which cook cannot do to itself and an attacker with write access
/// to the toolchain does not need. On a platform with no ctime or inode, no metadata shortcut is taken.
///
/// A path whose `metadata` call fails is not memoised at all, though an entry
/// made earlier is not deleted either; it simply cannot be served while the
/// stat keeps failing.
///
/// A path that stats but cannot be READ is memoised, at the all-zero digest
/// [`crate::probe::hash_file_sha256`] returns for it. This is reachable because
/// `which` selects on `X_OK`, not `R_OK`, so an execute-only binary gets here.
/// On unix that entry is invalidated when the permission changes, because
/// `chmod` moves ctime. Off Unix the tool is read again on every lookup.
pub struct ToolHashMemo {
    entries: Mutex<HashMap<PathBuf, (FileIdentity, [u8; 32])>>,
    reads: std::sync::atomic::AtomicUsize,
}

impl ToolHashMemo {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            reads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// SHA-256 of `path`'s current bytes, from the memo when the file's
    /// identity has not moved. Identical in result to
    /// [`crate::probe::hash_file_sha256`], including all-zero when the path
    /// cannot be read.
    pub fn hash(&self, path: &Path) -> [u8; 32] {
        let identity = observe_file(path).and_then(|o| o.identity);
        if let Some(current) = identity {
            if let Some((seen, hash)) = self.entries.lock().unwrap().get(path) {
                if *seen == current {
                    return *hash;
                }
            }
        }
        // The lock is not held across the read: hashing a large binary is the
        // expensive thing this memo exists to avoid, and holding it would
        // serialise every other tool lookup behind this one. Two threads can
        // therefore race the same cold path and both read; and a slow thread
        // can overwrite a fresher entry with the identity it observed before
        // reading. Neither can produce a wrong ANSWER: an entry is only served
        // when the file's identity still equals the one its writer observed,
        // so the worst case is a redundant re-read, and it self-heals on the
        // next insert.
        self.reads.fetch_add(1, Ordering::Relaxed);
        let hash = crate::probe::hash_file_sha256(path);
        if let Some(current) = identity {
            self.entries
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), (current, hash));
        }
        hash
    }

    /// How many times this memo has gone to the file rather than answering
    /// from its map. A read that fails (a vanished or unreadable path) counts:
    /// the point of the number is the work not avoided.
    ///
    /// The memo's whole purpose is a read it does NOT perform, and that is not
    /// observable in its return value: a correct memo and a memo that re-reads
    /// every time answer identically. Counting the reads is how a test proves
    /// the memoisation without pinning a stale digest as if it were a
    /// requirement.
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }
}

impl Default for ToolHashMemo {
    fn default() -> Self {
        Self::new()
    }
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
