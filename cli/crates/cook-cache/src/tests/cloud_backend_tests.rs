use super::*;
use sha2::{Digest, Sha256};

fn key(byte: u8) -> CloudKey {
    let mut k = [0u8; 32];
    k[0] = byte;
    k
}

fn sample_meta() -> ArtifactMeta {
    ArtifactMeta {
        recipe_namespace: "cook/Cookfile::build".into(),
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: 3,
        size_bytes: 0,
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

/// Build a backend with the given config pointed at the mockito
/// server's URL. Tight backoff so retry tests don't drag.
fn make_backend(server_url: &str, max_retries: u32) -> CloudBackend {
    let cfg = BackendConfig {
        timeout: Duration::from_secs(5),
        max_retries,
        backoff_initial: Duration::from_millis(1),
        backoff_max: Duration::from_millis(5),
        max_artifact_bytes: 1024 * 1024,
    };
    CloudBackend::new(server_url.to_string(), "test-token-zzz".into(), cfg)
}

#[test]
fn cloud_backend_get_round_trips() {
    let mut server = mockito::Server::new();
    let bytes = b"hello cloud backend";
    let hash: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
    let k = key(0x10);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));

    let m = server
        .mock("GET", url_path.as_str())
        .match_header("authorization", "Bearer test-token-zzz")
        .with_status(200)
        .with_header("X-Cook-Content-Hash", &hex::encode(hash))
        .with_header("X-Cook-Size-Bytes", &bytes.len().to_string())
        .with_body(bytes)
        .create();

    let backend = make_backend(&server.url(), 0);
    let mut reader = backend.get(&k).expect("get").expect("hit");
    let mut out = Vec::new();
    reader.read_to_end(&mut out).expect("read_to_end");
    assert_eq!(out, bytes);
    m.assert();
}

#[test]
fn cloud_backend_get_returns_none_on_404() {
    let mut server = mockito::Server::new();
    let k = key(0x11);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    let m = server
        .mock("GET", url_path.as_str())
        .with_status(404)
        .create();

    let backend = make_backend(&server.url(), 0);
    let result = backend.get(&k).expect("get");
    assert!(result.is_none());
    m.assert();
}

#[test]
fn cloud_backend_get_fails_closed_on_byte_tamper() {
    let mut server = mockito::Server::new();
    let bytes = b"the real bytes";
    // Hash for *different* bytes — guarantees mismatch.
    let bogus: [u8; 32] = <Sha256 as Digest>::digest(b"DIFFERENT bytes").into();
    let k = key(0x12);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));

    let _m = server
        .mock("GET", url_path.as_str())
        .with_status(200)
        .with_header("X-Cook-Content-Hash", &hex::encode(bogus))
        .with_body(bytes)
        .create();

    let backend = make_backend(&server.url(), 0);
    let mut reader = backend.get(&k).expect("get").expect("hit");
    let mut out = Vec::new();
    let err = reader
        .read_to_end(&mut out)
        .expect_err("VerifyingReader must surface InvalidData on hash mismatch");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn cloud_backend_put_round_trips() {
    let mut server = mockito::Server::new();
    let k = key(0x20);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    let payload = b"some bytes to upload";

    let m = server
        .mock("PUT", url_path.as_str())
        .match_header("authorization", "Bearer test-token-zzz")
        .match_body(mockito::Matcher::Exact(
            String::from_utf8(payload.to_vec()).unwrap(),
        ))
        .with_status(200)
        .create();

    let backend = make_backend(&server.url(), 0);
    let mut meta = sample_meta();
    meta.size_bytes = payload.len() as u64;
    let mut cursor = std::io::Cursor::new(payload.to_vec());
    backend.put(&k, &mut cursor, &mut meta).expect("put");
    m.assert();
}

