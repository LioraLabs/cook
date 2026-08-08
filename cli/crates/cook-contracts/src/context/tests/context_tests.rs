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

// ---------------------------------------------------------------------------
// CS-0204: module-source folding
// ---------------------------------------------------------------------------

/// The identity that keeps every pre-CS-0204 probe addressable. A probe that
/// loads no module must fingerprint exactly as it did before this rule existed.
#[test]
fn folding_an_empty_module_set_is_the_identity() {
    let declared = [7u8; 32];
    assert_eq!(fold_module_sources(&declared, &[]), declared);
}

#[test]
fn folding_module_content_moves_the_fingerprint() {
    let declared = [7u8; 32];
    let a = fold_module_sources(&declared, &[("m.lua".into(), [1u8; 32])]);
    let b = fold_module_sources(&declared, &[("m.lua".into(), [2u8; 32])]);
    assert_ne!(a, declared);
    assert_ne!(a, b);
}

/// Order must not matter: the set is drained from a sink two doors write into,
/// and a fingerprint that moved with load order would be a false miss.
#[test]
fn folding_is_order_independent() {
    let declared = [7u8; 32];
    let one = [("a.lua".to_string(), [1u8; 32]), ("b.lua".to_string(), [2u8; 32])];
    let other = [("b.lua".to_string(), [2u8; 32]), ("a.lua".to_string(), [1u8; 32])];
    assert_eq!(fold_module_sources(&declared, &one), fold_module_sources(&declared, &other));
}

/// A path is part of the fold, not just its bytes: two modules swapping
/// contents must not compose the same fingerprint.
#[test]
fn folding_distinguishes_paths() {
    let declared = [7u8; 32];
    let one = [("a.lua".to_string(), [1u8; 32])];
    let other = [("b.lua".to_string(), [1u8; 32])];
    assert_ne!(fold_module_sources(&declared, &one), fold_module_sources(&declared, &other));
}

#[test]
fn manifest_key_is_derived_and_distinct_from_the_declared_fingerprint() {
    let declared = [7u8; 32];
    assert_ne!(probe_module_manifest_key(&declared), declared);
    assert_eq!(probe_module_manifest_key(&declared), probe_module_manifest_key(&declared));
}

