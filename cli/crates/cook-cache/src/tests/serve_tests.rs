use super::*;
use crate::backend::{get_bytes, put_bytes};
use crate::cas_backend::{BackendConfig, BackendError};
use crate::cloud_backend::CloudBackend;
use std::sync::Arc;
use std::time::Duration;

/// A live server on 127.0.0.1:0 over a temp store; shut down and joined on drop.
struct Fixture {
    server: Arc<CacheServer>,
    thread: Option<std::thread::JoinHandle<()>>,
    store: tempfile::TempDir,
}

const TOKEN: &str = "serve-test-token";

impl Fixture {
    fn start() -> Self {
        Self::start_with(BackendConfig::default(), |s| s)
    }

    fn start_with(config: BackendConfig, tune: impl FnOnce(CacheServer) -> CacheServer) -> Self {
        let store = tempfile::tempdir().unwrap();
        let backend = LocalBackend::with_config(store.path().to_path_buf(), config);
        let server = Arc::new(tune(CacheServer::bind("127.0.0.1:0", backend, TOKEN.into()).unwrap()));
        let thread = {
            let server = server.clone();
            Some(std::thread::spawn(move || server.run()))
        };
        Self { server, thread, store }
    }

    fn url(&self) -> String {
        format!("http://{}", self.server.local_addr())
    }

    fn client_with(&self, token: &str) -> CloudBackend {
        let cfg = BackendConfig {
            timeout: Duration::from_secs(10),
            max_retries: 0,
            backoff_initial: Duration::from_millis(1),
            backoff_max: Duration::from_millis(2),
            max_artifact_bytes: 1 << 30,
        };
        CloudBackend::new(self.url(), token.into(), cfg)
    }

    fn client(&self) -> CloudBackend {
        self.client_with(TOKEN)
    }

