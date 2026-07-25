//! `LocalBackend::enumerate()` — the CAS walk that `cook cache du` (COOK-232
//! Task 3) and `cook cache gc` (COOK-234) both consume. Proves the walk sees
//! exactly what's on disk under `{root}/{2hex}/{62hex}` and nothing else:
//! sidecars, `.tmp` scratch files, and junk-named files are all excluded.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use cook_cache::backend::{cloud_key, put_bytes, ArtifactMeta, CloudKeyInputs, LocalBackend};
use cook_cache::store::CACHE_VERSION;

const RECIPE_NS: &str = "proj/Cookfile::build";

fn make_meta(recipe_namespace: &str, output_path: &str, size_bytes: u64) -> ArtifactMeta {
    ArtifactMeta {
        recipe_namespace: recipe_namespace.into(),
        command_hash: 0xbeef,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: CACHE_VERSION,
        size_bytes,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: output_path.into(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: None,
        mode: ArtifactMeta::default_mode(),
        target: None,
    }
}

fn some_key(seed: u64) -> cook_cache::backend::CloudKey {
    cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: RECIPE_NS,
        command_hash: seed,
        env_contribution: 0,
        seal_contribution: 0,
        sorted_input_content_hashes: &[],
    })
}

#[test]
fn empty_dir_yields_zero_candidates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let candidates = backend.enumerate().expect("enumerate");
    assert_eq!(candidates, vec![]);
}

#[test]
fn nonexistent_root_yields_zero_candidates_not_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing_root = dir.path().join("does-not-exist");
    // LocalBackend::new/with_config creates the root via create_dir_all;
    // remove it again immediately so `enumerate` sees a missing root and
    // we exercise the "root doesn't exist" branch directly.
    let backend = LocalBackend::new(missing_root.clone());
    std::fs::remove_dir_all(&missing_root).expect("remove freshly-created root");
    let candidates = backend.enumerate().expect("enumerate");
    assert_eq!(candidates, vec![]);
}

#[test]
fn put_blobs_are_all_returned_with_correct_size_and_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());

    let key_a = some_key(1);
    let key_b = some_key(2);
    let bytes_a = b"hello world";
    let bytes_b = b"a much longer artifact body than the other one";

    let mut meta_a = make_meta(RECIPE_NS, "out_a.bin", bytes_a.len() as u64);
    put_bytes(&backend, &key_a, bytes_a, &mut meta_a).expect("put a");

    let mut meta_b = make_meta(RECIPE_NS, "out_b.bin", bytes_b.len() as u64);
    put_bytes(&backend, &key_b, bytes_b, &mut meta_b).expect("put b");

    let mut candidates = backend.enumerate().expect("enumerate");
    candidates.sort_by_key(|c| c.key);

    let mut expected_keys = [key_a, key_b];
    expected_keys.sort();

    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].key, expected_keys[0]);
    assert_eq!(candidates[1].key, expected_keys[1]);

    let sizes: BTreeSet<u64> = candidates.iter().map(|c| c.size).collect();
    let expected_sizes: BTreeSet<u64> = [bytes_a.len() as u64, bytes_b.len() as u64]
        .into_iter()
        .collect();
    assert_eq!(sizes, expected_sizes);
}

#[test]
fn kind_and_recipe_namespace_round_trip_from_sidecar() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());

    let key = some_key(3);
    let bytes = b"probe-body";
    let mut meta = make_meta(RECIPE_NS, "probe.out", bytes.len() as u64);
    meta.kind = Some("probe_value".into());
    put_bytes(&backend, &key, bytes, &mut meta).expect("put");

    let candidates = backend.enumerate().expect("enumerate");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].kind, Some("probe_value".into()));
    assert_eq!(candidates[0].recipe_namespace, RECIPE_NS);
}

#[test]
fn junk_and_sidecar_files_in_a_shard_dir_are_excluded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());

    let key = some_key(4);
    let bytes = b"real-blob";
    let mut meta = make_meta(RECIPE_NS, "real.out", bytes.len() as u64);
    put_bytes(&backend, &key, bytes, &mut meta).expect("put");

    // Drop junk into the same shard directory as the real blob.
    let hex = hex::encode(key);
    let shard_dir = dir.path().join(&hex[..2]);
    std::fs::write(shard_dir.join("not-hex-junk.dat"), b"junk").expect("write junk");
    std::fs::write(shard_dir.join(format!("{}.tmp", &hex[2..])), b"scratch").expect("write tmp");
    // .meta.json and .provenance.json already exist from `put`; write a
    // provenance sidecar explicitly to be sure it's covered too.
    std::fs::write(
        shard_dir.join(format!("{}.provenance.json", &hex[2..])),
        b"{}",
    )
    .expect("write provenance");

    let candidates = backend.enumerate().expect("enumerate");
    assert_eq!(
        candidates.len(),
        1,
        "only the real 62-hex-char blob should be counted, got {candidates:?}"
    );
    assert_eq!(candidates[0].key, key);
}

