//! End-to-end tests for `cook cache serve`, driving the real `cook` binary.
//! The store, the cache home and the listen port (`:0`) are all the test's own.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cook_engine::cook_cache::cloud_backend::API_KEY_ENV;
use cook_engine::cook_cache::{
    ArtifactMeta, BackendConfig, BackendError, CacheBackend, CloudBackend,
};
use tempfile::TempDir;

const TOKEN: &str = "s3cret";

fn cook(tmp: &TempDir) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cook"));
    c.env("XDG_CACHE_HOME", tmp.path().join("xdg"))
        .current_dir(tmp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    c
}

/// Kills the child if a test panics before it has been waited on.
struct Served(Child);
impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(tmp: &TempDir) -> (Served, String, std::thread::JoinHandle<String>) {
    start_with(tmp, &[])
}

fn start_with(tmp: &TempDir, extra: &[&str]) -> (Served, String, std::thread::JoinHandle<String>) {
    if !extra.contains(&"--init") {
        std::fs::create_dir_all(tmp.path().join("store")).unwrap();
    }
    let mut child = cook(tmp)
        .args(["cache", "serve", "--listen", "127.0.0.1:0", "--store"])
        .arg(tmp.path().join("store"))
        .args(extra)
        .env(API_KEY_ENV, TOKEN)
        .spawn()
        .expect("spawn cook");
    let mut err = BufReader::new(child.stderr.take().unwrap());
    let mut first = String::new();
    err.read_line(&mut first).unwrap();
    let addr = first
        .trim()
        .strip_prefix("listening on ")
        .unwrap_or_else(|| panic!("first stderr line was {first:?}"))
        .to_string();
    // Keep the pipe drained so the server never blocks on stderr; the rest is the request log.
    let log = std::thread::spawn(move || {
        let mut rest = String::new();
        let _ = err.read_to_string(&mut rest);
        rest
    });
    (Served(child), addr, log)
}

