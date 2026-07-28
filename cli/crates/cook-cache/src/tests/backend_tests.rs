use super::*;
use std::collections::BTreeSet;

fn sample_meta() -> ArtifactMeta {
    ArtifactMeta {
        recipe_namespace: "cook/Cookfile::build".into(),
        command_hash: 0xdead_beef,
        env_contribution: 0x3333_4444,
        seal_contribution: 0,
        schema_version: 3,
        size_bytes: 5,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: "build/foo.o".into(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: None,
        mode: ArtifactMeta::default_mode(),
        target: None,
    }
}

fn key(byte: u8) -> CloudKey {
    let mut k = [0u8; 32];
    k[0] = byte;
    k
}

// ─── VerifyingReader unit tests ─────────────────────────────────────────

#[test]
fn verifying_reader_passes_through_on_match() {
    let bytes = b"verifying-reader payload";
    let expected: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
    let mut vr = VerifyingReader::new(Cursor::new(bytes.to_vec()), expected);
    let mut out = Vec::new();
    vr.read_to_end(&mut out).expect("read_to_end ok on match");
    assert_eq!(out, bytes);
}

#[test]
fn verifying_reader_errors_on_mismatch() {
    let bytes = b"verifying-reader payload";
    // Expected hash for *different* bytes — guarantees a mismatch
    // when we feed `bytes` through the reader.
    let bogus: [u8; 32] = <Sha256 as Digest>::digest(b"not the real bytes").into();
    let mut vr = VerifyingReader::new(Cursor::new(bytes.to_vec()), bogus);
    let mut out = Vec::new();
    let err = vr.read_to_end(&mut out).expect_err("expected mismatch error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn verifying_reader_passes_through_on_empty_match() {
        let bytes: &[u8] = b"";
    let expected: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
    let mut vr = VerifyingReader::new(Cursor::new(bytes.to_vec()), expected);
    let mut out = Vec::new();
    vr.read_to_end(&mut out).expect("empty match ok");
    assert!(out.is_empty());
}

// ─── LocalBackend basic tests ───────────────────────────────────────────

#[test]
fn local_backend_health_ok_on_existing_root() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        backend.health().expect("health ok");
}

#[test]
fn local_backend_get_miss_returns_none() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xAB);
        assert!(backend.get(&k).expect("get").is_none());
}

#[test]
fn local_backend_put_get_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x01);
        let mut meta = sample_meta();
        put_bytes(&backend, &k, b"hello", &mut meta).expect("put");
    let got = get_bytes(&backend, &k).expect("get").expect("hit");
    assert_eq!(got, b"hello");
}

#[test]
fn local_backend_put_idempotent() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x02);
        let mut meta1 = sample_meta();
        let mut meta2 = sample_meta();
        put_bytes(&backend, &k, b"data", &mut meta1).expect("put 1");
    put_bytes(&backend, &k, b"data", &mut meta2).expect("put 2");
    let got = get_bytes(&backend, &k).expect("get").expect("hit");
    assert_eq!(got, b"data");
}

#[test]
fn local_backend_batch_query_returns_hits_subset() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k1 = key(0x10);
        let k2 = key(0x20);
        let k3 = key(0x30);
        let mut m1 = sample_meta();
        let mut m3 = sample_meta();
        put_bytes(&backend, &k1, b"a", &mut m1).expect("put1");
    put_bytes(&backend, &k3, b"c", &mut m3).expect("put3");
    let hits = backend.batch_query(&[k1, k2, k3]).expect("query");
    assert!(hits.contains(&k1));
    assert!(!hits.contains(&k2));
    assert!(hits.contains(&k3));
}

#[test]
fn local_backend_delete_idempotent() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xFF);
        backend.delete(&k).expect("delete missing ok"); // never existed
    let mut meta = sample_meta();
    put_bytes(&backend, &k, b"x", &mut meta).expect("put");
    backend.delete(&k).expect("delete existing ok");
    backend.delete(&k).expect("delete missing again ok");
    assert!(backend.get(&k).expect("get").is_none());
}

#[test]
fn local_backend_meta_sidecar_persisted() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x55);
        let mut meta = sample_meta();
        meta.tags.insert("ci".into());
    meta.tags.insert("release:v0.5".into());
    put_bytes(&backend, &k, b"x", &mut meta).expect("put");

    // Read the sidecar file directly to verify structure.
    let path = backend.path_for(&k);
    let meta_path = path.with_extension("meta.json");
    let bytes = std::fs::read(&meta_path).expect("read sidecar");
        let restored: ArtifactMeta = serde_json::from_slice(&bytes).expect("deserialize");
    assert_eq!(restored.tags, meta.tags);
    assert_eq!(restored.recipe_namespace, meta.recipe_namespace);
}