#[test]
fn blob_with_deleted_sidecar_still_appears_as_orphan() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());

    let key = some_key(5);
    let bytes = b"orphan-blob";
    let mut meta = make_meta(RECIPE_NS, "orphan.out", bytes.len() as u64);
    put_bytes(&backend, &key, bytes, &mut meta).expect("put");

    let hex = hex::encode(key);
    let shard_dir = dir.path().join(&hex[..2]);
    std::fs::remove_file(shard_dir.join(format!("{}.meta.json", &hex[2..])))
        .expect("delete sidecar");

    let candidates = backend.enumerate().expect("enumerate");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].key, key);
    assert_eq!(candidates[0].kind, None);
    assert_eq!(candidates[0].recipe_namespace, "");
}

#[test]
fn blob_with_malformed_sidecar_still_appears_as_orphan() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());

    let key = some_key(8);
    let bytes = b"malformed-sidecar-blob";
    let mut meta = make_meta(RECIPE_NS, "malformed.out", bytes.len() as u64);
    put_bytes(&backend, &key, bytes, &mut meta).expect("put");

    let hex = hex::encode(key);
    let shard_dir = dir.path().join(&hex[..2]);
    // Overwrite the sidecar `put` just wrote with garbage — not a missing
    // file, a present-but-unparseable one.
    std::fs::write(
        shard_dir.join(format!("{}.meta.json", &hex[2..])),
        b"not valid json {{{",
    )
    .expect("overwrite sidecar with invalid json");

    let candidates = backend.enumerate().expect("enumerate");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].key, key);
    assert_eq!(candidates[0].kind, None);
    assert_eq!(candidates[0].recipe_namespace, "");
}

#[test]
fn shard_dir_name_filtering_and_blob_case_sensitivity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());

    // Genuine blob in a genuine shard dir — the one thing that should
    // survive all the junk planted below.
    let key = some_key(9);
    let bytes = b"legit-blob";
    let mut meta = make_meta(RECIPE_NS, "legit.out", bytes.len() as u64);
    put_bytes(&backend, &key, bytes, &mut meta).expect("put");

    let hex = hex::encode(key);
    let shard_dir = dir.path().join(&hex[..2]);

    // Same (legitimate) shard dir, but a 62-char *uppercase* hex name:
    // right length, hex characters, but not lowercase — must be excluded.
    std::fs::write(shard_dir.join("A".repeat(62)), b"uppercase-blob")
        .expect("write uppercase-named blob");

    // Store-root directories whose names are not exactly 2 lowercase hex
    // chars: uppercase (wrong case), "zz" (right length, not hex chars),
    // and "deadbeef" (valid hex chars, wrong length). Each holds a
    // plausible 62-hex-named file that would pass the blob-name filter on
    // its own, to prove it's the shard-name check doing the excluding.
    let plausible_blob_name = "a".repeat(62);
    for bogus_shard in ["AB", "zz", "deadbeef"] {
        let bogus_dir = dir.path().join(bogus_shard);
        std::fs::create_dir_all(&bogus_dir).expect("mkdir bogus shard");
        std::fs::write(bogus_dir.join(&plausible_blob_name), b"bogus-shard-blob")
            .expect("write into bogus shard dir");
    }

    let candidates = backend.enumerate().expect("enumerate");
    assert_eq!(
        candidates.len(),
        1,
        "only the genuine lowercase blob in the genuine shard dir should be counted, got {candidates:?}"
    );
    assert_eq!(candidates[0].key, key);
}

#[test]
fn size_reflects_real_file_length_not_untrusted_meta_size_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());

    let key = some_key(6);
    let bytes = b"twelve bytes"; // 12 bytes
    assert_eq!(bytes.len(), 12);
    let mut meta = make_meta(RECIPE_NS, "lie.out", 999_999); // deliberately wrong
    put_bytes(&backend, &key, bytes, &mut meta).expect("put");

    let candidates = backend.enumerate().expect("enumerate");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].size, 12, "size must come from fs::metadata, not caller-set size_bytes");
}

#[test]
fn last_access_is_recent_for_a_freshly_written_blob() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());

    let key = some_key(7);
    let bytes = b"fresh";
    let mut meta = make_meta(RECIPE_NS, "fresh.out", bytes.len() as u64);
    put_bytes(&backend, &key, bytes, &mut meta).expect("put");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("now")
        .as_secs();

    let candidates = backend.enumerate().expect("enumerate");
    assert_eq!(candidates.len(), 1);
    let last_access = candidates[0].last_access;
    assert!(
        last_access <= now && now.saturating_sub(last_access) < 10,
        "last_access {last_access} should be within a few seconds of now {now}"
    );
}
