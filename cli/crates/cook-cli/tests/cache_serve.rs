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

// ── Manifests over the wire: two checkouts, one server ───────────────────────
//
// Each checkout run has its own XDG_CACHE_HOME and B has never run, so a hit in
// B can only come from the server.

const TEST_UNIT: &str = "recipe r\n    gather \"src/in.txt\"\n    test { cat $<in> >/dev/null; echo ran >> t.runlog }\n";

/// A producing unit with a depfile (discovered input) and an empty directory output.
const DEPFILE_DIR_UNIT: &str = r#"recipe build
    cook.add_unit({
        name    = "build-step",
        inputs  = { "src/in.txt" },
        outputs = { "out/" },
        command = "mkdir -p out/empty .cook/deps && cp src/in.txt out/a.txt && echo 'out/a.txt: src/in.txt' > .cook/deps/build.d && echo ran >> build.runlog",
        discovered_inputs = { from = ".cook/deps/build.d", format = "make" },
    })
"#;

/// A checkout at `name` under `tmp`, talking to the server at `addr`.
fn checkout(tmp: &TempDir, name: &str, addr: &str, cookfile: &str, publish: bool) -> std::path::PathBuf {
    let wd = tmp.path().join(name);
    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::create_dir_all(wd.join(".cook")).unwrap();
    std::fs::write(wd.join("src/in.txt"), "src-content\n").unwrap();
    std::fs::write(wd.join("Cookfile"), cookfile).unwrap();
    std::fs::write(
        wd.join(".cook/cloud.toml"),
        format!(
            "[cloud]\nenabled = true\nproject = \"manifests\"\nendpoint = \"http://{addr}\"\npublish = {publish}\n"
        ),
    )
    .unwrap();
    wd
}

fn run_in(tmp: &TempDir, wd: &std::path::Path, args: &[&str]) -> std::process::Output {
    let out = Command::new(env!("CARGO_BIN_EXE_cook"))
        .args(args)
        .current_dir(wd)
        .env("XDG_CACHE_HOME", tmp.path().join(format!("xdg-{}", wd.file_name().unwrap().to_string_lossy())))
        .env(API_KEY_ENV, TOKEN)
        .output()
        .expect("cook invocation");
    assert!(
        out.status.success(),
        "cook {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn runs(wd: &std::path::Path, log: &str) -> usize {
    std::fs::read_to_string(wd.join(log)).map_or(0, |s| s.lines().count())
}

fn manifests_in(store: &std::path::Path) -> Vec<std::path::PathBuf> {
    walkdir::WalkDir::new(store)
        .into_iter()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".provenance.json"))
        .map(|e| e.path().to_path_buf())
        .collect()
}

#[test]
fn a_test_unit_built_in_one_checkout_is_a_shared_hit_in_another() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let a = checkout(&tmp, "a", &addr, TEST_UNIT, true);
    run_in(&tmp, &a, &["test"]);
    assert_eq!(runs(&a, "t.runlog"), 1);

    let b = checkout(&tmp, "b", &addr, TEST_UNIT, true);
    let out = run_in(&tmp, &b, &["test"]);
    assert_eq!(runs(&b, "t.runlog"), 0, "B's body must not run");
    assert!(String::from_utf8_lossy(&out.stdout).contains("cached"));
}

#[test]
fn a_shared_fetch_restores_the_depfile_and_the_empty_directory() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let a = checkout(&tmp, "a", &addr, DEPFILE_DIR_UNIT, true);
    run_in(&tmp, &a, &["build"]);
    assert_eq!(runs(&a, "build.runlog"), 1);
    assert!(a.join("out/empty").is_dir());

    let b = checkout(&tmp, "b", &addr, DEPFILE_DIR_UNIT, true);
    run_in(&tmp, &b, &["build"]);
    assert_eq!(runs(&b, "build.runlog"), 0, "B restores, never runs");
    assert!(b.join("out/a.txt").is_file());
    assert!(b.join("out/empty").is_dir(), "the empty directory output is restored");
    assert!(b.join(".cook/deps/build.d").is_file(), "the depfile is restored");
}

#[test]
fn why_diffs_determinants_against_a_manifest_fetched_from_the_server() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let cookfile = "recipe build\n    cook.add_unit({\n        name = \"build-step\",\n        inputs = {\"src/in.txt\"},\n        outputs = {\"out.txt\"},\n        command = \"cp src/in.txt out.txt\",\n    })\n";
    let a = checkout(&tmp, "a", &addr, cookfile, true);
    run_in(&tmp, &a, &["build"]);

    // Keep only the manifest, and make it record a different input hash.
    let store = tmp.path().join("store");
    let manifest = manifests_in(&store).into_iter().next().expect("A published a manifest");
    for e in walkdir::WalkDir::new(&store).into_iter().flatten().filter(|e| e.file_type().is_file()) {
        if e.path() != manifest {
            std::fs::remove_file(e.path()).unwrap();
        }
    }
    let mut json: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    json["inputs"]["src/in.txt"] = "deadbeefdeadbeef".into();
    std::fs::write(&manifest, serde_json::to_vec(&json).unwrap()).unwrap();

    let b = checkout(&tmp, "b", &addr, cookfile, true);
    let out = run_in(&tmp, &b, &["why", "build", "--level", "unit"]);
    let out = String::from_utf8_lossy(&out.stdout);
    assert!(out.contains("shared-miss diff vs producer manifest:"), "got:\n{out}");
    assert!(!out.contains("no producer manifest published for this key"), "got:\n{out}");
    assert!(out.contains("input src/in.txt: ours Some(") && out.contains(&format!("producer Some({})", 0xdead_beef_dead_beef_u64)), "got:\n{out}");
}

