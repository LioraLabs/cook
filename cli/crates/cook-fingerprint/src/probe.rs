//! Resolve a ProbeUnit's declared inputs into ProbeFingerprintInputs by
//! consulting the current env, PATH, filesystem, and upstream probe map.

use std::collections::BTreeMap;
use std::path::Path;

use cook_contracts::ProbeUnit;
use sha2::{Digest, Sha256};

use crate::context::ProbeFingerprintInputs;

/// Resolve a `ProbeUnit`'s declared inputs into `ProbeFingerprintInputs` by
/// walking env/PATH/filesystem/upstream-fp-map.
pub fn resolve_probe_inputs(
    probe: &ProbeUnit,
    working_dir: &Path,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    upstream_fingerprints: &BTreeMap<String, [u8; 32]>,
) -> Result<ProbeFingerprintInputs, String> {
    let env: Vec<(String, Option<String>)> = probe
        .inputs
        .env
        .iter()
        .map(|name| (name.clone(), env_lookup(name)))
        .collect();

    let tools: Vec<(String, [u8; 32])> = probe
        .inputs
        .tools
        .iter()
        .map(|name| (name.clone(), resolve_tool_hash(name)))
        .collect();

    let files: Vec<(String, [u8; 32])> = probe
        .inputs
        .files
        .iter()
        .map(|path| (path.clone(), hash_file(&working_dir.join(path))))
        .collect();

    let upstream_probes: Vec<(String, [u8; 32])> = probe
        .inputs
        .requires
        .iter()
        .map(|k| {
            let fp = upstream_fingerprints.get(k).copied().ok_or_else(|| {
                format!(
                    "probe '{}' requires upstream '{}' which has no fingerprint",
                    probe.key, k,
                )
            })?;
            Ok((k.clone(), fp))
        })
        .collect::<Result<_, String>>()?;

    Ok(ProbeFingerprintInputs {
        key: probe.key.clone(),
        produce_source: probe.produce_source.clone(),
        env,
        tools,
        files,
        upstream_probes,
    })
}

/// Resolve a tool name to its current PATH location, freshly, every call.
/// CS-0157: the resolved path is LOCATION metadata, not identity — it is
/// deliberately excluded from probe fingerprints and canonical probe values,
/// and deliberately NOT memoized here, so a consumer (the Lua read view,
/// `cook why` display) always sees where the tool resolves NOW rather than a
/// cached location that can go stale.
pub fn resolve_tool_path(name: &str) -> Option<String> {
    which::which(name).ok().map(|p| p.to_string_lossy().into_owned())
}

/// CS-0158: canonical tool identity for Lua consumers (`cook.tools.id`).
/// Resolves `name` on PATH and returns `(lowercase-hex sha256 of the binary,
/// resolved path)` — the hash is the machine-independent identity a module
/// folds into a sealed probe VALUE; the path is location metadata for
/// invocation. `None` when the name does not resolve. Hashing goes through
/// the same per-run memo as the fingerprint fold, so a module calling this
/// never re-hashes a binary the fingerprint pass already read.
pub fn tool_identity(name: &str) -> Option<(String, String)> {
    let path = which::which(name).ok()?;
    let hash = memoized_hash(&path);
    Some((
        crate::context::probe_hex_encode(&hash),
        path.to_string_lossy().into_owned(),
    ))
}

fn resolve_tool_hash(name: &str) -> [u8; 32] {
    let Ok(path) = which::which(name) else {
        return [0u8; 32];
    };
    memoized_hash(&path)
}

/// Per-run memo keyed by resolved path. The same tool is fingerprinted
/// once per probe NODE (five recipes sealing one `web:tools` probe hash
/// its binaries five times), and a binary like node is ~60MB — without
/// this, an all-cached workspace build spends seconds re-hashing the
/// same toolchain. One run = one process, so a process-wide memo cannot
/// go stale across builds.
fn memoized_hash(path: &std::path::Path) -> [u8; 32] {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MEMO: OnceLock<Mutex<HashMap<std::path::PathBuf, [u8; 32]>>> = OnceLock::new();
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(h) = memo.lock().unwrap().get(path) {
        return *h;
    }
    let h = hash_file(path);
    memo.lock().unwrap().insert(path.to_path_buf(), h);
    h
}

fn hash_file(path: &Path) -> [u8; 32] {
    let Ok(bytes) = std::fs::read(path) else {
        return [0u8; 32];
    };
    let result = Sha256::digest(&bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

#[cfg(test)]
#[path = "tests/probe_tests.rs"]
mod tests;
