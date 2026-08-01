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