#[test]
fn a_local_hit_backfills_the_manifest_so_another_checkout_gets_the_verdict() {
    let tmp = TempDir::new().unwrap();
    let (mut served, addr, log) = start(&tmp);
    let a = checkout(&tmp, "a", &addr, TEST_UNIT, false);
    run_in(&tmp, &a, &["test"]);
    assert!(manifests_in(&tmp.path().join("store")).is_empty(), "publish-off run publishes nothing");

    // Publishing on: the local hit backfills the entry, manifest included.
    std::fs::write(
        a.join(".cook/cloud.toml"),
        format!("[cloud]\nenabled = true\nproject = \"manifests\"\nendpoint = \"http://{addr}\"\n"),
    )
    .unwrap();
    run_in(&tmp, &a, &["test"]);
    assert_eq!(runs(&a, "t.runlog"), 1, "second run is a local hit");
    assert!(!manifests_in(&tmp.path().join("store")).is_empty(), "the backfill stores the manifest");

    let b = checkout(&tmp, "b", &addr, TEST_UNIT, true);
    run_in(&tmp, &b, &["test"]);
    assert_eq!(runs(&b, "t.runlog"), 0, "B gets the verdict as a shared hit");

    served.0.kill().unwrap();
    wait_exit(&mut served.0, Duration::from_secs(5));
    let log = log.join().unwrap();
    assert!(log.contains("PUT /v1/manifests/"), "a manifest PUT reached the server:\n{log}");
}

/// A producing unit whose declared output is a plain file.
const CP_UNIT: &str = "recipe build\n    cook.add_unit({\n        name = \"build-step\",\n        inputs = {\"src/in.txt\"},\n        outputs = {\"out.txt\"},\n        command = \"cp src/in.txt out.txt && echo ran >> build.runlog\",\n    })\n";

/// Age every manifest in `store` by two days, then sweep with the real binary beside the live server.
fn gc_aged_manifests(tmp: &TempDir, store: &std::path::Path) {
    let old = std::time::SystemTime::now() - Duration::from_secs(2 * 86_400);
    for m in manifests_in(store) {
        std::fs::File::open(&m).unwrap().set_times(std::fs::FileTimes::new().set_modified(old)).unwrap();
    }
    let blobs = |s: &std::path::Path| {
        walkdir::WalkDir::new(s)
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file() && !e.file_name().to_string_lossy().contains('.') && e.file_name() != "lock")
            .count()
    };
    let before = blobs(store);
    let out = cook(tmp).args(["cache", "gc", "--older-than", "1d", "--store"]).arg(store).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(manifests_in(store).is_empty(), "gc evicts the aged manifests");
    assert_eq!(blobs(store), before, "only the manifests went; the artifact blobs remain");
    assert!(before > 0, "the producer's output blob is in the store");
}

#[test]
fn an_evicted_manifest_makes_the_test_unit_miss_never_pass() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let a = checkout(&tmp, "a", &addr, TEST_UNIT, true);
    run_in(&tmp, &a, &["test"]);
    assert!(!manifests_in(&tmp.path().join("store")).is_empty(), "A published a manifest");
    gc_aged_manifests(&tmp, &tmp.path().join("store"));

    let b = checkout(&tmp, "b", &addr, TEST_UNIT, true);
    run_in(&tmp, &b, &["test"]);
    assert_eq!(runs(&b, "t.runlog"), 1, "without the verdict B runs the body: a miss, not a replayed pass");
}

#[test]
fn an_evicted_manifest_leaves_a_producer_its_declared_outputs() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let a = checkout(&tmp, "a", &addr, CP_UNIT, true);
    run_in(&tmp, &a, &["build"]);
    gc_aged_manifests(&tmp, &tmp.path().join("store"));

    let b = checkout(&tmp, "b", &addr, CP_UNIT, true);
    run_in(&tmp, &b, &["build"]);
    assert_eq!(std::fs::read_to_string(b.join("out.txt")).unwrap(), "src-content\n");
    assert_eq!(runs(&b, "build.runlog"), 0, "B restores the declared outputs, never runs the body");
}