#[test]
fn local_backend_path_for_fans_out_by_first_byte() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xAB);
        let path = backend.path_for(&k);
        // First two hex chars are the parent directory; remaining 62 are the file name.
        let parent = path.parent().unwrap().file_name().unwrap().to_string_lossy();
        assert_eq!(parent, "ab");
    let file_name = path.file_name().unwrap().to_string_lossy();
    assert_eq!(file_name.len(), 62);
}

// ─── CS-0054: SHA-256 content_hash integrity ────────────────────────────

/// `put` MUST stamp `meta.content_hash` with the SHA-256 of the bytes,
/// overwriting whatever placeholder the caller passed. The persisted
/// sidecar reflects the stamped value.
#[test]
fn put_computes_sha256_in_meta() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xC5);
        let bytes = b"hello cs-0054";
    // Caller passes the zero sentinel; put must overwrite.
    let mut meta = sample_meta();
    meta.content_hash = [0u8; 32];
    put_bytes(&backend, &k, bytes, &mut meta).expect("put");

    let path = backend.path_for(&k);
    let meta_path = path.with_extension("meta.json");
    let restored: ArtifactMeta =
        serde_json::from_slice(&std::fs::read(&meta_path).expect("read meta"))
            .expect("deserialize");

    // Known-answer check against an independent SHA-256.
    let expected = <Sha256 as Digest>::digest(bytes);
    let expected_arr: [u8; 32] = expected.into();
    assert_eq!(
        restored.content_hash, expected_arr,
        "put must stamp content_hash with SHA-256(bytes)"
    );
    assert_ne!(
        restored.content_hash,
        [0u8; 32],
        "put must overwrite the caller's zero sentinel"
    );
    // The in-memory `meta` is also stamped — caller observes the
    // canonical hash without re-reading the sidecar.
    assert_eq!(meta.content_hash, expected_arr);
}

/// Happy round-trip: put bytes, get bytes, content_hash verifies, bytes
/// returned are byte-identical to bytes put.
#[test]
fn get_succeeds_when_bytes_match_meta() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xC6);
        let bytes = b"round-trip payload";
    let mut meta = sample_meta();
    put_bytes(&backend, &k, bytes, &mut meta).expect("put");

    let got = get_bytes(&backend, &k).expect("get").expect("hit");
    assert_eq!(got, bytes, "get returns the bytes put under this key");
}

/// Tampering the bytes-on-disk after `put` MUST cause `get` to surface
/// as `None` once the streaming verification finalizes at EOF.
#[test]
fn get_fails_closed_on_byte_tamper() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xC7);
        let bytes = b"original bytes";
    let mut meta = sample_meta();
    put_bytes(&backend, &k, bytes, &mut meta).expect("put");

    // Mutate the on-disk artifact bytes — sidecar is left intact.
    let path = backend.path_for(&k);
    std::fs::write(&path, b"TAMPERED bytes").expect("tamper write");

    let got = get_bytes(&backend, &k).expect("get");
    assert!(
        got.is_none(),
        "byte tamper must surface as a miss (fail closed); got Some"
    );
}

/// Tampering the sidecar's `content_hash` MUST also cause `get` to
/// surface as `None` (the streaming reader fails at EOF — the bytes
/// hash to their actual value, which no longer matches the rewritten
/// sidecar's claim).
#[test]
fn get_fails_closed_on_meta_tamper() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xC8);
        let bytes = b"original bytes 2";
    let mut meta = sample_meta();
    put_bytes(&backend, &k, bytes, &mut meta).expect("put");

    // Read the sidecar, flip content_hash to a different value.
    let path = backend.path_for(&k);
    let meta_path = path.with_extension("meta.json");
    let mut restored: ArtifactMeta =
        serde_json::from_slice(&std::fs::read(&meta_path).expect("read meta"))
            .expect("deserialize");
    let bogus = <Sha256 as Digest>::digest(b"not the real bytes");
    restored.content_hash = bogus.into();
    std::fs::write(&meta_path, serde_json::to_vec(&restored).expect("serialize"))
        .expect("rewrite");

    let got = get_bytes(&backend, &k).expect("get");
    assert!(
        got.is_none(),
        "meta tamper must surface as a miss (fail closed); got Some"
    );
}

/// Missing sidecar — without a recorded hash, no integrity proof; miss.
#[test]
fn get_fails_closed_on_missing_meta() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xC9);
        let mut meta = sample_meta();
        put_bytes(&backend, &k, b"x", &mut meta).expect("put");

    let path = backend.path_for(&k);
    let meta_path = path.with_extension("meta.json");
    std::fs::remove_file(&meta_path).expect("remove sidecar");

        let got = backend.get(&k).expect("get");
    assert!(
        got.is_none(),
        "missing sidecar must surface as a miss; got Some"
    );
}

// ─── CS-0055: backend trait idempotency contract ────────────────────────

