//! Fingerprint and cache-key computation for the Cook build system.
//!
//! This crate is the "what changed?" surface: pure functions that compute
//! content hashes, env contributions, machine/tool identity, and the
//! SHA-256 cache keys that address artifacts in any backend. It also defines
//! the `CacheBackend` trait — the seam the persistence layer (filesystem,
//! Cook Cloud, etc.) implements.
//!
//! `cook-cache` provides the v3 filesystem backend and the recipe-cache
//! manager built on top of these primitives.

pub mod backend;
pub mod check;
pub mod context;
pub mod envkey;
pub mod probe;
pub mod record;

use std::collections::BTreeSet;
use std::path::Path;

use sha2::{Digest, Sha256};

pub use backend::{
    artifact_key, cloud_key, ArtifactMeta, BackendError, BackendResult, CacheBackend, CloudKey,
    CloudKeyInputs,
};
pub use check::{
    hash_env, hash_file, install_depfile_parser, needs_rebuild_cook, needs_rebuild_plate,
    stat_mtime, RebuildReason, RebuildResult, RestoreCtx,
};
pub use context::{
    compute_probe_fingerprint, ExecutionContext, MachineIdentity, ProbeFingerprintInputs, ToolHash,
};
pub use probe::resolve_probe_inputs;
pub use envkey::{env_contribution, EnvDenylist};
pub use record::{FileRecord, StepEntry, CACHE_VERSION};