/// A glob-output unit: `dist/b.js` exists only while the input says v1.
const GLOB_UNIT: &str = "recipe build\n    cook.add_unit({\n        name = \"build-step\",\n        inputs = {\"src/in.txt\"},\n        outputs = {\"dist/*.js\"},\n        command = \"rm -rf dist && mkdir -p dist && echo a > dist/a.js && if grep -q v1 src/in.txt; then echo b > dist/b.js; fi && echo ran >> build.runlog\",\n    })\n";

#[test]
fn an_evicted_manifest_makes_a_reverted_glob_unit_miss_never_restore_the_last_runs_names() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let a = checkout(&tmp, "a", &addr, GLOB_UNIT, true);
    std::fs::write(a.join("src/in.txt"), "v1\n").unwrap();
    run_in(&tmp, &a, &["build"]);
    std::fs::write(a.join("src/in.txt"), "v2\n").unwrap();
    run_in(&tmp, &a, &["build"]); // the local entry now lists only dist/a.js
    assert!(!a.join("dist/b.js").exists());
    gc_aged_manifests(&tmp, &tmp.path().join("store"));

    std::fs::write(a.join("src/in.txt"), "v1\n").unwrap();
    run_in(&tmp, &a, &["build"]);
    assert!(a.join("dist/b.js").is_file(), "the revert must not be served from the last run's names");
    assert_eq!(runs(&a, "build.runlog"), 3, "without the manifest the glob unit misses and rebuilds");
}

// ── Links in a directory output ──────────────────────────────────────────────

/// A unit owning the directory `out/`.
const DIR_UNIT: &str = r#"recipe build
    cook.add_unit({
        name    = "build-step",
        inputs  = { "src/in.txt" },
        outputs = { "out/" },
        command = "mkdir -p out && cp src/in.txt out/a.txt && echo ran >> build.runlog",
    })
"#;

/// What a link into the rest of the workspace would hand over: a secret, git's
/// history, the sources.
fn plant_workspace(wd: &std::path::Path) {
    std::fs::write(wd.join(".env"), "SECRET=1\n").unwrap();
    std::fs::create_dir_all(wd.join(".git/objects")).unwrap();
    std::fs::write(wd.join(".git/objects/pack"), "history").unwrap();
    std::fs::write(wd.join("src/main.c"), "mine").unwrap();
}

fn workspace_intact(wd: &std::path::Path) -> bool {
    [".env", ".git/objects/pack", "src/main.c", "src/in.txt", "Cookfile"].iter().all(|p| wd.join(p).is_file())
}

/// Whether any file in the store names `needle` (an artifact's sidecar, a manifest).
fn store_mentions(store: &std::path::Path, needle: &str) -> bool {
    walkdir::WalkDir::new(store)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .any(|e| String::from_utf8_lossy(&std::fs::read(e.path()).unwrap()).contains(needle))
}

/// A token holder adds `out/h -> ..` to a published key: an artifact and a
/// manifest naming it beside the real output, which `out/` covers. The link
/// is not restored — a fetch, the plain hits after it and the rebuild's
/// pre-clean all walk `out/`, and through the link they would delete the
/// workspace and record and upload what it holds.
#[test]
fn a_forged_link_in_a_directory_output_never_reaches_the_workspace() {
    let tmp = TempDir::new().unwrap();
    let (mut served, addr, log) = start(&tmp);
    let store = tmp.path().join("store");
    let a = checkout(&tmp, "a", &addr, DIR_UNIT, true);
    run_in(&tmp, &a, &["build"]);

    let path = manifests_in(&store).into_iter().next().expect("A published a manifest");
    let mut m: cook_engine::cook_cache::DeterminantManifest = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let key: [u8; 32] = std::array::from_fn(|i| u8::from_str_radix(&m.key[2 * i..2 * i + 2], 16).unwrap());
    assert_eq!(m.output_paths, ["out/a.txt"]);
    m.output_paths.push("out/h".into());
    let forger = client(&addr, TOKEN);
    let mut link = ArtifactMeta {
        recipe_namespace: m.recipe_namespace.clone(),
        output_index: 1,
        output_path: "out/h".into(),
        kind: Some("symlink".into()),
        target: Some("..".into()),
        ..meta()
    };
    let link_key = cook_engine::cook_cache::artifact_key(&key, 1, "out/h");
    forger.put(&link_key, &mut std::io::empty(), &mut link).unwrap();
    forger.put_manifest(&key, &m).unwrap();

    let b = checkout(&tmp, "b", &addr, DIR_UNIT, true);
    plant_workspace(&b);
    run_in(&tmp, &b, &["build"]); // the fetch
    assert!(std::fs::symlink_metadata(b.join("out/h")).is_err(), "the forged link was restored");
    assert!(workspace_intact(&b), "after the fetch");
    run_in(&tmp, &b, &["build"]); // a plain hit
    assert!(workspace_intact(&b), "after a hit");
    std::fs::write(b.join("src/in.txt"), "changed\n").unwrap();
    run_in(&tmp, &b, &["build"]); // pre-clean, then a rebuild
    assert!(workspace_intact(&b), "after a rebuild");
    assert_eq!(std::fs::read_to_string(b.join("out/a.txt")).unwrap(), "changed\n");
    assert!(!store_mentions(&store, "SECRET") && !store_mentions(&store, "out/h/"), "B uploaded what a link reaches");

    served.0.kill().unwrap();
    wait_exit(&mut served.0, Duration::from_secs(5));
    let _ = log.join();
}