/// `put` of identical bytes to an existing key MUST succeed (idempotent
/// re-put).
#[test]
fn put_idempotent_on_same_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x55);
        let bytes = b"identical payload";
    let mut m1 = sample_meta();
    let mut m2 = sample_meta();
    put_bytes(&backend, &k, bytes, &mut m1).expect("first put");
    put_bytes(&backend, &k, bytes, &mut m2)
        .expect("re-put with identical bytes must succeed");

    // Bytes still readable round-trip.
    let got = get_bytes(&backend, &k).expect("get").expect("hit");
    assert_eq!(got, bytes);
    // Both meta values were stamped with the canonical hash.
    let expected: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
    assert_eq!(m1.content_hash, expected);
    assert_eq!(m2.content_hash, expected);
}

/// `put` of conflicting bytes to an existing key MUST be rejected.
#[test]
fn put_rejects_conflict() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x56);
        let bytes_a = b"payload alpha";
    let bytes_b = b"payload bravo";
    let mut m_a = sample_meta();
    let mut m_b = sample_meta();
    put_bytes(&backend, &k, bytes_a, &mut m_a).expect("put a");

    let err = put_bytes(&backend, &k, bytes_b, &mut m_b)
        .expect_err("conflicting put must error");
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("conflict"),
        "diagnostic must mention 'conflict'; got: {err_msg}"
    );
    let key_hex = hex::encode(k);
    assert!(
        err_msg.contains(&key_hex),
        "diagnostic must name the key in hex ({key_hex}); got: {err_msg}"
    );
    match err {
        BackendError::Other(_) => {}
        other => panic!("expected BackendError::Other, got {other:?}"),
    }

    // Prior bytes must still be on disk and readable.
    let got = get_bytes(&backend, &k).expect("get").expect("hit");
    assert_eq!(
        got, bytes_a,
        "conflicting put must NOT overwrite prior bytes"
    );
}

/// Missing sidecar — partial write recovery.
#[test]
fn put_recovers_from_missing_meta() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x57);
        let bytes = b"recovery payload";
    let mut m1 = sample_meta();
    put_bytes(&backend, &k, bytes, &mut m1).expect("put 1");

    let path = backend.path_for(&k);
    let meta_path = path.with_extension("meta.json");
    std::fs::remove_file(&meta_path).expect("remove sidecar");

        let mut m2 = sample_meta();
        put_bytes(&backend, &k, bytes, &mut m2)
            .expect("re-put after missing sidecar must succeed");

    let got = get_bytes(&backend, &k).expect("get").expect("hit");
    assert_eq!(got, bytes);
}

/// Corrupt sidecar — must not permanently brick a key.
#[test]
fn put_recovers_from_corrupt_meta() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x58);
        let bytes = b"corrupt-meta payload";
    let mut m1 = sample_meta();
    put_bytes(&backend, &k, bytes, &mut m1).expect("put 1");

    let path = backend.path_for(&k);
    let meta_path = path.with_extension("meta.json");
    std::fs::write(&meta_path, b"this is not JSON {{{ ::: garbage")
        .expect("corrupt sidecar");

        let mut m2 = sample_meta();
        put_bytes(&backend, &k, bytes, &mut m2)
            .expect("re-put after corrupt sidecar must succeed");

    let got = get_bytes(&backend, &k).expect("get").expect("hit");
    assert_eq!(got, bytes);
}

// ─── CS-0056: streaming + helpers ───────────────────────────────────────

/// Put bytes, get returns a streaming reader; reading to end produces
/// exactly the bytes that were put.
#[test]
fn local_backend_get_returns_streaming_reader() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x60);
        let bytes: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
        let mut meta = sample_meta();
        meta.size_bytes = bytes.len() as u64;
        put_bytes(&backend, &k, &bytes, &mut meta).expect("put");

    let mut reader = backend.get(&k).expect("get").expect("hit");
    let mut out = Vec::new();
    reader.read_to_end(&mut out).expect("read_to_end ok");
    assert_eq!(out, bytes);
}

/// On the streaming path, byte tamper surfaces as an `io::Error`
/// (`InvalidData`) when the reader hits EOF — not as silent
/// corruption. The `get_bytes` helper maps that to `Ok(None)`; the
/// raw streaming API surfaces it as the error directly.
#[test]
fn local_backend_get_streaming_errors_on_byte_tamper() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x61);
        let bytes = b"streaming-tamper original";
    let mut meta = sample_meta();
    put_bytes(&backend, &k, bytes, &mut meta).expect("put");

    let path = backend.path_for(&k);
    std::fs::write(&path, b"streaming-tamper TAMPERED").expect("tamper write");

    let mut reader = backend.get(&k).expect("get").expect("hit");
    let mut out = Vec::new();
    let err = reader
        .read_to_end(&mut out)
        .expect_err("streaming verifier must raise InvalidData on tamper");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// `put` with the zero sentinel stamps `meta.content_hash` in-place to
    /// `SHA-256(bytes)` — the streaming-path equivalent of the CS-0054
    /// stamp.
    #[test]
    fn local_backend_put_streams_with_zero_sentinel() {
        let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let k = key(0x62);
    let bytes = b"sentinel stamp test";
    let mut meta = sample_meta();
    meta.content_hash = ArtifactMeta::zero_content_hash();
    put_bytes(&backend, &k, bytes, &mut meta).expect("put");

    let expected: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
    assert_eq!(
        meta.content_hash, expected,
        "put must stamp meta.content_hash in-place"
    );
}

