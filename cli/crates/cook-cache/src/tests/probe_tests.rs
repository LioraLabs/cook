use super::*;
use std::path::PathBuf;

#[test]
fn resolve_probe_inputs_with_no_inputs_succeeds() {
    let probe = ProbeUnit {
        key: "cc:x".into(),
        produce_source: "return 1".into(),
        produce_line: 1,
        inputs: cook_contracts::ProbeInputs::default(),
    };
    let r = resolve_probe_inputs(&probe, &PathBuf::from("."), &|_| None, &BTreeMap::new());
    assert!(r.is_ok());
}

#[test]
fn missing_upstream_fingerprint_errors() {
    let mut probe = ProbeUnit {
        key: "cc:x".into(),
        produce_source: "return 1".into(),
        produce_line: 1,
        inputs: cook_contracts::ProbeInputs::default(),
    };
    probe.inputs.requires = vec!["cc:missing".into()];
    let r = resolve_probe_inputs(&probe, &PathBuf::from("."), &|_| None, &BTreeMap::new());
    let err = r.unwrap_err();
    assert!(err.contains("cc:missing"), "got: {}", err);
    assert!(err.contains("cc:x"), "got: {}", err);
}

#[test]
fn env_lookup_propagates_to_fingerprint_inputs() {
    let mut probe = ProbeUnit {
        key: "k".into(),
        produce_source: "".into(),
        produce_line: 1,
        inputs: cook_contracts::ProbeInputs::default(),
    };
    probe.inputs.env = vec!["MY_VAR".into()];
    let lookup = |name: &str| match name {
        "MY_VAR" => Some("value".into()),
        _ => None,
    };
    let r =
        resolve_probe_inputs(&probe, &PathBuf::from("."), &lookup, &BTreeMap::new()).unwrap();
    assert_eq!(r.env, vec![("MY_VAR".into(), Some("value".into()))]);
}

/// COOK-414: a golden vector for the PROBE-side file hash, computed outside
/// this codebase (`sha256sum`) rather than read off a passing run. The
/// preimage, so it stays checkable without cook:
///
/// ```text
/// sha256(b"cook COOK-414 golden vector\n")   // 28 bytes
///   == efc9dc719da43ebcc60b500cb35f4809aa342940e5c73b447f2d194ad974a2e2
/// ```
///
/// This is the other of cook-cache's two file hashes, and the two answer
/// different questions: `check::hash_file` is xxh3 local content identity,
/// this is the SHA-256 identity that §22.5.3 folds into a probe fingerprint
/// and CS-0204 folds over module source. Both feed keys that cross machines,
/// so both are pinned to a number rather than to their own past behaviour.
#[test]
fn the_sha256_file_hash_is_this_exact_digest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("golden");
    std::fs::write(&path, b"cook COOK-414 golden vector\n").unwrap();
    assert_eq!(
        cook_contracts::render::lower_hex(&hash_file_sha256(&path)),
        "efc9dc719da43ebcc60b500cb35f4809aa342940e5c73b447f2d194ad974a2e2",
    );
}

/// An unreadable path contributes the all-zero digest rather than an error, and
/// the §22.5.3 fold depends on that: a file that VANISHES must compose a
/// different fingerprint from the one it composed while it existed, not drop
/// out of the fold.
#[test]
fn an_unreadable_path_hashes_to_all_zero() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(hash_file_sha256(&dir.path().join("nope")), [0u8; 32]);
}

#[test]
fn missing_env_value_becomes_none() {
    let mut probe = ProbeUnit {
        key: "k".into(),
        produce_source: "".into(),
        produce_line: 1,
        inputs: cook_contracts::ProbeInputs::default(),
    };
    probe.inputs.env = vec!["UNSET_VAR".into()];
    let r =
        resolve_probe_inputs(&probe, &PathBuf::from("."), &|_| None, &BTreeMap::new()).unwrap();
    assert_eq!(r.env, vec![("UNSET_VAR".into(), None)]);
}