/// An honest build that links its assets in (`dist/assets -> ../public`):
/// cook neither records nor uploads the files the link reaches, and the
/// pre-clean before the next build removes the link, not `public/`.
#[test]
fn an_honest_link_out_of_a_directory_output_is_not_followed() {
    let unit = r#"recipe build
    cook.add_unit({
        name    = "build-step",
        inputs  = { "src/in.txt" },
        outputs = { "dist/" },
        command = "mkdir -p dist && cp src/in.txt dist/app.js && ln -sfn ../public dist/assets && echo ran >> build.runlog",
    })
"#;
    let tmp = TempDir::new().unwrap();
    let (mut served, addr, log) = start(&tmp);
    let a = checkout(&tmp, "a", &addr, unit, true);
    plant_workspace(&a);
    std::fs::create_dir_all(a.join("public")).unwrap();
    std::fs::write(a.join("public/logo.png"), "logo-bytes").unwrap();
    run_in(&tmp, &a, &["build"]);
    std::fs::write(a.join("src/in.txt"), "changed\n").unwrap();
    run_in(&tmp, &a, &["build"]);
    assert_eq!(runs(&a, "build.runlog"), 2);
    // A directory link in an output fails the record, so the unit rebuilds each run and publishes nothing.
    run_in(&tmp, &a, &["build"]);
    assert_eq!(runs(&a, "build.runlog"), 3);
    assert!(manifests_in(&tmp.path().join("store")).is_empty());
    assert_eq!(std::fs::read_to_string(a.join("public/logo.png")).unwrap(), "logo-bytes", "removed through the link");
    assert!(workspace_intact(&a));
    assert_eq!(std::fs::read_link(a.join("dist/assets")).unwrap(), std::path::Path::new("../public"));
    let store = tmp.path().join("store");
    assert!(!store_mentions(&store, "dist/assets/") && !store_mentions(&store, "logo-bytes"), "uploaded what the link reaches");

    served.0.kill().unwrap();
    wait_exit(&mut served.0, Duration::from_secs(5));
    let _ = log.join();
}

/// A token holder's put can claim any kind, so a served store sizes every entry
/// alike: each kind the project store spares, and a manifest beside one, is
/// reclaimed by `--max-size` beside the live server.
#[test]
fn puts_claiming_every_exempt_kind_are_reclaimed_by_a_store_size_sweep() {
    use cook_engine::cook_cache::SIZE_SWEEP_EXEMPT_KINDS;
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let store = tmp.path().join("store");
    let good = client(&addr, TOKEN);
    let keys: Vec<[u8; 32]> = (1..=SIZE_SWEEP_EXEMPT_KINDS.len() as u8).map(|b| [b; 32]).collect();
    for (key, kind) in keys.iter().zip(SIZE_SWEEP_EXEMPT_KINDS) {
        let mut m = meta();
        m.kind = Some(kind.to_string());
        good.put(key, &mut vec![b'x'; 100_000].as_slice(), &mut m).unwrap_or_else(|e| panic!("put {kind}: {e:?}"));
    }
    let hex: String = keys[0].iter().map(|b| format!("{b:02x}")).collect();
    let manifest = store.join(&hex[..2]).join(format!("{}.provenance.json", &hex[2..]));
    std::fs::write(&manifest, vec![b'm'; 100_000]).unwrap();

    let out = cook(&tmp).args(["cache", "gc", "--max-size", "1", "--store"]).arg(&store).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    for (key, kind) in keys.iter().zip(SIZE_SWEEP_EXEMPT_KINDS) {
        assert!(good.get(key).unwrap().is_none(), "a put claiming {kind} survived --max-size");
    }
    assert!(!manifest.exists(), "the manifest beside an exempt-kind blob survived --max-size");
}

// ── Warnings reach stderr ────────────────────────────────────────────────────

/// A unit whose one output is 2 MiB, over a `--max-artifact-mib 1` cap.
const BIG_OUTPUT_UNIT: &str = "recipe build\n    cook.add_unit({\n        name = \"build-step\",\n        inputs = {\"src/in.txt\"},\n        outputs = {\"out.bin\"},\n        command = \"head -c 2097152 /dev/zero > out.bin\",\n    })\n";

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn lines_containing<'a>(text: &'a str, needle: &str) -> Vec<&'a str> {
    text.lines().filter(|l| l.contains(needle)).collect()
}

