//! Resolve a ProbeUnit's declared inputs into ProbeFingerprintInputs by
//! consulting the current env, PATH, filesystem, and upstream probe map.

use std::collections::BTreeMap;
use std::path::Path;

use cook_contracts::ProbeUnit;
use sha2::{Digest, Sha256};

use cook_contracts::context::ProbeFingerprintInputs;

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
        .map(|path| (path.clone(), hash_file_sha256(&working_dir.join(path))))
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
/// the same per-run memo as the fingerprint fold ([`crate::statmemo`]), so a
/// module calling this never re-hashes a binary the fingerprint pass already
/// read — and, since COOK-414, never sees a binary cook rebuilt mid-run at its
/// pre-build bytes either.
pub fn tool_identity(name: &str) -> Option<(String, String)> {
    let path = which::which(name).ok()?;
    let hash = crate::statmemo::tool_hash_memo(&path);
    Some((
        cook_contracts::render::lower_hex(&hash),
        path.to_string_lossy().into_owned(),
    ))
}

fn resolve_tool_hash(name: &str) -> [u8; 32] {
    let Ok(path) = which::which(name) else {
        return [0u8; 32];
    };
    crate::statmemo::tool_hash_memo(&path)
}

/// SHA-256 of a file's bytes, or all-zero when it cannot be read.
///
/// Public because CS-0204 hashes module source with it: the probe fingerprint
/// folds every other file through the same function, and a second hasher over
/// the same question is how two halves of one key come to disagree.
///
/// COOK-414: cook-cache has two file hashes and they answer different
/// questions. This one is IDENTITY THAT LEAVES THE MACHINE — the §22.5.3 probe
/// fingerprint, the CS-0204 module-source fold, the cloud key underneath both.
/// [`crate::check::hash_file`] is the other: xxh3 local content identity for
/// `FileRecord` and the local cache key. The algorithm is part of each name
/// because this function was once ALSO called `hash_file`, privately, in this
/// module, which meant the crate's most-used verb named two different hashes
/// depending on which file you were reading.
pub fn hash_file_sha256(path: &Path) -> [u8; 32] {
    let Ok(bytes) = std::fs::read(path) else {
        return [0u8; 32];
    };
    Sha256::digest(&bytes).into()
}

#[cfg(test)]
#[path = "tests/probe_tests.rs"]
mod tests;