#[test]
fn cloud_backend_put_rejects_oversize() {
    // The cap is enforced client-side BEFORE the request completes.
    // Mockito may still observe a partial connection; use `expect_at_most`
    // to avoid coupling the test to the abort timing.
    let mut server = mockito::Server::new();
    let k = key(0x21);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    let _m = server
        .mock("PUT", url_path.as_str())
        .with_status(200)
        .expect_at_most(1)
        .create();

    let cfg = BackendConfig {
        timeout: Duration::from_secs(5),
        max_retries: 0,
        backoff_initial: Duration::from_millis(1),
        backoff_max: Duration::from_millis(5),
        max_artifact_bytes: 100, // small cap
    };
    let backend = CloudBackend::new(server.url(), "test-token-zzz".into(), cfg);
    let payload = vec![0xABu8; 500]; // 5x the cap
    let mut meta = sample_meta();
    let mut cursor = std::io::Cursor::new(payload);
    let err = backend
        .put(&k, &mut cursor, &mut meta)
        .expect_err("oversize put must error");
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds"),
        "diagnostic must mention 'exceeds'; got: {msg}"
    );
    assert!(
        msg.contains("100"),
        "diagnostic must name the cap (100); got: {msg}"
    );
}

#[test]
fn cloud_backend_put_handles_409_conflict() {
    let mut server = mockito::Server::new();
    let k = key(0x22);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    let m = server
        .mock("PUT", url_path.as_str())
        .with_status(409)
        .with_body("server-side bytes differ")
            .create();

        let backend = make_backend(&server.url(), 0);
        let mut meta = sample_meta();
        let mut cursor = std::io::Cursor::new(b"new bytes".to_vec());
    let err = backend
        .put(&k, &mut cursor, &mut meta)
        .expect_err("409 must error");
        let msg = err.to_string();
        assert!(
            msg.contains("conflict"),
        "diagnostic must mention 'conflict'; got: {msg}"
    );
    match err {
        BackendError::Other(_) => {}
        other => panic!("expected BackendError::Other, got {other:?}"),
    }
    m.assert();
}

#[test]
fn cloud_backend_batch_query_round_trips() {
    let mut server = mockito::Server::new();
    let k1 = key(0x30);
    let k2 = key(0x31);
    let k3 = key(0x32);
    // Server says only k1 and k3 are present.
    let response_body = serde_json::json!({
        "present": [hex::encode(k1), hex::encode(k3)]
    })
    .to_string();

    let m = server
        .mock("POST", "/v1/artifacts/batch_query")
        .match_header("authorization", "Bearer test-token-zzz")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(response_body)
        .create();

    let backend = make_backend(&server.url(), 0);
    let hits = backend.batch_query(&[k1, k2, k3]).expect("batch_query");
    assert!(hits.contains(&k1));
    assert!(!hits.contains(&k2));
    assert!(hits.contains(&k3));
    m.assert();
}

#[test]
fn cloud_backend_retries_on_5xx() {
    let mut server = mockito::Server::new();
    let k = key(0x40);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    // First call: 503. Second call: 404 (= Ok(None)).
    // Use create() ordering — mockito returns mocks in registration
    // order until each is exhausted.
    let m_503 = server
        .mock("GET", url_path.as_str())
        .with_status(503)
        .expect(1)
        .create();
    let m_404 = server
        .mock("GET", url_path.as_str())
        .with_status(404)
        .expect(1)
        .create();

    let backend = make_backend(&server.url(), 3);
    let result = backend.get(&k).expect("get");
    assert!(result.is_none(), "second call should be 404 → Ok(None)");
    m_503.assert();
    m_404.assert();
}

#[test]
fn cloud_backend_does_not_retry_on_401() {
    let mut server = mockito::Server::new();
    let k = key(0x41);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    let m = server
        .mock("GET", url_path.as_str())
        .with_status(401)
        .expect(1) // exactly one call — no retry on auth failure
        .create();

    let backend = make_backend(&server.url(), 5);
    match backend.get(&k) {
        Err(BackendError::Unauthorized(_)) => {}
        Err(other) => panic!("expected BackendError::Unauthorized, got {other:?}"),
        Ok(_) => panic!("expected error, got success"),
    }
    m.assert();
}