/// `put` with a non-zero `content_hash` that does NOT match the
/// streamed bytes is a caller bug — must error, must not persist.
#[test]
fn local_backend_put_rejects_caller_hash_mismatch() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x63);
        let bytes = b"caller-bug detection";
    let mut meta = sample_meta();
    // A non-zero hash that is provably *not* SHA-256(bytes).
    let bogus: [u8; 32] = <Sha256 as Digest>::digest(b"different bytes entirely").into();
    meta.content_hash = bogus;

    let err = put_bytes(&backend, &k, bytes, &mut meta)
        .expect_err("caller-claimed hash mismatch must error");
        let msg = err.to_string();
        assert!(
            msg.contains("caller-claimed content_hash"),
        "diagnostic must mention caller-claimed mismatch; got: {msg}"
    );
    // No artifact persisted at this key.
    assert!(backend.get(&k).expect("get").is_none());
}

/// `put` with a non-zero `content_hash` that DOES match the streamed
/// bytes succeeds (caller pre-computed the canonical hash).
#[test]
fn local_backend_put_accepts_caller_hash_match() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x64);
        let bytes = b"caller pre-computed hash";
    let mut meta = sample_meta();
    let pre: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
    meta.content_hash = pre;
    put_bytes(&backend, &k, bytes, &mut meta).expect("put with matching hash ok");
    assert_eq!(meta.content_hash, pre);
}

/// Convenience helper round-trip: `put_bytes` / `get_bytes` produce
/// the right bytes through the streaming trait surface.
#[test]
fn put_bytes_helper_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x65);
        let bytes = b"helper round trip";
    let mut meta = sample_meta();
    put_bytes(&backend, &k, bytes, &mut meta).expect("put_bytes");
    let got = get_bytes(&backend, &k).expect("get_bytes").expect("hit");
    assert_eq!(got, bytes);
}

/// `get_bytes` returns `Ok(None)` on tamper rather than an error —
/// the helper folds the streaming `InvalidData` back into the
/// fail-closed shape callers already expect.
#[test]
fn get_bytes_helper_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x66);
        let bytes = b"helper tamper-surface test";
    let mut meta = sample_meta();
    put_bytes(&backend, &k, bytes, &mut meta).expect("put");

    // Happy path: bytes round-trip.
    assert_eq!(
        get_bytes(&backend, &k).expect("get").expect("hit"),
        bytes
    );

    // Tamper: helper folds InvalidData into Ok(None).
    let path = backend.path_for(&k);
    std::fs::write(&path, b"helper tamper-surface ATTACKER").expect("tamper");
        assert!(
            get_bytes(&backend, &k).expect("get").is_none(),
        "tamper must surface as None through the helper"
        );
    }

    // ─── CS-0057: BackendConfig threading ───────────────────────────────────

    /// `BackendConfig::default()` matches the values pinned by the
    /// CS-0057 spec. Sanity check — if anyone tightens or loosens the
    /// defaults later, this test is the single source of truth they
    /// should review.
    #[test]
    fn backend_config_default_values() {
        let cfg = BackendConfig::default();
        assert_eq!(cfg.timeout, std::time::Duration::from_secs(30));
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.backoff_initial, std::time::Duration::from_millis(100));
        assert_eq!(cfg.backoff_max, std::time::Duration::from_secs(5));
        assert_eq!(cfg.max_artifact_bytes, 1024 * 1024 * 1024);
    }

    /// `LocalBackend::with_config` stores the config and exposes it via
    /// the `config()` accessor — observable proof that the constructor
    /// honoured the override rather than silently substituting defaults.
    #[test]
    fn local_backend_with_config_honored() {
        let dir = tempfile::tempdir().expect("tempdir");
    let custom = BackendConfig {
        timeout: std::time::Duration::from_secs(7),
        max_retries: 11,
        backoff_initial: std::time::Duration::from_millis(42),
        backoff_max: std::time::Duration::from_secs(13),
        max_artifact_bytes: 4096,
    };
    let backend = LocalBackend::with_config(dir.path().to_path_buf(), custom.clone());
    assert_eq!(backend.config().timeout, custom.timeout);
    assert_eq!(backend.config().max_retries, custom.max_retries);
    assert_eq!(backend.config().backoff_initial, custom.backoff_initial);
    assert_eq!(backend.config().backoff_max, custom.backoff_max);
    assert_eq!(backend.config().max_artifact_bytes, custom.max_artifact_bytes);
}