/// Hash a string (for command templates, env vars, etc.)
pub fn hash_str(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

// ---------------------------------------------------------------------------
// Test-unit fingerprint (CS-0061 §3.3)
// ---------------------------------------------------------------------------

/// Environmental and file-system inputs that contribute to a test unit's
/// content-addressed fingerprint. Matches the analogous inputs used for
/// recipe-step fingerprints but is kept separate so the test cache can
/// evolve independently.
///
/// All four `Vec` fields are sorted before hashing, so insertion order is
/// irrelevant — callers should not pre-sort them.
#[derive(Debug, Default, Clone)]
pub struct FingerprintInputs {
    /// `(path, content_fingerprint)` for cook-step outputs consumed by the test.
    pub cook_outputs: Vec<(String, String)>,
    /// `(path, content_fingerprint)` for dep-step outputs consumed by the test.
    pub dep_outputs: Vec<(String, String)>,
    /// `(key, value)` for env-var contributions.
    pub env_keys: Vec<(String, String)>,
    /// `(tool_name, hash)` for tool-version contributions.
    pub tool_hashes: Vec<(String, String)>,
}

/// Hash a sorted list of `(key, value)` pairs into `h`.
fn hash_pairs(h: &mut Sha256, v: &[(String, String)]) {
    let mut s: Vec<&(String, String)> = v.iter().collect();
    s.sort();
    for (k, val) in s {
        h.update(k.as_bytes());
        h.update(b"=");
        h.update(val.as_bytes());
        h.update(b"\0");
    }
}

/// Compute a content-addressed fingerprint for a test unit per CS-0061 §3.3.
///
/// Inputs (hashed in this stable order):
///   1. `cmd` — the substituted command text
///   2. `timeout` — big-endian u64 bytes
///   3. `should_fail` — 0x00 (false) or 0x01 (true)
///   4. `cook_outputs` — sorted by `(path, fingerprint)`
///   5. `dep_outputs`  — sorted by `(path, fingerprint)`
///   6. `env_keys`     — sorted by `(key, value)`
///   7. `tool_hashes`  — sorted by `(name, hash)`
///
/// **Excluded:** `suite_name`, `test_name` — these are display metadata.
/// Renaming a test via `as STRING` MUST NOT bust its fingerprint (§3.3).
///
/// # Panics
/// Panics if `payload` is not `WorkPayload::Test { .. }`. This function is
/// intentionally test-only; callers must route non-Test payloads elsewhere.
pub fn compute_test_fingerprint(
    payload: &cook_contracts::WorkPayload,
    inputs: &FingerprintInputs,
) -> String {
    let (cmd, timeout, should_fail) = match payload {
        cook_contracts::WorkPayload::Test {
            cmd,
            timeout,
            should_fail,
            ..
        } => (cmd.as_str(), *timeout, *should_fail),
        _ => panic!("compute_test_fingerprint: not a Test payload"),
    };

    let mut h = Sha256::new();

    // 1. cmd
    h.update(cmd.as_bytes());
    h.update(b"\0");

    // 2. timeout (big-endian u64)
    h.update(timeout.to_be_bytes());
    h.update(b"\0");

    // 3. should_fail (0 or 1)
    h.update([if should_fail { 1u8 } else { 0u8 }]);
    h.update(b"\0");

    // 4-7. sorted pair lists
    hash_pairs(&mut h, &inputs.cook_outputs);
    hash_pairs(&mut h, &inputs.dep_outputs);
    hash_pairs(&mut h, &inputs.env_keys);
    hash_pairs(&mut h, &inputs.tool_hashes);

    format!("sha256:{:x}", h.finalize())
}

/// Returns true if the string contains any glob metacharacter recognised by
/// the reference implementation's `glob = "0.3"` matcher: `*`, `?`, `[`.
///
/// CS-0085 specifies these three characters as the glob metacharacter set.
/// `{` is intentionally excluded — `glob` 0.3 does not support brace
/// alternation, so a string like "out/{a,b}.txt" is treated as a literal
/// path.
pub fn has_glob_meta(s: &str) -> bool {
    s.bytes().any(|b| matches!(b, b'*' | b'?' | b'['))
}

/// Helper to resolve a glob pattern into a set of files.
///
/// Sub-directory matches are dropped (CS-0064): every consumer of this
/// helper feeds the results into cook's file-hashing path, where a
/// directory entry has no hashable bytes.
pub fn resolve_glob(root: &Path, pattern: &str) -> BTreeSet<String> {
    let full_pattern = root.join(pattern);
    let prefix = root.to_string_lossy().to_string();

    let paths = match glob::glob(&full_pattern.to_string_lossy()) {
        Ok(p) => p,
        Err(_) => return BTreeSet::new(),
    };

    paths
        .filter_map(Result::ok)
        .filter(|p| !matches!(std::fs::metadata(p), Ok(m) if m.is_dir()))
        .map(|p| {
            let path_str = p.to_string_lossy().to_string();
            path_str
                .strip_prefix(&prefix)
                .unwrap_or(&path_str)
                .trim_start_matches('/')
                .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cook_contracts::WorkPayload;

    #[test]
    fn test_hash_str_deterministic() {
        let h1 = hash_str("hello");
        let h2 = hash_str("hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_str_differs() {
        let h1 = hash_str("hello");
        let h2 = hash_str("world");
        assert_ne!(h1, h2);
    }

    fn empty_inputs() -> FingerprintInputs {
        FingerprintInputs::default()
    }

    fn make_test_payload(
        cmd: &str,
        timeout: u64,
        should_fail: bool,
        suite_name: &str,
        test_name: &str,
    ) -> WorkPayload {
        WorkPayload::Test {
            cmd: cmd.into(),
            line: 1,
            timeout,
            should_fail,
            suite_name: suite_name.into(),
            test_name: test_name.into(),
            iteration_item: None,
            input_paths: vec![],
        }
    }

    #[test]
    fn test_unit_fingerprint_includes_timeout() {
        let fp_30 = compute_test_fingerprint(
            &make_test_payload("true", 30, false, "r", "t"),
            &empty_inputs(),
        );
        let fp_60 = compute_test_fingerprint(
            &make_test_payload("true", 60, false, "r", "t"),
            &empty_inputs(),
        );
        assert_ne!(
            fp_30, fp_60,
            "different timeouts must produce different fingerprints"
        );
    }

    #[test]
    fn test_unit_fingerprint_includes_should_fail() {
        let fp_t = compute_test_fingerprint(
            &make_test_payload("true", 30, true, "r", "t"),
            &empty_inputs(),
        );
        let fp_f = compute_test_fingerprint(
            &make_test_payload("true", 30, false, "r", "t"),
            &empty_inputs(),
        );
        assert_ne!(fp_t, fp_f);
    }

    #[test]
    fn test_unit_fingerprint_independent_of_test_name() {
        // Renaming via `as` (the test_name) MUST NOT bust fingerprint per CS-0061 §3.3.
        let fp_a = compute_test_fingerprint(
            &make_test_payload("true", 30, false, "r", "alpha"),
            &empty_inputs(),
        );
        let fp_b = compute_test_fingerprint(
            &make_test_payload("true", 30, false, "r", "beta"),
            &empty_inputs(),
        );
        assert_eq!(fp_a, fp_b, "renaming a test MUST NOT bust its fingerprint");
    }

    #[test]
    fn test_unit_fingerprint_independent_of_suite_name() {
        let fp_a = compute_test_fingerprint(
            &make_test_payload("true", 30, false, "recipe_a", "t"),
            &empty_inputs(),
        );
        let fp_b = compute_test_fingerprint(
            &make_test_payload("true", 30, false, "recipe_b", "t"),
            &empty_inputs(),
        );
        assert_eq!(fp_a, fp_b);
    }

    #[test]
    fn test_unit_fingerprint_deterministic() {
        let payload = make_test_payload("run_tests.sh", 120, false, "suite", "test1");
        let inputs = FingerprintInputs {
            cook_outputs: vec![("out/lib.a".into(), "sha256:abc".into())],
            dep_outputs: vec![],
            env_keys: vec![("CC".into(), "gcc".into())],
            tool_hashes: vec![("bash".into(), "sha256:def".into())],
        };
        let fp1 = compute_test_fingerprint(&payload, &inputs);
        let fp2 = compute_test_fingerprint(&payload, &inputs);
        assert_eq!(fp1, fp2);
        assert!(fp1.starts_with("sha256:"));
    }

    #[test]
    fn test_unit_fingerprint_includes_cmd() {
        let fp_a = compute_test_fingerprint(
            &make_test_payload("cmd_a", 30, false, "r", "t"),
            &empty_inputs(),
        );
        let fp_b = compute_test_fingerprint(
            &make_test_payload("cmd_b", 30, false, "r", "t"),
            &empty_inputs(),
        );
        assert_ne!(fp_a, fp_b, "different commands must produce different fingerprints");
    }

    #[test]
    fn glob_meta_literal_paths_return_false() {
        assert!(!has_glob_meta(""));
        assert!(!has_glob_meta("main.c"));
        assert!(!has_glob_meta("build/main.o"));
        assert!(!has_glob_meta("apps/web/.next/BUILD_ID"));
        assert!(!has_glob_meta("a/b/c/d.txt"));
    }

    #[test]
    fn glob_meta_star_returns_true() {
        assert!(has_glob_meta("*"));
        assert!(has_glob_meta("*.c"));
        assert!(has_glob_meta("src/**"));
        assert!(has_glob_meta("src/**/*"));
        assert!(has_glob_meta("apps/web/.next/**"));
    }

    #[test]
    fn glob_meta_question_returns_true() {
        assert!(has_glob_meta("?"));
        assert!(has_glob_meta("file?.txt"));
    }

    #[test]
    fn glob_meta_bracket_returns_true() {
        assert!(has_glob_meta("[abc].txt"));
        assert!(has_glob_meta("src/[ab]/main.c"));
    }

    #[test]
    fn glob_meta_brace_returns_false() {
        // The reference engine's `glob = "0.3"` crate does NOT support
        // brace alternation; `{` is treated as a literal. Per CS-0085
        // the spec excludes `{` from the metacharacter set so that a
        // string like "out/{a,b}.txt" is treated as a LITERAL PATH,
        // not as a glob pattern. Brace expansion may be added in a
        // future CS once the reference engine supports it.
        assert!(!has_glob_meta("{a,b}.txt"));
        assert!(!has_glob_meta("src/{lib,app}/main.c"));
    }

    #[test]
    fn test_unit_fingerprint_cook_outputs_order_independent() {
        let inputs_a = FingerprintInputs {
            cook_outputs: vec![
                ("a".into(), "hash1".into()),
                ("b".into(), "hash2".into()),
            ],
            ..Default::default()
        };
        let inputs_b = FingerprintInputs {
            cook_outputs: vec![
                ("b".into(), "hash2".into()),
                ("a".into(), "hash1".into()),
            ],
            ..Default::default()
        };
        let payload = make_test_payload("true", 30, false, "r", "t");
        assert_eq!(
            compute_test_fingerprint(&payload, &inputs_a),
            compute_test_fingerprint(&payload, &inputs_b),
            "cook_outputs insertion order must not affect fingerprint"
        );
    }
}
