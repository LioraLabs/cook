use super::*;
use cook_contracts::LocalProbeKey;
use std::path::PathBuf;

fn probe(inputs: cook_contracts::ProbeInputs) -> ProbeUnit {
    ProbeUnit {
        key: LocalProbeKey::new("cc:x"),
        produce_source: "return 1".into(),
        produce_line: 1,
        inputs,
    }
}

#[test]
fn a_probe_declaring_nothing_resolves_to_empty_sets() {
    let values = resolve_probe_input_digests(
        &probe(cook_contracts::ProbeInputs::default()),
        &PathBuf::from("."),
    );
    assert!(values.tools.is_empty());
    assert!(values.files.is_empty());
}

/// CS-0244: `requires` is a scheduling edge, not a resolved input. A probe
/// naming an upstream resolves without consulting it — the ordering is carried
/// by the DAG (`unit_graph::plan`), never by a value looked up here.
#[test]
fn a_declared_requires_resolves_nothing_and_cannot_fail() {
    let values = resolve_probe_input_digests(
        &probe(cook_contracts::ProbeInputs {
            requires: vec!["cc:missing".into()],
            ..Default::default()
        }),
        &PathBuf::from("."),
    );
    assert!(values.tools.is_empty());
    assert!(values.files.is_empty());
}

/// A declared file that does not exist resolves to the all-zero digest rather
/// than being dropped: COOK-510's guard in `cook_probe::eval` is written
/// against exactly that digest.
#[test]
fn a_declared_file_resolves_to_its_content_digest() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("present"), b"alpha").unwrap();
    let values = resolve_probe_input_digests(
        &probe(cook_contracts::ProbeInputs {
            files: vec!["present".into(), "absent".into()],
            ..Default::default()
        }),
        dir.path(),
    );
    assert_eq!(
        values.files,
        vec![
            (
                "present".to_string(),
                hash_file_sha256(&dir.path().join("present"))
            ),
            ("absent".to_string(), [0u8; 32]),
        ],
    );
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
/// this is the SHA-256 identity a probe's declared `files`/`tools` sets fold
/// into its value. That value crosses machines through a sealed consumer's
/// key, so it is pinned to a number rather than to its own past behaviour.
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

#[test]
fn an_unreadable_path_hashes_to_all_zero() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(hash_file_sha256(&dir.path().join("nope")), [0u8; 32]);
}