/// `put` of bytes that exceed `max_artifact_bytes` MUST be rejected
/// during streaming, with a diagnostic that names the limit. No
/// artifact persisted at the key.
#[test]
fn local_backend_put_rejects_oversize_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
        let cfg = BackendConfig {
            max_artifact_bytes: 100,
            ..BackendConfig::default()
        };
        let backend = LocalBackend::with_config(dir.path().to_path_buf(), cfg);
        let k = key(0x70);
        let bytes = vec![0xABu8; 200]; // 2x the cap
        let mut meta = sample_meta();
        let err = put_bytes(&backend, &k, &bytes, &mut meta)
            .expect_err("oversize put must error");
    let msg = err.to_string();
    assert!(
        msg.contains("exceeds"),
        "diagnostic must mention 'exceeds'; got: {msg}"
    );
    assert!(
        msg.contains("100"),
        "diagnostic must name the limit (100); got: {msg}"
    );
    match err {
        BackendError::Other(_) => {}
        other => panic!("expected BackendError::Other, got {other:?}"),
    }

    // No artifact persisted at this key — the rejected put must not
    // leak partial bytes through to a reader.
    assert!(backend.get(&k).expect("get").is_none());
}

/// `put` of exactly `max_artifact_bytes` bytes succeeds — the cap is
/// "MUST NOT exceed", not "MUST be strictly less than".
#[test]
fn local_backend_put_accepts_artifact_at_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
        let cfg = BackendConfig {
            max_artifact_bytes: 100,
            ..BackendConfig::default()
        };
        let backend = LocalBackend::with_config(dir.path().to_path_buf(), cfg);
        let k = key(0x71);
        let bytes = vec![0xCDu8; 100]; // exactly at the cap
        let mut meta = sample_meta();
        put_bytes(&backend, &k, &bytes, &mut meta).expect("put at limit ok");

    let got = get_bytes(&backend, &k).expect("get").expect("hit");
    assert_eq!(got, bytes);
}

// ─── COOK-166 / CS-0110: determinant manifest sidecar ───────────────────

fn sample_manifest() -> DeterminantManifest {
    use std::collections::BTreeMap;
    let mut inputs = BTreeMap::new();
    inputs.insert("src/a.c".to_string(), 0xAABB_u64);
    let mut env = BTreeMap::new();
    env.insert("CC".to_string(), "clang".to_string());
    let mut probes = BTreeMap::new();
    probes.insert("host".to_string(), "\"x86_64-linux\"".to_string());
    DeterminantManifest {
        schema_version: 5,
        recipe_namespace: "cook/Cookfile::build".into(),
        key: "ab".repeat(32),
        command_hash: 0x1111,
        env_contribution: 0x2222,
        seal_contribution: 0x3333,
        inputs,
        output_paths: vec!["build/a.o".into()],
        empty_dir_outputs: Vec::new(),
        consulted_env: env,
        sealed_probes: probes,
        observation: None,
    }
}

#[test]
fn local_backend_manifest_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x80);
        let m = sample_manifest();
        backend.put_manifest(&k, &m).expect("put_manifest");
    let got = backend.get_manifest(&k).expect("get_manifest").expect("present");
    assert_eq!(got, m);
}

#[test]
fn local_backend_manifest_miss_returns_none() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        assert!(backend.get_manifest(&key(0x81)).expect("get_manifest").is_none());
}

#[test]
fn local_backend_manifest_stored_beside_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x82);
        backend.put_manifest(&k, &sample_manifest()).expect("put_manifest");
    let prov = backend.path_for(&k).with_extension("provenance.json");
    assert!(prov.exists(), "manifest sidecar must exist at {}", prov.display());
}

// ─── COOK-180: get_with_meta seam ───────────────────────────────────────

#[test]
fn local_get_with_meta_returns_mode_and_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let be = LocalBackend::new(tmp.path().to_path_buf());
    let k = [7u8; 32];
    let mut meta = sample_meta();
    meta.mode = 0o755;
    meta.kind = Some("symlink".into());
    meta.target = Some("../sib".into());
    put_bytes(&be, &k, b"", &mut meta).unwrap();

    let (mut reader, got) = be.get_with_meta(&k).unwrap().expect("hit");
    assert_eq!(got.mode, 0o755);
    assert_eq!(got.kind.as_deref(), Some("symlink"));
    assert_eq!(got.target.as_deref(), Some("../sib"));
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut body).unwrap();
    assert!(body.is_empty());
}

