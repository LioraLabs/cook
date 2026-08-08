//! Probe fingerprint computation (CS-0074 §22.5.3).
//!
//! The engine has no machine-identity, tool, or environment concept of its
//! own (Cache-trust v3 §1): every ambient determinant a unit depends on is
//! author-declared as a probe. This module hashes a probe unit's declared
//! inputs into its §22.5.3 fingerprint; nothing here infers host or tool
//! identity.

use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Probe fingerprint (CS-0074 §22.5.3)
// ---------------------------------------------------------------------------

/// Inputs to a probe-unit's fingerprint (§22.5.3).
#[derive(Debug, Clone)]
pub struct ProbeFingerprintInputs {
    pub key: String,
    pub produce_source: String,
    /// (env-var name, current value or None if unset).
    pub env: Vec<(String, Option<String>)>,
    /// (tool name, 32-byte content hash of resolved binary, all-zero if missing).
    pub tools: Vec<(String, [u8; 32])>,
    /// (file path, 32-byte content hash, all-zero if missing).
    pub files: Vec<(String, [u8; 32])>,
    /// (upstream probe key, that probe's fingerprint).
    pub upstream_probes: Vec<(String, [u8; 32])>,
}

/// Compute the 32-byte SHA-256 fingerprint per §22.5.3.
pub fn compute_probe_fingerprint(inputs: &ProbeFingerprintInputs) -> [u8; 32] {
    let mut h = Sha256::new();

    // §22.5.3 section 1: literal marker. V1 → V2 by CS-0102 (probe values
    // re-encoded to canonical JSON): bumping the marker makes every
    // pre-CS-0102 artifact an unreachable cache key.
    h.update(b"COOK_PROBE_FP_V2\n");
    // §22.5.3 section 2: key
    h.update(inputs.key.as_bytes());
    h.update(b"\n");
    // §22.5.3 section 3: produce source string (UTF-8 bytes)
    h.update(inputs.produce_source.as_bytes());
    h.update(b"\n");

    // §22.5.3 section 4: ENV
    let mut env = inputs.env.clone();
    env.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"ENV\n");
    for (k, v) in &env {
        h.update(k.as_bytes());
        h.update(b"=");
        match v {
            Some(s) => h.update(s.as_bytes()),
            None => h.update(b"<unset>"),
        }
        h.update(b"\n");
    }

    // §22.5.3 section 5: TOOLS
    let mut tools = inputs.tools.clone();
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"TOOLS\n");
    for (name, hash) in &tools {
        h.update(name.as_bytes());
        h.update(b"=");
        h.update(crate::render::lower_hex(hash).as_bytes());
        h.update(b"\n");
    }

    // §22.5.3 section 6: FILES
    let mut files = inputs.files.clone();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"FILES\n");
    for (path, hash) in &files {
        h.update(path.as_bytes());
        h.update(b"=");
        h.update(crate::render::lower_hex(hash).as_bytes());
        h.update(b"\n");
    }

    // §22.5.3 section 7: UPSTREAM
    let mut up = inputs.upstream_probes.clone();
    up.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"UPSTREAM\n");
    for (key, fp) in &up {
        h.update(key.as_bytes());
        h.update(b"=");
        h.update(crate::render::lower_hex(fp).as_bytes());
        h.update(b"\n");
    }

    let result = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

// ---------------------------------------------------------------------------
// Module-source folding (§22.5.3.1, CS-0204)
// ---------------------------------------------------------------------------

/// The §22.5.3 fingerprint folds seven declared sections and no module source,
/// so a probe whose `produce` body loads a module was addressable at the same
/// fingerprint after the module changed, and served its old value.
///
/// Module source cannot join the declared sections, because which modules a
/// `produce` body loads is a fact about the RUN. The fold is therefore a
/// second stage over the first: the declared fingerprint identifies the probe,
/// and this identifies the probe *together with the code it ran*.
///
/// # Why an empty set is the identity
///
/// A probe that loads no module MUST fingerprint exactly as it did before
/// CS-0204 — otherwise every probe in every existing store is orphaned by a
/// change that concerns none of them, and the reference implementation would
/// be paying a cold `cc` discovery pass for a rule it does not exercise. So an
/// empty set returns `declared` unchanged rather than hashing "nothing".
///
/// `modules` is `(path, content-hash)` pairs; order does not matter, they are
/// sorted here. A path that could not be read contributes its all-zero hash
/// rather than being dropped, so a module that VANISHES composes a different
/// fingerprint and misses, instead of composing the fingerprint it had while
/// it existed.
pub fn fold_module_sources(declared: &[u8; 32], modules: &[(String, [u8; 32])]) -> [u8; 32] {
    if modules.is_empty() {
        return *declared;
    }
    let mut sorted = modules.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    h.update(b"COOK_PROBE_FP_MODULES_V1\n");
    h.update(declared);
    h.update(b"\nMODULES\n");
    for (path, hash) in &sorted {
        h.update(path.as_bytes());
        h.update(b"=");
        h.update(crate::render::lower_hex(hash).as_bytes());
        h.update(b"\n");
    }
    let result = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Where the module-path manifest for a probe lives: a key derived from the
/// DECLARED fingerprint.
///
/// A cold reader knows the declared fingerprint and nothing else — it has not
/// run the produce body, so it cannot know which modules the body would load.
/// The manifest is the only bridge: read the recorded path sets from here,
/// re-hash them against the local tree, and probe the composed full
/// fingerprint. A recorded set that no longer describes this machine composes
/// a fingerprint nothing is stored under, which is a safe MISS. It can never
/// be a wrong hit, because every listed path's CONTENT is part of the key it
/// composes.
pub fn probe_module_manifest_key(declared: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"COOK_PROBE_MODULE_MANIFEST_V1\n");
    h.update(declared);
    let result = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// How many distinct module-path sets one manifest remembers.
///
/// More than one because a set is not stable: a branch inside a module, a
/// platform-conditional `require`, or a revert can each produce a different
/// set for the same declaration, and remembering only the newest would make a
/// revert a permanent cold miss (the defect COOK-278 fixed for depfiles).
/// Capped because the list is re-read and re-hashed on every cold lookup.
pub const MODULE_SET_CAP: usize = 8;

/// Merge an observed path set into a manifest, newest first, deduplicated,
/// capped at [`MODULE_SET_CAP`].
///
/// Pure and total so both stores that keep such a manifest — the probe store
/// and the step store — merge it the same way rather than each rolling the
/// obvious three lines slightly differently.
pub fn merge_path_set(existing: &[Vec<String>], observed: &[String]) -> Vec<Vec<String>> {
    let observed = observed.to_vec();
    let mut out = Vec::with_capacity(existing.len() + 1);
    out.push(observed.clone());
    for set in existing {
        if *set != observed {
            out.push(set.clone());
        }
    }
    out.truncate(MODULE_SET_CAP);
    out
}

#[cfg(test)]
#[path = "tests/context_tests.rs"]
mod tests;
