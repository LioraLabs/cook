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
        h.update(probe_hex_encode(hash).as_bytes());
        h.update(b"\n");
    }

    // §22.5.3 section 6: FILES
    let mut files = inputs.files.clone();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"FILES\n");
    for (path, hash) in &files {
        h.update(path.as_bytes());
        h.update(b"=");
        h.update(probe_hex_encode(hash).as_bytes());
        h.update(b"\n");
    }

    // §22.5.3 section 7: UPSTREAM
    let mut up = inputs.upstream_probes.clone();
    up.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"UPSTREAM\n");
    for (key, fp) in &up {
        h.update(key.as_bytes());
        h.update(b"=");
        h.update(probe_hex_encode(fp).as_bytes());
        h.update(b"\n");
    }

    let result = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

fn probe_hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_fingerprint_is_deterministic_for_same_inputs() {
        let inputs = ProbeFingerprintInputs {
            key: "cc:zlib".into(),
            produce_source: "return run_pkg_config(\"zlib\")".into(),
            env: vec![("CC".into(), Some("gcc".into())), ("PATH".into(), Some("/usr/bin".into()))],
            tools: vec![("pkg-config".into(), [0u8; 32])],
            files: vec![],
            upstream_probes: vec![],
        };
        assert_eq!(compute_probe_fingerprint(&inputs), compute_probe_fingerprint(&inputs));
    }

    #[test]
    fn probe_fingerprint_changes_when_env_value_changes() {
        let mut a = ProbeFingerprintInputs {
            key: "cc:zlib".into(),
            produce_source: "".into(),
            env: vec![("PKG_CONFIG_PATH".into(), Some("/a".into()))],
            tools: vec![], files: vec![], upstream_probes: vec![],
        };
        let h1 = compute_probe_fingerprint(&a);
        a.env[0].1 = Some("/b".into());
        assert_ne!(h1, compute_probe_fingerprint(&a));
    }

    #[test]
    fn probe_fingerprint_is_invariant_to_input_order() {
        let a = ProbeFingerprintInputs {
            key: "cc:x".into(), produce_source: "".into(),
            env: vec![("A".into(), Some("1".into())), ("B".into(), Some("2".into()))],
            tools: vec![], files: vec![], upstream_probes: vec![],
        };
        let b = ProbeFingerprintInputs {
            key: "cc:x".into(), produce_source: "".into(),
            env: vec![("B".into(), Some("2".into())), ("A".into(), Some("1".into()))],
            tools: vec![], files: vec![], upstream_probes: vec![],
        };
        assert_eq!(compute_probe_fingerprint(&a), compute_probe_fingerprint(&b));
    }

    #[test]
    fn probe_fingerprint_changes_on_upstream_probe_change() {
        let mut a = ProbeFingerprintInputs {
            key: "cc:x".into(), produce_source: "".into(),
            env: vec![], tools: vec![], files: vec![],
            upstream_probes: vec![("cc:compiler".into(), [1u8; 32])],
        };
        let h1 = compute_probe_fingerprint(&a);
        a.upstream_probes[0].1 = [2u8; 32];
        assert_ne!(h1, compute_probe_fingerprint(&a));
    }

    /// CS-0102 marker bump: the fingerprint preimage starts with
    /// `COOK_PROBE_FP_V2`, so every artifact addressed under the V1
    /// (pre-CS-0102) marker is unreachable.
    #[test]
    fn probe_fingerprint_marker_is_v2() {
        let inputs = ProbeFingerprintInputs {
            key: "k".into(),
            produce_source: "return 1".into(),
            env: vec![], tools: vec![], files: vec![], upstream_probes: vec![],
        };
        let fp = compute_probe_fingerprint(&inputs);

        let mut h = Sha256::new();
        h.update(b"COOK_PROBE_FP_V1\nk\nreturn 1\nENV\nTOOLS\nFILES\nUPSTREAM\n");
        let v1: [u8; 32] = h.finalize().into();

        assert_ne!(fp, v1, "probe fingerprint still uses the V1 marker");
    }

    #[test]
    fn probe_fingerprint_changes_when_produce_source_changes() {
        let mut a = ProbeFingerprintInputs {
            key: "k".into(), produce_source: "return 1".into(),
            env: vec![], tools: vec![], files: vec![], upstream_probes: vec![],
        };
        let h1 = compute_probe_fingerprint(&a);
        a.produce_source = "return 2".into();
        assert_ne!(h1, compute_probe_fingerprint(&a));
    }
}