/// A checkout whose store is an endpoint nobody listens on, failing fast.
fn dead_store_checkout(tmp: &TempDir, cookfile: &str) -> std::path::PathBuf {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let wd = checkout(tmp, "dead", &format!("127.0.0.1:{port}"), cookfile, true);
    let mut cfg = std::fs::read_to_string(wd.join(".cook/cloud.toml")).unwrap();
    cfg.push_str("timeout_secs = 1\nmax_retries = 0\n");
    std::fs::write(wd.join(".cook/cloud.toml"), cfg).unwrap();
    wd
}

#[test]
fn a_refused_put_prints_one_line_naming_the_unit_and_never_the_token() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start_with(&tmp, &["--max-artifact-mib", "1"]);
    let wd = checkout(&tmp, "a", &addr, BIG_OUTPUT_UNIT, true);
    let err = stderr_of(&run_in(&tmp, &wd, &["build"]));
    let failed = lines_containing(&err, "cache upload for");
    assert_eq!(failed.len(), 1, "one line for the one unit:\n{err}");
    assert!(
        failed[0].starts_with("cook: warning: cache upload for head -c 2097152 /dev/zero > out.bin failed: "),
        "names the unit (as the progress line does): {}",
        failed[0]
    );
    assert!(!err.contains(TOKEN), "the token never reaches stderr:\n{err}");
}

#[test]
fn an_unreachable_store_warns_once_per_failure_not_once_per_put() {
    let tmp = TempDir::new().unwrap();
    let wd = dead_store_checkout(&tmp, DEPFILE_DIR_UNIT);
    let err = stderr_of(&run_in(&tmp, &wd, &["build"]));
    // One `cook build` builds two cache contexts; the health line is told once.
    assert_eq!(
        lines_containing(&err, "cook: warning: cache store unavailable").len(),
        1,
        "the health failure is visible, once:\n{err}"
    );
    // The unit has an output, a depfile, two manifests and an observation to put; only the first failure is told.
    assert_eq!(lines_containing(&err, "cache upload for").len(), 1, "one line for the one unit:\n{err}");
    assert_eq!(lines_containing(&err, "cache backend put failed").len(), 0, "{err}");
    assert!(!err.contains(TOKEN), "{err}");
}

/// Three units to publish, each with one output.
const THREE_UNITS: &str = "recipe build\n    cook.add_unit({\n        name = \"unit-a\",\n        inputs = {\"src/in.txt\"},\n        outputs = {\"a.txt\"},\n        command = \"cp src/in.txt a.txt\",\n    })\n    cook.add_unit({\n        name = \"unit-b\",\n        inputs = {\"src/in.txt\"},\n        outputs = {\"b.txt\"},\n        command = \"cp src/in.txt b.txt\",\n    })\n    cook.add_unit({\n        name = \"unit-c\",\n        inputs = {\"src/in.txt\"},\n        outputs = {\"c.txt\"},\n        command = \"cp src/in.txt c.txt\",\n    })\n";

#[test]
fn a_dead_store_names_the_first_unit_and_counts_the_rest() {
    let tmp = TempDir::new().unwrap();
    let wd = dead_store_checkout(&tmp, THREE_UNITS);
    let err = stderr_of(&run_in(&tmp, &wd, &["build"]));
    let first = lines_containing(&err, "cache upload for");
    assert_eq!(first.len(), 1, "the first failure is told in full, once:\n{err}");
    assert!(first[0].starts_with("cook: warning: cache upload for cp src/in.txt "), "{err}");
    let more = lines_containing(&err, "cache upload failed for");
    assert_eq!(more.len(), 1, "one summary line:\n{err}");
    assert_eq!(more[0], "cook: warning: cache upload failed for 2 more unit(s); not published", "{err}");
}

#[test]
fn a_json_command_keeps_stdout_json_while_its_warnings_go_to_stderr() {
    let tmp = TempDir::new().unwrap();
    let wd = dead_store_checkout(&tmp, CP_UNIT);
    let out = run_in(&tmp, &wd, &["cache", "verify", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is one JSON document ({e}):\n{}", String::from_utf8_lossy(&out.stdout)));
    assert!(report["units"].is_array(), "{report}");
    let err = stderr_of(&out);
    assert!(err.contains("cook: warning: cache store unavailable"), "warning on stderr:\n{err}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("warning"), "stdout stays JSON only");
}

#[test]
fn a_wrong_token_is_a_miss_with_a_warning_never_a_failed_build() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let wd = checkout(&tmp, "wrong", &addr, CP_UNIT, true);
    let wrong = "not-the-servers-token";
    let out = Command::new(env!("CARGO_BIN_EXE_cook"))
        .arg("build")
        .current_dir(&wd)
        .env("XDG_CACHE_HOME", tmp.path().join("xdg-wrong"))
        .env(API_KEY_ENV, wrong)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "a refused token must not fail the build:\n{err}");
    assert_eq!(runs(&wd, "build.runlog"), 1, "the unit ran: a miss");
    assert_eq!(
        err.matches("cook: warning: cache store unavailable").count(),
        1,
        "stderr should say once that the store refused the token: {err}"
    );
    assert!(err.contains("401"), "the warning names the refusal: {err}");
    assert!(!err.contains(wrong), "the token never reaches stderr: {err}");
}

/// A module declaring `names` materializers (as cook_dotnet declares its inventory and graph) whose
/// values name the recipe that writes them to `result.txt`; each producer run is logged.
fn materializer_module(wd: &std::path::Path, names: &[&str]) {
    let decls: String = names
        .iter()
        .map(|n| format!("  local {n} = cook.materialize(\"{n}\", {{}}, function()\n    fs.write(\"{n}.ran\", \"ran\")\n    return {{ value = \"{n}-value\" }}\n  end)\n"))
        .collect();
    let values: Vec<String> = names.iter().map(|n| format!("{n}.value")).collect();
    let lua = format!(
        "local M = {{}}\nfunction M.install()\n{decls}  local v = table.concat({{ {} }}, \",\")\n  cook.recipe(\"build\", {{}}, function()\n    cook.add_unit({{ output = \"result.txt\", command = \"printf '\" .. v .. \"' > result.txt\" }})\n  end)\nend\nreturn M\n",
        values.join(", ")
    );
    let dir = wd.join(".cook/modules/share/lua/5.4");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("fixture.lua"), lua).unwrap();
    std::fs::write(wd.join("Cookfile"), "use fixture\n\nfixture.install()\n").unwrap();
}

