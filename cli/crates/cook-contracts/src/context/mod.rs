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

#[cfg(test)]
#[path = "tests/context_tests.rs"]
mod tests;