fn meta() -> ArtifactMeta {
    ArtifactMeta {
        recipe_namespace: "cook/Cookfile::build".into(),
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: 3,
        size_bytes: 0,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: 0,
        output_path: "build/foo.o".into(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: None,
        mode: ArtifactMeta::default_mode(),
        target: None,
    }
}

fn client(addr: &str, token: &str) -> CloudBackend {
    let config = BackendConfig { max_retries: 0, ..BackendConfig::default() };
    CloudBackend::new(format!("http://{addr}"), token.into(), config)
}

fn wait_exit(child: &mut Child, within: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        assert!(Instant::now() < deadline, "cook cache serve did not exit in time");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn serves_put_and_get_checks_the_token_and_stops_cleanly_on_sigterm() {
    let tmp = TempDir::new().unwrap();
    let (mut served, addr, log) = start(&tmp);

    let key = [7u8; 32];
    let bytes = b"hello shared cache".to_vec();
    let good = client(&addr, TOKEN);
    good.put(&key, &mut bytes.as_slice(), &mut meta()).expect("put");
    let (mut r, _) = good.get_with_meta(&key).expect("get").expect("hit");
    let mut got = Vec::new();
    r.read_to_end(&mut got).expect("bytes verify against the content hash");
    assert_eq!(got, bytes);

    let bad = client(&addr, "wrong");
    assert!(matches!(bad.get(&key), Err(BackendError::Unauthorized(_))));

    assert!(good.batch_query(&[key]).unwrap().contains(&key));
    good.health().expect("health");
    // Same key, different bytes: refused, not overwritten.
    let conflict = good.put(&key, &mut b"other bytes".as_slice(), &mut meta());
    assert!(matches!(conflict, Err(BackendError::Other(_))), "conflicting put: {conflict:?}");
    good.delete(&key).expect("delete");
    good.delete(&key).expect("deleting a missing key succeeds");
    assert!(good.get(&key).expect("get after delete").is_none());

    let st = Command::new("kill").arg("-TERM").arg(served.0.id().to_string()).status().unwrap();
    assert!(st.success());
    let status = wait_exit(&mut served.0, Duration::from_secs(5));
    assert!(status.success(), "SIGTERM should exit 0, got {status:?}");

    // One line per request: put, get, bad get, query, health, conflicting put, 2 deletes, get.
    let log = log.join().unwrap();
    assert_eq!(log.lines().count(), 9, "one log line per request:\n{log}");
    assert!(!log.contains(TOKEN), "the token never reaches the log:\n{log}");
}

#[test]
fn missing_token_refuses_to_start_and_names_the_variable() {
    let tmp = TempDir::new().unwrap();
    let out = cook(&tmp)
        .args(["cache", "serve", "--listen", "127.0.0.1:0", "--store"])
        .arg(tmp.path().join("store"))
        .env_remove(API_KEY_ENV)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(API_KEY_ENV), "stderr should name {API_KEY_ENV}: {err}");
    assert!(!tmp.path().join("store").exists(), "nothing is created before the token check");
}

#[test]
fn store_is_required() {
    let tmp = TempDir::new().unwrap();
    let out = cook(&tmp).args(["cache", "serve"]).env(API_KEY_ENV, TOKEN).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--store"), "usage error should name --store: {err}");
}

#[test]
fn a_missing_store_is_refused_without_init_and_created_with_it() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let mut refused = Served(
        cook(&tmp)
            .args(["cache", "serve", "--listen", "127.0.0.1:0", "--store"])
            .arg(&store)
            .env(API_KEY_ENV, TOKEN)
            .spawn()
            .unwrap(),
    );
    // Bounded: a regression that starts serving fails here instead of hanging.
    let status = wait_exit(&mut refused.0, Duration::from_secs(20));
    assert!(!status.success());
    let mut err = String::new();
    refused.0.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(err.contains(store.to_str().unwrap()) && err.contains("--init"), "{err}");
    assert!(!err.contains("listening on"), "{err}");
    assert!(!store.exists(), "a refused start creates nothing");

    let file = tmp.path().join("afile");
    std::fs::write(&file, b"x").unwrap();
    let out = cook(&tmp)
        .args(["cache", "serve", "--listen", "127.0.0.1:0", "--init", "--store"])
        .arg(&file)
        .env(API_KEY_ENV, TOKEN)
        .output()
        .unwrap();
    assert!(!out.status.success(), "a file is not a store even with --init");

    let (mut served, addr, _log) = start_with(&tmp, &["--init"]);
    assert!(store.is_dir());
    let c = client(&addr, TOKEN);
    let key = [3u8; 32];
    c.put(&key, &mut b"round trip".as_slice(), &mut meta()).expect("put");
    assert!(c.get(&key).expect("get").is_some());
    Command::new("kill").arg("-TERM").arg(served.0.id().to_string()).status().unwrap();
    assert!(wait_exit(&mut served.0, Duration::from_secs(5)).success());
}

#[test]
fn max_artifact_mib_bounds_what_the_server_accepts() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start_with(&tmp, &["--max-artifact-mib", "1"]);
    let c = client(&addr, TOKEN);
    let big = vec![1u8; 2 * 1024 * 1024];
    let r = c.put(&[1u8; 32], &mut big.as_slice(), &mut meta());
    assert!(matches!(r, Err(BackendError::Other(_))), "2 MiB over a 1 MiB cap: {r:?}");
    let small = vec![2u8; 512 * 1024];
    c.put(&[2u8; 32], &mut small.as_slice(), &mut meta()).expect("512 KiB fits");
}

#[test]
fn max_connections_caps_open_connections_and_refuses_zero() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("store")).unwrap();
    let out = cook(&tmp)
        .args(["cache", "serve", "--max-connections", "0", "--store"])
        .arg(tmp.path().join("store"))
        .env(API_KEY_ENV, TOKEN)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--max-connections"), "usage error should name the flag: {err}");

    let (_served, addr, _log) = start_with(&tmp, &["--max-connections", "1"]);
    let _held = std::net::TcpStream::connect(&addr).unwrap();
    let mut extra = std::net::TcpStream::connect(&addr).unwrap();
    extra.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut response = String::new();
    extra.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 503 "), "got {response:?}");
}