/// Like `materializer_module`, but each value names a recipe (`<n>-target`) that is a test unit,
/// so `cook test` has a scope that exists only because a materialized value named it.
fn testing_materializer_module(wd: &std::path::Path, names: &[&str]) {
    let decls: String = names
        .iter()
        .map(|n| format!("  local {n} = cook.materialize(\"{n}\", {{}}, function()\n    return {{ target = \"{n}-target\" }}\n  end)\n  cook.recipe({n}.target, {{}}, function()\n    cook.add_unit({{ step_kind = \"test\", name = \"t-\" .. {n}.target, command = \"true\" }})\n  end)\n"))
        .collect();
    let dir = wd.join(".cook/modules/share/lua/5.4");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("fixture.lua"), format!("local M = {{}}\nfunction M.install()\n{decls}end\nreturn M\n")).unwrap();
    std::fs::write(wd.join("Cookfile"), "use fixture\n\nfixture.install()\n").unwrap();
}

#[test]
fn a_dead_store_builds_materialized_values_locally_naming_the_first_failed_put_and_counting_the_rest() {
    let tmp = TempDir::new().unwrap();
    let wd = dead_store_checkout(&tmp, "");
    materializer_module(&wd, &["inventory", "graph"]);
    let err = stderr_of(&run_in(&tmp, &wd, &["build"]));
    assert_eq!(std::fs::read_to_string(wd.join("result.txt")).unwrap(), "inventory-value,graph-value");
    let named = lines_containing(&err, "cache upload for materialized value");
    assert_eq!(named.len(), 1, "the first failed put is told once:\n{err}");
    assert!(named[0].starts_with("cook: warning: cache upload for materialized value fixture."), "{err}");
    assert_eq!(
        lines_containing(&err, "cook: warning: cache upload failed for 1 more materialized value(s); not published").len(),
        1,
        "the second is counted:\n{err}"
    );
    // The recipe's unit fails to publish too, after registration has told its failure.
    assert_eq!(lines_containing(&err, "cache upload for").len(), 1, "one invocation names one failed put:\n{err}");
    assert_eq!(
        lines_containing(&err, "cook: warning: cache upload failed for 1 more unit(s); not published").len(),
        1,
        "the unit's is counted:\n{err}"
    );
    assert!(!err.contains("cache read failed"), "a failed read is a miss, never a warning:\n{err}");
}

#[test]
fn a_wrong_token_builds_a_materialized_value_locally_with_one_warning() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let wd = checkout(&tmp, "wrong", &addr, "", true);
    materializer_module(&wd, &["graph"]);
    let wrong = "not-the-servers-token";
    let out = Command::new(env!("CARGO_BIN_EXE_cook"))
        .arg("build")
        .current_dir(&wd)
        .env("XDG_CACHE_HOME", tmp.path().join("xdg-wrong"))
        .env(API_KEY_ENV, wrong)
        .output()
        .unwrap();
    let err = stderr_of(&out);
    assert!(out.status.success(), "a refused token must not fail the build:\n{err}");
    assert_eq!(std::fs::read_to_string(wd.join("result.txt")).unwrap(), "graph-value");
    let named = lines_containing(&err, "cache upload for materialized value");
    assert_eq!(named.len(), 1, "one warning for the materializer:\n{err}");
    assert!(named[0].contains("401"), "{err}");
    assert!(!err.contains("materialized value(s)"), "nothing more to count:\n{err}");
    assert_eq!(lines_containing(&err, "cache upload for").len(), 1, "the unit's refused put is counted, not named:\n{err}");
    assert!(!err.contains(wrong), "the token never reaches stderr:\n{err}");
}

