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