/// CS-0058: a 429 with NO `Retry-After` header is terminal. The retry
/// shell sees `QuotaExceeded(None)` and falls through. CS-0059
/// preserves this — the `None` payload is the "no hint" sentinel.
#[test]
fn cloud_backend_does_not_retry_on_429_without_retry_after() {
    let mut server = mockito::Server::new();
    let k = key(0x42);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    let m = server
        .mock("GET", url_path.as_str())
        .with_status(429)
        .expect(1)
        .create();

    let backend = make_backend(&server.url(), 5);
    match backend.get(&k) {
        Err(BackendError::QuotaExceeded(None)) => {}
        Err(other) => panic!("expected BackendError::QuotaExceeded(None), got {other:?}"),
        Ok(_) => panic!("expected error, got success"),
    }
    m.assert();
}

/// CS-0059: a 429 WITH `Retry-After: <delta-seconds>` is retryable.
/// The retry shell sleeps the server-supplied hint (clamped to
/// `[backoff_initial, backoff_max]`), then retries. Server returns
/// 429 on call 1 and 200 on call 2; the test asserts exactly two
/// calls happened and that elapsed time is at least the hinted delay.
#[test]
fn cloud_backend_honors_retry_after_on_429() {
    use std::time::Instant;

    let mut server = mockito::Server::new();
    let bytes = b"";
    let hash: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
    let k = key(0x43);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));

    // Call 1: 429 with Retry-After: 1 (one second).
    let m_429 = server
        .mock("GET", url_path.as_str())
        .with_status(429)
        .with_header("Retry-After", "1")
        .expect(1)
        .create();
    // Call 2: 200 with valid headers and empty body.
    let m_200 = server
        .mock("GET", url_path.as_str())
        .with_status(200)
        .with_header("X-Cook-Content-Hash", &hex::encode(hash))
        .with_header("X-Cook-Size-Bytes", "0")
        .with_header("X-Cook-Schema-Version", "3")
        .with_header("X-Cook-Recipe-Namespace", "cook/Cookfile::build")
        .with_header("X-Cook-Output-Index", "0")
        .with_header("X-Cook-Output-Path", "build/foo.o")
        .with_body(bytes)
        .expect(1)
        .create();

    // backoff_max = 5s gives the 1s hint room to land unclamped.
    let cfg = BackendConfig {
        timeout: Duration::from_secs(5),
        max_retries: 3,
        backoff_initial: Duration::from_millis(1),
        backoff_max: Duration::from_secs(5),
        max_artifact_bytes: 1024 * 1024,
    };
    let backend = CloudBackend::new(server.url(), "test-token-zzz".into(), cfg);

    let started = Instant::now();
    let mut reader = backend.get(&k).expect("get ok").expect("present");
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).expect("read body");
    let elapsed = started.elapsed();

    assert_eq!(buf, bytes);
    assert!(
        elapsed >= Duration::from_millis(900),
        "Retry-After=1s must produce at least ~1s elapsed, got {elapsed:?}"
    );
    m_429.assert();
    m_200.assert();
}