// ─── COOK-233: last-access tracking (blob mtime touched on read) ────────
//
// LRU eviction (COOK-234) needs a last-access signal. It is the blob's
// mtime, bumped on every hit by a single `utimensat`. These tests pin the
// three properties that make that safe: it fires on hits, it mutates
// nothing but the blob's mtime, and it cannot break a read.
//
// Every case is rooted at a `tempfile::tempdir()` — never the real CAS.

/// An mtime old enough that "now" is unambiguously greater: 2001-09-09.
const OLD_STAMP: filetime::FileTime = filetime::FileTime::from_unix_time(1_000_000_000, 0);

fn mtime_of(path: &std::path::Path) -> filetime::FileTime {
    filetime::FileTime::from_last_modification_time(
        &std::fs::metadata(path).expect("stat"),
    )
}

/// Case 1 — `get_with_meta` on a hit advances the blob's mtime.
#[test]
fn get_with_meta_touches_blob_mtime_on_hit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let k = key(0xD0);
    let mut meta = sample_meta();
    put_bytes(&backend, &k, b"touch me", &mut meta).expect("put");

    let blob = backend.path_for(&k);
    filetime::set_file_mtime(&blob, OLD_STAMP).expect("backdate blob");

    let hit = backend.get_with_meta(&k).expect("get_with_meta");
    assert!(hit.is_some(), "expected a hit");

    assert!(
        mtime_of(&blob) > OLD_STAMP,
        "get_with_meta on a hit must advance blob mtime; still {:?}",
        mtime_of(&blob)
    );
}

/// Case 2 — `get` (which delegates to `get_with_meta`) does the same.
#[test]
fn get_touches_blob_mtime_on_hit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let k = key(0xD1);
    let mut meta = sample_meta();
    put_bytes(&backend, &k, b"touch me too", &mut meta).expect("put");

    let blob = backend.path_for(&k);
    filetime::set_file_mtime(&blob, OLD_STAMP).expect("backdate blob");

    assert!(backend.get(&k).expect("get").is_some(), "expected a hit");
    assert!(
        mtime_of(&blob) > OLD_STAMP,
        "get on a hit must advance blob mtime; still {:?}",
        mtime_of(&blob)
    );
}

/// Case 3 — only the blob is touched. Neither the `.meta.json` nor the
/// `.provenance.json` sidecar is restamped or rewritten: zero write
/// amplification on the read path is the whole reason mtime was chosen
/// over a `last_access` JSON field. Both sidecars are asserted so this
/// stays a regression guard if anything is later added near the touch.
#[test]
fn read_does_not_restamp_or_rewrite_sidecars() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let k = key(0xD2);
    let mut meta = sample_meta();
    put_bytes(&backend, &k, b"sidecar untouched", &mut meta).expect("put");
    backend.put_manifest(&k, &sample_manifest()).expect("put_manifest");

    let blob = backend.path_for(&k);
    let meta_path = blob.with_extension("meta.json");
    let prov_path = blob.with_extension("provenance.json");
    let meta_before = std::fs::read(&meta_path).expect("read meta sidecar");
    let prov_before = std::fs::read(&prov_path).expect("read provenance sidecar");
    filetime::set_file_mtime(&blob, OLD_STAMP).expect("backdate blob");
    filetime::set_file_mtime(&meta_path, OLD_STAMP).expect("backdate meta sidecar");
    filetime::set_file_mtime(&prov_path, OLD_STAMP).expect("backdate provenance sidecar");

    assert!(backend.get(&k).expect("get").is_some(), "expected a hit");

    assert_eq!(
        mtime_of(&meta_path),
        OLD_STAMP,
        "meta sidecar mtime must be untouched by a read"
    );
    assert_eq!(
        std::fs::read(&meta_path).expect("re-read meta sidecar"),
        meta_before,
        "meta sidecar bytes must be untouched by a read"
    );
    assert_eq!(
        mtime_of(&prov_path),
        OLD_STAMP,
        "provenance sidecar mtime must be untouched by a read"
    );
    assert_eq!(
        std::fs::read(&prov_path).expect("re-read provenance sidecar"),
        prov_before,
        "provenance sidecar bytes must be untouched by a read"
    );
    assert!(mtime_of(&blob) > OLD_STAMP, "blob mtime should have advanced");
}

/// Case 4a — a miss on a missing sidecar returns before the touch.
#[test]
fn missing_sidecar_miss_does_not_touch_blob() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let k = key(0xD3);
    let mut meta = sample_meta();
    put_bytes(&backend, &k, b"orphaned bytes", &mut meta).expect("put");

    let blob = backend.path_for(&k);
    std::fs::remove_file(blob.with_extension("meta.json")).expect("remove sidecar");
    filetime::set_file_mtime(&blob, OLD_STAMP).expect("backdate blob");

    assert!(backend.get(&k).expect("get").is_none(), "expected a miss");
    assert_eq!(
        mtime_of(&blob),
        OLD_STAMP,
        "a miss must leave blob mtime untouched"
    );
}