    fn blob_path(&self, key: &CloudKey) -> std::path::PathBuf {
        let h = hex::encode(key);
        self.store.path().join(&h[..2]).join(&h[2..])
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.shutdown();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn key(byte: u8) -> CloudKey {
    let mut k = [0u8; 32];
    k[0] = byte;
    k[31] = 0x5a;
    k
}

fn meta() -> ArtifactMeta {
    ArtifactMeta {
        recipe_namespace: "cook/Cookfile::build".into(),
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: 3,
        size_bytes: 11,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: 2,
        output_path: "build/link".into(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: Some("symlink".into()),
        mode: 0o755,
        target: Some("../real".into()),
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

#[test]
fn round_trip_through_the_client() {
    let fx = Fixture::start();
    let c = fx.client();
    let k = key(1);
    let bytes = b"hello world".to_vec();
    put_bytes(&c, &k, &bytes, &mut meta()).unwrap();

    assert!(c.batch_query(&[k, key(2)]).unwrap().contains(&k));
    assert!(!c.batch_query(&[k, key(2)]).unwrap().contains(&key(2)));

    let (mut r, got) = c.get_with_meta(&k).unwrap().unwrap();
    let mut body = Vec::new();
    r.read_to_end(&mut body).unwrap();
    assert_eq!(body, bytes);
    let want = meta();
    assert_eq!(got.kind, want.kind);
    assert_eq!(got.mode, 0o755);
    assert_eq!(got.target, want.target);
    assert_eq!(got.output_index, 2);
    assert_eq!(got.output_path, want.output_path);
    assert_eq!(got.recipe_namespace, want.recipe_namespace);
    assert_eq!(got.schema_version, 3);
    assert_eq!(got.size_bytes, 11);
    assert_eq!(got.content_hash, <sha2::Sha256 as sha2::Digest>::digest(&bytes).as_slice());

    c.delete(&k).unwrap();
    assert!(c.get_with_meta(&k).unwrap().is_none());
    c.delete(&k).unwrap();
    c.health().unwrap();
}

#[test]
fn wrong_token_is_unauthorized() {
    let fx = Fixture::start();
    let c = fx.client_with("not-the-token");
    assert!(matches!(c.health(), Err(BackendError::Unauthorized(_))));
    assert!(matches!(c.get_with_meta(&key(1)), Err(BackendError::Unauthorized(_))));
}

#[test]
fn raw_requests_without_auth_or_with_bad_key() {
    let fx = Fixture::start();
    let health = format!("{}/v1/health", fx.url());
    match ureq::get(&health).call() {
        Err(ureq::Error::Status(401, _)) => {}
        other => panic!("expected 401, got {other:?}"),
    }
    let bad = format!("{}/v1/artifacts/{}", fx.url(), "AB".repeat(32));
    match ureq::get(&bad).set("Authorization", &format!("Bearer {TOKEN}")).call() {
        Err(ureq::Error::Status(400, _)) => {}
        other => panic!("expected 400, got {other:?}"),
    }
}

#[test]
fn server_serves_tampered_bytes_and_client_rejects_them() {
    let fx = Fixture::start();
    let c = fx.client();
    let k = key(3);
    let bytes = pattern(4096, 1);
    put_bytes(&c, &k, &bytes, &mut meta()).unwrap();

    let path = fx.blob_path(&k);
    let mut on_disk = std::fs::read(&path).unwrap();
    on_disk[100] ^= 0xff;
    std::fs::write(&path, &on_disk).unwrap();

    let url = format!("{}/v1/artifacts/{}", fx.url(), hex::encode(k));
    let resp = ureq::get(&url).set("Authorization", &format!("Bearer {TOKEN}")).call().unwrap();
    assert_eq!(resp.status(), 200);
    let mut raw = Vec::new();
    resp.into_reader().read_to_end(&mut raw).unwrap();
    assert_eq!(raw, on_disk);

    assert!(get_bytes(&c, &k).unwrap().is_none());
}

#[test]
fn concurrent_conflicting_first_puts_leave_a_verifying_pair() {
    let fx = Fixture::start();
    let k = key(4);
    let variants = [pattern(200 * 1024, 7), pattern(200 * 1024, 9)];
    std::thread::scope(|s| {
        for i in 0..16 {
            let (fx, variants) = (&fx, &variants);
            s.spawn(move || {
                // The loser of a conflict gets an error; only the end state matters.
                let _ = put_bytes(&fx.client(), &k, &variants[i % 2], &mut meta());
            });
        }
    });
    let stored = get_bytes(&fx.client(), &k).unwrap().expect("stored pair must verify");
    assert!(variants.contains(&stored));
}

#[test]
fn multi_megabyte_put_streams_through() {
    let fx = Fixture::start();
    let c = fx.client();
    let k = key(5);
    let bytes = pattern(8 * 1024 * 1024, 3);
    put_bytes(&c, &k, &bytes, &mut meta()).unwrap();
    assert_eq!(get_bytes(&c, &k).unwrap().unwrap(), bytes);
}

#[test]
fn control_bytes_in_a_sidecar_cannot_split_the_response() {
    let fx = Fixture::start();
    let k = key(6);
    put_bytes(&fx.client(), &k, b"hello world", &mut meta()).unwrap();

    let sidecar = fx.blob_path(&k).with_extension("meta.json");
    let mut m: ArtifactMeta = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    m.output_path = "x\r\nX-Injected: yes".into();
    std::fs::write(&sidecar, serde_json::to_vec(&m).unwrap()).unwrap();

    let url = format!("{}/v1/artifacts/{}", fx.url(), hex::encode(k));
    match ureq::get(&url).set("Authorization", &format!("Bearer {TOKEN}")).call() {
        Err(ureq::Error::Status(500, resp)) => assert!(resp.header("X-Injected").is_none()),
        other => panic!("expected 500, got {other:?}"),
    }
}

#[test]
fn empty_token_is_refused() {
    let store = tempfile::tempdir().unwrap();
    let backend = LocalBackend::new(store.path().to_path_buf());
    let err = CacheServer::bind("127.0.0.1:0", backend, String::new()).err().unwrap();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

fn put_url(fx: &Fixture, k: &CloudKey) -> String {
    format!("{}/v1/artifacts/{}", fx.url(), hex::encode(k))
}

#[test]
fn stalled_upload_holds_no_stripe_and_cannot_outlive_the_socket_timeout() {
    use std::io::Write;
    let store = tempfile::tempdir().unwrap();
    let backend = LocalBackend::new(store.path().to_path_buf());
    let server = Arc::new(
        CacheServer::bind_with_io_timeout("127.0.0.1:0", backend, TOKEN.into(), Duration::from_secs(1))
            .unwrap(),
    );
    let (run_tx, run_rx) = std::sync::mpsc::channel();
    {
        let server = server.clone();
        std::thread::spawn(move || {
            server.run();
            let _ = run_tx.send(());
        });
    }

    // Key A (stripe 0) starts an upload it never finishes.
    let a = key(0x00);
    let mut stalled = std::net::TcpStream::connect(server.local_addr()).unwrap();
    let mut head = format!(
        "PUT /v1/artifacts/{} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 100000\r\n",
        hex::encode(a)
    );
    for (n, v) in meta_to_headers(&meta()) {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\nhello");
    stalled.write_all(head.as_bytes()).unwrap();
    std::thread::sleep(Duration::from_millis(300));

    // Key B (0x40) shares A's stripe and must not wait for it.
    let (tx, rx) = std::sync::mpsc::channel();
    let client = CloudBackend::new(
        format!("http://{}", server.local_addr()),
        TOKEN.into(),
        BackendConfig {
            timeout: Duration::from_secs(10),
            max_retries: 0,
            backoff_initial: Duration::from_millis(1),
            backoff_max: Duration::from_millis(2),
            max_artifact_bytes: 1 << 30,
        },
    );
    std::thread::spawn(move || {
        let _ = tx.send(put_bytes(&client, &key(0x40), b"hello world", &mut meta()));
    });
    rx.recv_timeout(Duration::from_secs(3))
        .expect("PUT of a same-stripe key must not wait on the stalled upload")
        .unwrap();

    server.shutdown();
    run_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("run must return once the stalled socket times out");
    drop(stalled);
}

#[test]
fn reput_over_a_corrupted_blob_heals_it() {
    let fx = Fixture::start();
    let c = fx.client();
    let k = key(7);
    let bytes = pattern(4096, 5);
    put_bytes(&c, &k, &bytes, &mut meta()).unwrap();

    let path = fx.blob_path(&k);
    let mut on_disk = std::fs::read(&path).unwrap();
    on_disk[10] ^= 0xff;
    std::fs::write(&path, &on_disk).unwrap();
    assert!(get_bytes(&c, &k).unwrap().is_none());

    put_bytes(&c, &k, &bytes, &mut meta()).unwrap();
    assert_eq!(get_bytes(&c, &k).unwrap().unwrap(), bytes);
}

#[test]
fn store_failure_on_put_is_a_5xx_and_leaves_no_temp() {
    let fx = Fixture::start();
    let k = key(8);
    // A non-empty directory where the blob belongs: the final rename must fail.
    let blob = fx.blob_path(&k);
    std::fs::create_dir_all(&blob).unwrap();
    std::fs::write(blob.join("squatter"), b"x").unwrap();

    let err = put_bytes(&fx.client(), &k, b"hello world", &mut meta()).unwrap_err();
    assert!(matches!(err, BackendError::Transient(_)), "got {err:?}");

    let mut req = ureq::put(&put_url(&fx, &k)).set("Authorization", &format!("Bearer {TOKEN}"));
    for (n, v) in meta_to_headers(&meta()) {
        req = req.set(n, &v);
    }
    match req.send_bytes(b"hello world") {
        Err(ureq::Error::Status(500, _)) => {}
        other => panic!("expected 500, got {other:?}"),
    }
    let leftovers: Vec<_> = std::fs::read_dir(blob.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
}

#[test]
fn conflicting_put_is_a_client_error() {
    let fx = Fixture::start();
    let c = fx.client();
    let k = key(9);
    put_bytes(&c, &k, b"first bytes", &mut meta()).unwrap();
    let err = put_bytes(&c, &k, b"other bytes", &mut meta()).unwrap_err();
    assert!(matches!(err, BackendError::Other(_)), "got {err:?}");
}

#[test]
fn idle_server_keeps_accepting() {
    let store = tempfile::tempdir().unwrap();
    let backend = LocalBackend::new(store.path().to_path_buf());
    let server = Arc::new(
        CacheServer::bind_with_io_timeout("127.0.0.1:0", backend, TOKEN.into(), Duration::from_secs(1))
            .unwrap(),
    );
    let runner = {
        let server = server.clone();
        std::thread::spawn(move || server.run())
    };
    std::thread::sleep(Duration::from_millis(2500));
    let cfg = BackendConfig { max_retries: 0, ..BackendConfig::default() };
    let client = CloudBackend::new(format!("http://{}", server.local_addr()), TOKEN.into(), cfg);
    let health = client.health();
    server.shutdown();
    runner.join().unwrap();
    health.unwrap();
}

/// An authorized PUT head with the `X-Cook-*` headers, then `framing` lines.
fn raw_put_head(k: &CloudKey, framing: &str) -> String {
    let mut head = format!(
        "PUT /v1/artifacts/{} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n{framing}",
        hex::encode(k)
    );
    for (n, v) in meta_to_headers(&meta()) {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    head
}

/// Send `bytes`, half-close, and return the whole response text.
fn raw_exchange(fx: &Fixture, bytes: &[u8]) -> String {
    use std::io::Write;
    let mut s = std::net::TcpStream::connect(fx.server.local_addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(bytes).unwrap();
    s.shutdown(std::net::Shutdown::Write).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

fn assert_refused_and_nothing_stored(fx: &Fixture, k: &CloudKey, request: &str) {
    let response = raw_exchange(fx, request.as_bytes());
    assert!(response.starts_with("HTTP/1.1 400 "), "got {response:?}");
    assert!(fx.client().batch_query(&[*k]).unwrap().is_empty());
    let shard = fx.blob_path(k).parent().unwrap().to_path_buf();
    let temps = std::fs::read_dir(shard).into_iter().flatten().filter(|e| {
        e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp")
    });
    assert_eq!(temps.count(), 0);
}

#[test]
fn short_content_length_body_is_refused_and_leaves_the_key_free() {
    let fx = Fixture::start();
    let k = key(0x0a);
    let request = raw_put_head(&k, "Content-Length: 100000\r\n") + "hello";
    assert_refused_and_nothing_stored(&fx, &k, &request);

    put_bytes(&fx.client(), &k, b"hello world", &mut meta()).unwrap();
    assert_eq!(get_bytes(&fx.client(), &k).unwrap().unwrap(), b"hello world");
}

#[test]
fn truncated_chunked_bodies_store_nothing() {
    let fx = Fixture::start();
    let chunked = "Transfer-Encoding: chunked\r\n";
    // Cut inside a chunk, and cut after a whole chunk with no terminator.
    let k = key(0x0b);
    assert_refused_and_nothing_stored(&fx, &k, &(raw_put_head(&k, chunked) + "a\r\nhello"));
    let k = key(0x0c);
    assert_refused_and_nothing_stored(&fx, &k, &(raw_put_head(&k, chunked) + "5\r\nhello\r\n"));

    let k = key(0x0d);
    let whole = raw_put_head(&k, chunked) + "5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
    let r = raw_exchange(&fx, whole.as_bytes());
    assert!(r.starts_with("HTTP/1.1 200 "), "{r}");
    assert_eq!(get_bytes(&fx.client(), &k).unwrap().unwrap(), b"hello world");
}

#[test]
fn ambiguous_body_framing_is_refused() {
    let fx = Fixture::start();
    let k = key(0x0e);
    let both = "Content-Length: 5\r\nTransfer-Encoding: chunked\r\n";
    assert_refused_and_nothing_stored(&fx, &k, &(raw_put_head(&k, both) + "5\r\nhello\r\n0\r\n\r\n"));
    assert_refused_and_nothing_stored(&fx, &k, &(raw_put_head(&k, "Transfer-Encoding: gzip\r\n") + "hello"));
    assert_refused_and_nothing_stored(&fx, &k, &(raw_put_head(&k, "Content-Length: +5\r\n") + "hello"));
}

#[test]
fn refused_large_put_reports_unauthorized_not_a_transport_error() {
    let fx = Fixture::start();
    let bytes = pattern(8 * 1024 * 1024, 11);
    let err = put_bytes(&fx.client_with("not-the-token"), &key(0x0f), &bytes, &mut meta()).unwrap_err();
    assert!(matches!(err, BackendError::Unauthorized(_)), "got {err:?}");
}

/// Connect and send nothing: a slot held by an idle peer.
fn idle(fx: &Fixture) -> std::net::TcpStream {
    let s = std::net::TcpStream::connect(fx.server.local_addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}

/// Retry `health` until it succeeds or `within` passes.
fn healthy_within(fx: &Fixture, within: Duration) -> Result<(), BackendError> {
    let deadline = std::time::Instant::now() + within;
    loop {
        match fx.client().health() {
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            r => return r,
        }
    }
}

#[test]
fn connections_past_the_cap_get_503_and_slots_free_when_peers_leave() {
    let fx = Fixture::start_with(BackendConfig::default(), |s| s.with_max_connections(2.try_into().unwrap()));
    let held = [idle(&fx), idle(&fx)];
    let mut extra = idle(&fx);
    let mut response = String::new();
    extra.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 503 "), "got {response:?}");
    assert!(response.contains("Connection: close"), "got {response:?}");

    drop(held);
    healthy_within(&fx, Duration::from_secs(5)).expect("a slot frees once its idle peer leaves");
}

#[test]
fn a_failed_thread_spawn_drops_one_connection_and_the_server_goes_on() {
    let fx = Fixture::start_with(BackendConfig::default(), |s| s.with_max_connections(1.try_into().unwrap()));
    fx.server.fail_spawns.store(1, Ordering::SeqCst);
    let mut dropped = idle(&fx);
    dropped.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut response = String::new();
    // A timeout is an Err, so only a prompt EOF passes.
    dropped.read_to_string(&mut response).unwrap();
    assert!(response.is_empty(), "got {response:?}");
    // The cap is 1, so this also proves the failed spawn gave its slot back.
    fx.client().health().expect("the server still serves");
}

#[test]
fn too_many_header_fields_is_431() {
    let fx = Fixture::start();
    let mut request = String::from("GET /v1/health HTTP/1.1\r\nHost: x\r\n");
    for i in 0..65 {
        request.push_str(&format!("X-Pad-{i}: y\r\n"));
    }
    request.push_str("\r\n");
    let response = raw_exchange(&fx, request.as_bytes());
    assert!(response.starts_with("HTTP/1.1 431 "), "got {response:?}");
}

#[test]
fn a_declared_oversize_put_is_413_before_100_continue() {
    let config = BackendConfig { max_artifact_bytes: 1024 * 1024, ..BackendConfig::default() };
    let fx = Fixture::start_with(config, |s| s);
    let k = key(0x10);
    let head = raw_put_head(&k, "Content-Length: 2097152\r\nExpect: 100-continue\r\n");
    // No body is sent, and the write side stays open: a server that waited
    // for the body would time out rather than answer.
    use std::io::Write;
    let mut s = idle(&fx);
    s.write_all(head.as_bytes()).unwrap();
    let mut response = String::new();
    s.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 413 "), "got {response:?}");
    assert!(!response.contains("100 Continue"), "got {response:?}");
    assert!(fx.client().batch_query(&[k]).unwrap().is_empty());
}

#[test]
fn malformed_framing_on_a_put_is_400_not_413() {
    let config = BackendConfig { max_artifact_bytes: 16, ..BackendConfig::default() };
    let fx = Fixture::start_with(config, |s| s);
    let k = key(0x11);
    // Each declares more than the cap, so only refusing the framing first gives 400.
    for framing in [
        "Content-Length: +999999\r\n",
        "Content-Length: 999999\r\nContent-Length: 5\r\n",
        "Content-Length: 999999\r\nTransfer-Encoding: chunked\r\n",
    ] {
        let response = raw_exchange(&fx, raw_put_head(&k, framing).as_bytes());
        assert!(response.starts_with("HTTP/1.1 400 "), "{framing:?}: got {response:?}");
    }
}
