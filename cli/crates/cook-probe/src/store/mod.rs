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

/// Per-run probe-value store (§22.5.8). The canonical value of a probe is
/// the file [`materialize_value`] writes above (CS-0102); this store is a
/// read-through byte cache of that file, shared by the engine scheduler
/// and every worker's `cook.probes.get`. It is NOT a cross-VM shared-memory
/// channel: writers (engine scheduler, register pre-pass) write the file
/// first and seed this cache with the same bytes.
///
/// Locking: one mutex guards the whole map, held across the read-through
/// file load. Probe files are tiny; simplicity beats contention here, and
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

    /// Point the read-through at a `.cook/probes` directory. Called once
    /// per execute_dag run by the engine.
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
        let bytes =
            std::fs::read(dir.join(cook_contracts::probe::value::probe_file_name(key))).ok()?;
        inner.map.insert(key.to_string(), bytes.clone());
        Some(bytes)
    }
}

/// CS-0157: a probe value's READ VIEW — the canonical value with this run's
/// tool-path metadata merged in. The merge is
/// `cook_contracts::probe_value::merge_tool_paths`, applied to the JSON
/// value BEFORE any Lua conversion, so `cook.probes.get` and `$<key>`
/// substitution (CS-0192) build the same view from the same bytes and
/// cannot drift.
pub fn read_view(
    store: &ProbeValueStore,
    key: &str,
    bytes: &[u8],
) -> Result<serde_json::Value, String> {
    let mut value = cook_contracts::probe::value::decode_json(bytes)?;
    if let Some(paths) = store.tool_paths(key) {
        cook_contracts::probe::value::merge_tool_paths(&mut value, &paths);
    }
    Ok(value)
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