/// CS-0059: a `Retry-After` hint that exceeds `backoff_max` is
/// clamped down. Server returns 429 with `Retry-After: 600` (10 min)
/// and a tight `backoff_max = 50ms`. The retry must proceed within a
/// few hundred ms — far below the 10-minute hint.
#[test]
fn cloud_backend_clamps_retry_after_to_backoff_max() {
    use std::time::Instant;

    let mut server = mockito::Server::new();
    let bytes = b"";
    let hash: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
    let k = key(0x44);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));

    let m_429 = server
        .mock("GET", url_path.as_str())
        .with_status(429)
        .with_header("Retry-After", "600")
        .expect(1)
        .create();
    let m_200 = server
        .mock("GET", url_path.as_str())
        .with_status(200)
        .with_header("X-Cook-Content-Hash", &hex::encode(hash))
        .with_header("X-Cook-Size-Bytes", "0")
        .with_header("X-Cook-Schema-Version", "3")
        .with_header("X-Cook-Recipe-Namespace", "cook/Cookfile::build")
        .with_header("X-Cook-Output-Index", "0")
        .with_header("X-Cook-Output-Path", "build/foo.o")
        .with_body(bytes)
        .expect(1)
        .create();

    let cfg = BackendConfig {
        timeout: Duration::from_secs(5),
        max_retries: 3,
        backoff_initial: Duration::from_millis(1),
        backoff_max: Duration::from_millis(50),
        max_artifact_bytes: 1024 * 1024,
    };
    let backend = CloudBackend::new(server.url(), "test-token-zzz".into(), cfg);

    let started = Instant::now();
    let mut reader = backend.get(&k).expect("get ok").expect("present");
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).expect("read body");
    let elapsed = started.elapsed();

    assert_eq!(buf, bytes);
    // Generous upper bound — even on a slow CI runner, 10 minutes of
    // unclamped sleep would blow this by orders of magnitude.
    assert!(
        elapsed < Duration::from_secs(2),
        "Retry-After=600s must clamp to backoff_max=50ms, got {elapsed:?}"
    );
    m_429.assert();
    m_200.assert();
}

/// CS-0059: HTTP-date form of `Retry-After` is recognised by the
/// parser but not honoured (delta-seconds only in v1). Maps to
/// `QuotaExceeded(None)` → terminal, no retry. Pins the parser's
/// fall-through behaviour.
#[test]
fn cloud_backend_retry_after_http_date_falls_through_to_none() {
    let mut server = mockito::Server::new();
    let k = key(0x45);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    let m = server
        .mock("GET", url_path.as_str())
        .with_status(429)
        .with_header("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT")
        .expect(1)
        .create();

    let backend = make_backend(&server.url(), 5);
    match backend.get(&k) {
        Err(BackendError::QuotaExceeded(None)) => {}
        Err(other) => panic!(
            "HTTP-date form must map to QuotaExceeded(None) (terminal), got {other:?}"
        ),
        Ok(_) => panic!("expected error, got success"),
    }
    m.assert();
}

#[test]
fn cloud_backend_health_returns_ok_on_200() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("GET", "/v1/health")
        .match_header("authorization", "Bearer test-token-zzz")
        .with_status(200)
        .create();

    let backend = make_backend(&server.url(), 0);
    backend.health().expect("health ok");
    m.assert();
}

#[test]
fn cloud_backend_health_returns_transient_on_5xx() {
    let mut server = mockito::Server::new();
    // All 4 calls (initial + 3 retries) return 503; then we exhaust
    // retries and surface Transient.
    let m = server
        .mock("GET", "/v1/health")
        .with_status(503)
        .expect(4)
        .create();

    let backend = make_backend(&server.url(), 3);
    let err = backend.health().expect_err("5xx must error after retries");
    match err {
        BackendError::Transient(_) => {}
        other => panic!("expected BackendError::Transient, got {other:?}"),
    }
    m.assert();
}

#[test]
fn cloud_backend_delete_204_succeeds() {
    let mut server = mockito::Server::new();
    let k = key(0x50);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    let m = server
        .mock("DELETE", url_path.as_str())
        .with_status(204)
        .create();

    let backend = make_backend(&server.url(), 0);
    backend.delete(&k).expect("delete ok");
    m.assert();
}

#[test]
fn cloud_backend_delete_404_idempotent() {
    let mut server = mockito::Server::new();
    let k = key(0x51);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    let m = server
        .mock("DELETE", url_path.as_str())
        .with_status(404)
        .create();

    let backend = make_backend(&server.url(), 0);
    backend.delete(&k).expect("delete missing must be idempotent");
    m.assert();
}