/// Case 4b — a miss on a malformed sidecar likewise returns before the touch.
#[test]
fn malformed_sidecar_miss_does_not_touch_blob() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let k = key(0xD4);
    let mut meta = sample_meta();
    put_bytes(&backend, &k, b"bytes with bad meta", &mut meta).expect("put");

    let blob = backend.path_for(&k);
    std::fs::write(blob.with_extension("meta.json"), b"{ not json").expect("corrupt sidecar");
    filetime::set_file_mtime(&blob, OLD_STAMP).expect("backdate blob");

    assert!(backend.get(&k).expect("get").is_none(), "expected a miss");
    assert_eq!(
        mtime_of(&blob),
        OLD_STAMP,
        "a miss must leave blob mtime untouched"
    );
}

/// Case 5 — a failing touch is swallowed, never propagated.
///
/// The spec asks for "a read whose touch fails does not error the get", but
/// arranging a genuinely-failing `utimensat` inside an otherwise-successful
/// read is not portable: the file owner may set times regardless of mode,
/// and as root it is impossible. Exercising the helper directly against a
/// path that cannot be touched, plus `touch_on_read`'s `-> ()` signature
/// (there is no error channel to propagate), makes the guarantee structural
/// rather than incidental.
#[test]
fn touch_on_read_swallows_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("no").join("such").join("blob");
    assert!(!missing.exists());
    // Must return normally — no panic, no error to observe.
    touch_on_read(&missing);
}

// ─── delete / apply_eviction: provenance sidecars must not leak ────────
//
// `delete` has never removed `.provenance.json`; the developer's real store
// has 6,433 orphaned sidecars because of it. `apply_eviction` is the new
// entry point `cook cache gc` drives through `plan_eviction`'s output.
//
// Every test here is rooted at its own `tempfile::tempdir()` — never the
// real CAS at `~/.cache/cook/cloud`.

/// Put a blob, write a provenance sidecar via `put_manifest`, delete the
/// key, assert all three on-disk paths (blob, `.meta.json`,
/// `.provenance.json`) are gone. This test must fail against the unfixed
/// `delete` (it only ever removed the blob and `.meta.json`).
#[test]
fn delete_removes_blob_meta_and_provenance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let k = key(0x90);
    let mut meta = sample_meta();
    put_bytes(&backend, &k, b"provenance leak repro", &mut meta).expect("put");
    backend.put_manifest(&k, &sample_manifest()).expect("put_manifest");

    let blob = backend.path_for(&k);
    let meta_path = blob.with_extension("meta.json");
    let prov_path = blob.with_extension("provenance.json");
    assert!(blob.exists(), "blob must exist before delete");
    assert!(meta_path.exists(), "meta sidecar must exist before delete");
    assert!(prov_path.exists(), "provenance sidecar must exist before delete");

    backend.delete(&k).expect("delete ok");

    assert!(!blob.exists(), "delete must remove the blob");
    assert!(!meta_path.exists(), "delete must remove the meta sidecar");
    assert!(
        !prov_path.exists(),
        "delete must remove the provenance sidecar (the leak this fixes)"
    );
}

/// A key that was never given a provenance sidecar deletes cleanly — the
/// best-effort `remove_file` on a sidecar that was never written is not an
/// error.
#[test]
fn delete_of_a_key_with_no_provenance_is_not_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let k = key(0x91);
    let mut meta = sample_meta();
    put_bytes(&backend, &k, b"no provenance here", &mut meta).expect("put");

    let prov_path = backend.path_for(&k).with_extension("provenance.json");
    assert!(!prov_path.exists());

    backend.delete(&k).expect("delete without provenance must still be ok");
    assert!(backend.get(&k).expect("get").is_none());
}