/// Which dead or refusing store a materializer invocation runs against.
#[derive(Clone, Copy)]
enum Store {
    /// Nobody listens at the endpoint.
    Dead,
    /// A live server that answers 401 to the client's token.
    WrongToken,
    /// A server that was live when the checkout was made and is gone by the run.
    Stopped,
}

/// Runs `cook args` in a fresh checkout of `names` materializers against `store`; returns stderr.
/// `build` is true for the module whose `build` recipe is a unit, false for the one whose
/// `<name>-target` recipes are test units.
fn materializer_stderr(store: Store, names: &[&str], build: bool, args: &[&str]) -> String {
    let tmp = TempDir::new().unwrap();
    let (served, wd) = match store {
        Store::Dead => (None, dead_store_checkout(&tmp, "")),
        Store::WrongToken | Store::Stopped => {
            let (served, addr, _log) = start(&tmp);
            let wd = checkout(&tmp, "live", &addr, "", true);
            let mut cfg = std::fs::read_to_string(wd.join(".cook/cloud.toml")).unwrap();
            cfg.push_str("timeout_secs = 1\nmax_retries = 0\n");
            std::fs::write(wd.join(".cook/cloud.toml"), cfg).unwrap();
            (Some(served), wd)
        }
    };
    if build {
        materializer_module(&wd, names);
    } else {
        testing_materializer_module(&wd, names);
    }
    if matches!(store, Store::Stopped) {
        drop(served);
    }
    let out = Command::new(env!("CARGO_BIN_EXE_cook"))
        .args(args)
        .current_dir(&wd)
        .env("XDG_CACHE_HOME", tmp.path().join("xdg-run"))
        .env(API_KEY_ENV, if matches!(store, Store::WrongToken) { "not-the-servers-token" } else { TOKEN })
        .output()
        .expect("cook invocation");
    let err = stderr_of(&out);
    assert!(out.status.success(), "cook {args:?} failed:\n{err}");
    err
}

/// One invocation names one failed put and counts the rest per writer: exactly `total`
/// `cook: warning:` lines, one `cache upload for` line, and exactly the `summaries` lines.
fn assert_one_record(err: &str, total: usize, summaries: &[&str]) {
    assert_eq!(lines_containing(err, "cook: warning:").len(), total, "total warnings:\n{err}");
    assert_eq!(lines_containing(err, "cache upload for").len(), 1, "one failed put named:\n{err}");
    let counted = lines_containing(err, "cache upload failed for");
    assert_eq!(counted.len(), summaries.len(), "one summary per writer with untold failures:\n{err}");
    for s in summaries {
        let line = format!("cook: warning: cache upload failed for {s}; not published");
        assert_eq!(counted.iter().filter(|l| **l == line).count(), 1, "{line}:\n{err}");
    }
}

#[test]
fn every_command_that_registers_names_one_failed_put_and_counts_the_rest_once() {
    for store in [Store::Dead, Store::WrongToken] {
        for names in [&["graph"][..], &["inventory", "graph"][..]] {
            // Both materializers' puts fail (the second is counted), never one per pass.
            let more: &[&str] = if names.len() == 2 { &["1 more materialized value(s)"] } else { &[] };
            let with = |extra: &[&'static str]| [more, extra].concat();
            let health = 1; // once per run()
            let (named, counted) = (1, more.len());

            // build: health + named put + counted values + the unit's put counted
            let err = materializer_stderr(store, names, true, &["build"]);
            assert_one_record(&err, health + named + counted + 1, &with(&["1 more unit(s)"]));

            // test <scope> lists the scope (own coordinator), then registers and runs: same record.
            // Test units publish nothing, so no unit summary.
            let scope = format!("{}-target", names[0]);
            let err = materializer_stderr(store, names, false, &["test", &scope]);
            assert_one_record(&err, health + named + counted, more);

            // bare test: no listing pass; health + named + counted.
            let err = materializer_stderr(store, names, false, &["test"]);
            assert_one_record(&err, health + named + counted, more);

            // why registers and queries without running: no health line.
            let err = materializer_stderr(store, names, false, &["why", &scope]);
            assert_one_record(&err, named + counted, more);
        }
    }
}

#[test]
fn a_store_that_went_away_after_the_checkout_is_named_once_by_test_scope() {
    let err = materializer_stderr(Store::Stopped, &["inventory", "graph"], false, &["test", "inventory-target"]);
    // health + named put + "1 more materialized value(s)"
    assert_one_record(&err, 3, &["1 more materialized value(s)"]);
}

#[test]
fn audit_against_a_dead_store_never_touches_it() {
    let require = std::env::var("COOK_AUDIT_E2E").is_ok_and(|v| v == "require");
    if std::process::Command::new("strace").arg("-V").output().is_err() {
        assert!(!require, "COOK_AUDIT_E2E=require but strace is not on PATH");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let wd = dead_store_checkout(&tmp, "");
    materializer_module(&wd, &["inventory", "graph"]);
    let out = Command::new(env!("CARGO_BIN_EXE_cook"))
        .args(["audit", "build"])
        .current_dir(&wd)
        .env("XDG_CACHE_HOME", tmp.path().join("xdg-run"))
        .env(API_KEY_ENV, TOKEN)
        .output()
        .unwrap();
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), stderr_of(&out));
    if all.contains("unsupported here") {
        assert!(!require, "COOK_AUDIT_E2E=require but audit is unsupported here: {all}");
        return;
    }
    assert!(!all.contains("cache upload"), "the audit's isolated child publishes nowhere:\n{all}");
}