#[test]
fn cloud_backend_get_errors_on_missing_content_hash_header() {
    let mut server = mockito::Server::new();
    let k = key(0x60);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));
    // 200 OK but no X-Cook-Content-Hash — server misbehaviour.
    let _m = server
        .mock("GET", url_path.as_str())
        .with_status(200)
        .with_body(b"some bytes" as &[u8])
        .create();

    let backend = make_backend(&server.url(), 0);
    match backend.get(&k) {
        Err(BackendError::Other(msg)) => {
            assert!(
                msg.contains("X-Cook-Content-Hash"),
                "diagnostic must mention the missing header; got: {msg}"
            );
        }
        Err(other) => panic!("expected BackendError::Other, got {other:?}"),
        Ok(_) => panic!("expected error, got success"),
    }
}

#[test]
fn put_headers_carry_mode_kind_target() {
    let mut meta = sample_meta();
    meta.mode = 0o755;
    meta.kind = Some("symlink".into());
    meta.target = Some("../sib".into());
    let req = ureq::agent().put("http://x/");
    let req = put_headers(req, "tok", &meta);
    assert_eq!(req.header("X-Cook-Mode"), Some("493")); // 0o755 == 493 decimal
    assert_eq!(req.header("X-Cook-Kind"), Some("symlink"));
    assert_eq!(req.header("X-Cook-Symlink-Target"), Some("../sib"));
}

#[test]
fn get_with_meta_round_trips_mode_kind_target() {
    let mut server = mockito::Server::new();
    let bytes = b"";
    let hash: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
    let k = key(0x70);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));

    let m = server
        .mock("GET", url_path.as_str())
        .with_status(200)
        .with_header("X-Cook-Content-Hash", &hex::encode(hash))
        .with_header("X-Cook-Size-Bytes", "0")
        .with_header("X-Cook-Schema-Version", "3")
        .with_header("X-Cook-Recipe-Namespace", "cook/Cookfile::build")
        .with_header("X-Cook-Output-Index", "0")
        .with_header("X-Cook-Output-Path", "build/foo.o")
        .with_header("X-Cook-Mode", "493")
        .with_header("X-Cook-Kind", "symlink")
        .with_header("X-Cook-Symlink-Target", "../sib")
        .with_body(bytes)
        .create();

    let backend = make_backend(&server.url(), 0);
    let (mut reader, meta) = backend.get_with_meta(&k).expect("get_with_meta ok").expect("present");
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).expect("read body");
    assert_eq!(meta.mode, 0o755);
    assert_eq!(meta.kind.as_deref(), Some("symlink"));
    assert_eq!(meta.target.as_deref(), Some("../sib"));
    assert_eq!(meta.schema_version, 3);
    assert_eq!(meta.output_path, "build/foo.o");
    m.assert();
}

#[test]
fn get_with_meta_defaults_mode_when_header_absent() {
    let mut server = mockito::Server::new();
    let bytes = b"hello";
    let hash: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
    let k = key(0x71);
    let url_path = format!("/v1/artifacts/{}", hex::encode(k));

    let _m = server
        .mock("GET", url_path.as_str())
        .with_status(200)
        .with_header("X-Cook-Content-Hash", &hex::encode(hash))
        .with_body(bytes)
        .create();

    let backend = make_backend(&server.url(), 0);
    let (_reader, meta) = backend.get_with_meta(&k).expect("ok").expect("present");
    assert_eq!(meta.mode, ArtifactMeta::default_mode()); // 0o644
    assert!(meta.kind.is_none());
    assert!(meta.target.is_none());
}

#[test]
fn jitter_factor_in_range() {
    // 50 samples should all fall in [0.75, 1.25].
    for _ in 0..50 {
        let f = jitter_factor();
        assert!(f >= 0.75 && f <= 1.25, "jitter factor out of range: {f}");
        // Tiny sleep to advance the clock for differentiated samples.
        std::thread::sleep(Duration::from_micros(1));
    }
}