/// Seed several objects with distinct keys, sizes, and last-access times;
/// build an `EvictPlan` (via `plan_eviction`) that names only a subset as
/// victims; apply it; assert the untouched objects still have all three
/// files, and the outcome reports the victims' bytes.
#[test]
fn apply_eviction_removes_only_the_planned_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());

    let victim_key = key(0xA0);
    let survivor_key = key(0xA1);

    let mut m_victim = sample_meta();
    m_victim.size_bytes = 10;
    put_bytes(&backend, &victim_key, b"0123456789", &mut m_victim).expect("put victim");
    backend.put_manifest(&victim_key, &sample_manifest()).expect("put_manifest victim");

    let mut m_survivor = sample_meta();
    m_survivor.size_bytes = 5;
    put_bytes(&backend, &survivor_key, b"abcde", &mut m_survivor).expect("put survivor");
    backend
        .put_manifest(&survivor_key, &sample_manifest())
        .expect("put_manifest survivor");

    // Both blobs are on disk now with real on-disk sizes; enumerate to get
    // trustworthy `EvictCandidate.size` (matches `LocalBackend::enumerate`'s
    // contract — size comes from `fs::metadata`, not the caller-set field).
    let candidates = backend.enumerate().expect("enumerate");
    assert_eq!(candidates.len(), 2);

    // Plan an age-based eviction that names only `victim_key`: give it an
    // ancient last_access and the survivor a fresh one, then age-sweep
    // anything older than a threshold that only catches the victim.
    let now = 1_000_000_000u64;
    let mut aged_candidates = candidates.clone();
    for c in aged_candidates.iter_mut() {
        if c.key == victim_key {
            c.last_access = 100; // ancient
        } else {
            c.last_access = now - 10; // recent
        }
    }
    let policy = EvictPolicy {
        max_size: None,
        older_than: Some(std::time::Duration::from_secs(1000)),
        low_water: 1.0,
    };
    let plan = plan_eviction(&aged_candidates, &policy, now);
    assert_eq!(plan.victims.len(), 1, "plan must name exactly one victim");
    assert_eq!(plan.victims[0].key, victim_key);

    let outcome = backend.apply_eviction(&plan).expect("apply_eviction");
    assert_eq!(outcome.objects, 1);
    assert_eq!(outcome.bytes, 10);

    // Victim: all three files gone.
    let victim_blob = backend.path_for(&victim_key);
    assert!(!victim_blob.exists());
    assert!(!victim_blob.with_extension("meta.json").exists());
    assert!(!victim_blob.with_extension("provenance.json").exists());

    // Survivor: untouched, all three files still present.
    let survivor_blob = backend.path_for(&survivor_key);
    assert!(survivor_blob.exists());
    assert!(survivor_blob.with_extension("meta.json").exists());
    assert!(survivor_blob.with_extension("provenance.json").exists());
}

/// If a victim's blob is removed out-of-band (a concurrent sweep, or a
/// developer poking at the store) before `apply_eviction` runs, the object
/// is already gone: `apply_eviction` must still clean up any stray
/// sidecars, must not error, and must NOT double-count the bytes in the
/// outcome (they were already freed by whoever removed the blob first).
#[test]
fn apply_eviction_tolerates_a_victim_that_vanished_mid_sweep() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let k = key(0xB0);
    let mut meta = sample_meta();
    meta.size_bytes = 7;
    put_bytes(&backend, &k, b"vanish!", &mut meta).expect("put");
    backend.put_manifest(&k, &sample_manifest()).expect("put_manifest");

    let blob = backend.path_for(&k);
    // Simulate a concurrent sweep already having removed the blob, leaving
    // the sidecars stranded (a half-removed object).
    std::fs::remove_file(&blob).expect("simulate concurrent removal");
    assert!(!blob.exists());
    assert!(blob.with_extension("meta.json").exists());
    assert!(blob.with_extension("provenance.json").exists());

    let plan = EvictPlan {
        victims: vec![EvictCandidate {
            key: k,
            size: 7,
            last_access: 1,
            kind: None,
            recipe_namespace: String::new(),
        }],
        freed_bytes: 7,
        total_before: 7,
        total_after: 0,
        count_before: 1,
    };

    let outcome = backend
        .apply_eviction(&plan)
        .expect("apply_eviction must not error on a vanished blob");
    assert_eq!(
        outcome.objects, 0,
        "a vanished-blob victim must not be counted as removed"
    );
    assert_eq!(
        outcome.bytes, 0,
        "a vanished-blob victim must not double-count freed bytes"
    );

    // Stranded sidecars are cleaned up anyway.
    assert!(!blob.with_extension("meta.json").exists());
    assert!(!blob.with_extension("provenance.json").exists());
}

/// An empty plan is a no-op: zero objects, zero bytes, no error.
#[test]
fn apply_eviction_of_an_empty_plan_is_a_no_op() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let k = key(0xC0);
    let mut meta = sample_meta();
    put_bytes(&backend, &k, b"untouched", &mut meta).expect("put");
    backend.put_manifest(&k, &sample_manifest()).expect("put_manifest");

    let plan = EvictPlan {
        victims: Vec::new(),
        freed_bytes: 0,
        total_before: 9,
        total_after: 9,
        count_before: 1,
    };

    let outcome = backend.apply_eviction(&plan).expect("apply_eviction empty plan");
    assert_eq!(outcome.objects, 0);
    assert_eq!(outcome.bytes, 0);

    // Untouched object still fully present.
    let blob = backend.path_for(&k);
    assert!(blob.exists());
    assert!(blob.with_extension("meta.json").exists());
    assert!(blob.with_extension("provenance.json").exists());
}