#[test]
fn a_healthy_store_publishes_a_materialized_value_another_checkout_restores() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let a = checkout(&tmp, "a", &addr, "", true);
    materializer_module(&a, &["graph"]);
    let out = run_in(&tmp, &a, &["build"]);
    assert!(a.join("graph.ran").exists(), "A computes the value");
    assert!(!stderr_of(&out).contains("warning"), "{}", stderr_of(&out));

    let b = checkout(&tmp, "b", &addr, "", true);
    materializer_module(&b, &["graph"]);
    run_in(&tmp, &b, &["build"]);
    assert!(!b.join("graph.ran").exists(), "B restores the value, never runs the producer");
    assert_eq!(std::fs::read_to_string(b.join("result.txt")).unwrap(), "graph-value");
}

// ── COOK-617 (CS-0278): a root-marker declaration is keyed and recorded unit-relative ──

/// A unit declared the cook_pnpm way: `fs.glob("././Cookfile")` joins the working directory with the
/// pattern as written, so the declaration below is `<abs wd>/././dist/**`. The command is relative, so
/// its hash is the same in every checkout.
const ROOT_MARKER_UNIT: &str = r#"recipe build
    cook.add_unit({
        name    = "build-step",
        inputs  = { fs.glob("././Cookfile")[1]:match("^(.*)/Cookfile$") .. "/src/in.txt" },
        outputs = { fs.glob("././Cookfile")[1]:match("^(.*)/Cookfile$") .. "/dist/**" },
        command = "mkdir -p dist && cp src/in.txt dist/a.js && echo ran >> build.runlog",
    })
"#;

/// The cloud key `cook why` reports for the unit.
fn why_cloud_key(tmp: &TempDir, wd: &std::path::Path) -> String {
    let out = run_in(tmp, wd, &["why", "build", "--level", "unit"]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let line = text.lines().find(|l| l.contains("  key ")).unwrap_or_else(|| panic!("no key line in:\n{text}"));
    line.rsplit("key ").next().unwrap().trim().to_string()
}

/// No stored manifest names an absolute path (or the checkout) in what it records.
fn assert_manifests_relative(tmp: &TempDir) {
    let ms = manifests_in(&tmp.path().join("store"));
    assert!(!ms.is_empty(), "A published a manifest");
    let root = tmp.path().to_string_lossy().into_owned();
    for m in ms {
        let text = std::fs::read_to_string(&m).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut names: Vec<String> = json["inputs"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        names.extend(json["output_paths"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)));
        assert!(!names.is_empty(), "manifest records paths: {text}");
        for n in names {
            assert!(!n.starts_with('/') && !n.contains(&root), "absolute path `{n}` in manifest {text}");
        }
    }
}

#[test]
fn a_root_marker_unit_built_in_one_checkout_is_a_shared_hit_in_another() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let a = checkout(&tmp, "a", &addr, ROOT_MARKER_UNIT, true);
    run_in(&tmp, &a, &["build"]);
    assert_eq!(runs(&a, "build.runlog"), 1);
    assert_manifests_relative(&tmp);

    let b = checkout(&tmp, "b", &addr, ROOT_MARKER_UNIT, true);
    assert_eq!(why_cloud_key(&tmp, &a), why_cloud_key(&tmp, &b), "the key does not depend on where the checkout lies");
    run_in(&tmp, &b, &["build"]);
    assert_eq!(runs(&b, "build.runlog"), 0, "B restores, never runs");
    assert_eq!(std::fs::read(b.join("dist/a.js")).unwrap(), std::fs::read(a.join("dist/a.js")).unwrap());
}

#[test]
fn a_root_marker_unit_is_a_shared_hit_in_the_checkout_that_published_it() {
    let tmp = TempDir::new().unwrap();
    let (_served, addr, _log) = start(&tmp);
    let a = checkout(&tmp, "a", &addr, ROOT_MARKER_UNIT, true);
    run_in(&tmp, &a, &["build"]);
    assert_eq!(runs(&a, "build.runlog"), 1);

    // Lose every local record and the output: only the server remembers.
    let _ = std::fs::remove_dir_all(tmp.path().join("xdg-a"));
    std::fs::remove_dir_all(a.join(".cook/cache")).expect("local records live in .cook/cache");
    std::fs::remove_dir_all(a.join("dist")).unwrap();
    run_in(&tmp, &a, &["build"]);
    assert_eq!(runs(&a, "build.runlog"), 1, "restored from the store, not rebuilt");
    assert!(a.join("dist/a.js").is_file());
}
