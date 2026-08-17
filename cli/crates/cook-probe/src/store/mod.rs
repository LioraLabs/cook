//! Both halves of the `.cook/probes/<key>.json` contract (§22.5.8, CS-0102):
//! [`materialize_value`] writes a probe's canonical value, [`ProbeValueStore`]
//! reads it back. They were one crate apart until COOK-422 — the writer here,
//! the reader in the execute-phase VM crate that happened to need it first —
//! which is how a file format ends up with two owners and no agreement.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn materialize_value(dir: &Path, key: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = cook_contracts::probe::value::probe_file_name(key);
    let destination = dir.join(&name);
    let sequence = WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = dir.join(format!(".{name}.tmp-{}-{sequence}", std::process::id()));

    if let Err(error) = std::fs::write(&temporary, bytes) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temporary, &destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }

    Ok(destination)
}

/// The bytes [`materialize_value`] last wrote for `key` under `dir`, or
/// `None` when nothing has. The read half of the same one-file contract, so
/// the filename is spelled once for both directions.
///
/// Free rather than a [`ProbeValueStore`] method because [`crate::eval`] needs
/// it before any per-run store exists: COOK-526's cross-phase serve reads the
/// value the REGISTER pass materialised, in a phase that holds only an
/// `EvalCtx`.
pub fn read_value(dir: &Path, key: &str) -> Option<Vec<u8>> {
    std::fs::read(dir.join(cook_contracts::probe::value::probe_file_name(key))).ok()
}

/// Per-run probe-value store (§22.5.8). The canonical value of a probe is
/// the file [`materialize_value`] writes above (CS-0102). During a normal
/// `execute_dag` run this store is populated by explicit [`ProbeValueStore::insert`]
/// calls from the engine scheduler after `cook_probe::eval::lookup`/`record`
/// resolves each value — CS-0243 removed the probe-value cache, so nothing
/// here is ever read back from disk during that run. [`ProbeValueStore::get`]'s
/// `dir`-backed fallback exists for exactly one other caller, `cook why`
/// (`cook-engine::why`), which attaches a `.cook/probes` directory to read an
/// EARLIER invocation's on-disk record when no scheduler ran in this process
/// to populate the map. It is NOT a cross-VM shared-memory channel: within
/// one run, the engine scheduler and every worker's `cook.probes.get` share
/// this same in-memory map.
///
/// Locking: one mutex guards the whole map. [`Self::get`] is still
/// literally read-through — a map miss falls to [`read_value`] and inserts
/// the result, and the `dir`-backed fallback is used exclusively by `cook why`. The lock is
/// held across that whole path (miss, disk read, insert), not just the
/// map lookup. Probe files are tiny; simplicity beats contention here, and
/// double-checked locking is how the historical pool nil-index race
/// (W13) family of bugs happens — don't.
#[derive(Clone, Default)]
pub struct ProbeValueStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    dir: Option<PathBuf>,
    map: BTreeMap<String, Vec<u8>>,
    /// CS-0157: per-run tool-path metadata, probe key → (tool name →
    /// freshly-resolved path). Populated by the engine when it resolves a
    /// probe's declared `inputs.tools` for the fingerprint; merged into the
    /// Lua READ VIEW by `cook.probes.get`. Never persisted, never part of
    /// the canonical value bytes, never folded into any key.
    tool_paths: BTreeMap<String, BTreeMap<String, String>>,
}

impl ProbeValueStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Point [`Self::get`]'s disk fallback at a `.cook/probes` directory.
    /// `execute_dag` no longer calls this — CS-0243 removed the
    /// probe-value cache the fallback used to read through. The only
    /// caller left is `cook why` (`cli/crates/cook-engine/src/why.rs`),
    /// reading an earlier invocation's on-disk records after the fact.
    pub fn attach_dir(&self, dir: PathBuf) {
        self.inner.lock().unwrap().dir = Some(dir);
    }

    pub fn insert(&self, key: &str, bytes: Vec<u8>) {
        self.inner.lock().unwrap().map.insert(key.to_string(), bytes);
    }

    /// CS-0157: record the freshly-resolved paths of a probe's declared
    /// tools for this run. Read-view metadata only (see `Inner::tool_paths`).
    pub fn set_tool_paths(&self, key: &str, paths: BTreeMap<String, String>) {
        self.inner
            .lock()
            .unwrap()
            .tool_paths
            .insert(key.to_string(), paths);
    }

    /// The per-run tool-path metadata recorded for `key`, if any.
    pub fn tool_paths(&self, key: &str) -> Option<BTreeMap<String, String>> {
        self.inner.lock().unwrap().tool_paths.get(key).cloned()
    }

    /// Map lookup, then the file `materialize_value` writes (caching the
    /// file bytes on success). The filename comes from the one function
    /// the writer above calls, so the two ends cannot spell it differently.
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(b) = inner.map.get(key) {
            return Some(b.clone());
        }
        let dir = inner.dir.clone()?;
        let bytes = read_value(&dir, key)?;
        inner.map.insert(key.to_string(), bytes.clone());
        Some(bytes)
    }

    /// CS-0157: a probe value's READ VIEW — the canonical value with this
    /// run's tool-path metadata merged in. The merge is
    /// `cook_contracts::probe::value::merge_tool_paths`, applied to the JSON
    /// value BEFORE any Lua conversion, so `cook.probes.get` and `$<key>`
    /// substitution (CS-0192) build the same view from the same bytes and
    /// cannot drift.
    ///
    /// A method rather than a free function taking a store: the only thing
    /// it reads is this store's `tool_paths`, and the two always travelled
    /// together at every call site.
    pub fn read_view(&self, key: &str, bytes: &[u8]) -> Result<serde_json::Value, String> {
        let mut value = cook_contracts::probe::value::decode_json(bytes)?;
        if let Some(paths) = self.tool_paths(key) {
            cook_contracts::probe::value::merge_tool_paths(&mut value, &paths);
        }
        Ok(value)
    }
}

/// CS-0152: what a reader is told when `key` (already the full,
/// scope-prefixed key if applicable) has never been materialised — the step
/// never demanded this probe. One sentence for every reader: the unscoped
/// and scoped `cook.probes.get`, and `$<key>` substitution, which reads the
/// same store outside any VM. Returning `nil` instead let real misses
/// masquerade as legitimate probe-absent results.
pub fn not_materialised_message(key: &str) -> String {
    format!(
        "cook.probes.get(\"{key}\"): probe value not materialised — this step never \
         demanded probe '{key}'. Reference the probe in this step (a $<key> sigil in a \
         shell body, or probes = {{...}} on cook.add_unit), declare it in the probe's \
         `inputs.requires` (when reading from a probe produce body), or seal it \
         (seal {{ \"{key}\" }}) so it is scheduled before this step runs."
    )
}
