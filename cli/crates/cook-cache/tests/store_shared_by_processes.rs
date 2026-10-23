//! Several real processes writing one `LocalBackend` store directory: the
//! parent test re-executes this very test binary as children (role and
//! paths in env vars), releases them together through a go-file, and judges
//! the store and the result files they leave behind.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cook_cache::backend::{
    ArtifactMeta, CacheBackend, CloudKey, DeterminantManifest, EvictPolicy, LocalBackend,
    plan_eviction, put_bytes,
};
use sha2::{Digest, Sha256};

const ENV_ROLE: &str = "COOK_SHARED_STORE_ROLE";
const ENV_STORE: &str = "COOK_SHARED_STORE_DIR";
const ENV_WORK: &str = "COOK_SHARED_STORE_WORK";
const ENV_IDX: &str = "COOK_SHARED_STORE_IDX";
const ENV_KEYS: &str = "COOK_SHARED_STORE_KEYS";
const CONFLICT: &str = "artifact key conflict";

fn meta() -> ArtifactMeta {
    ArtifactMeta {
        recipe_namespace: "shared/Cookfile::build".into(),
        command_hash: 1,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: 3,
        size_bytes: 0,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: "out".into(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: None,
        mode: ArtifactMeta::default_mode(),
        target: None,
    }
}

fn key(i: usize) -> CloudKey {
    let mut k = [0u8; 32];
    k[0] = (i % 16) as u8; // few shards: the shard lock is what is contended
    k[1..9].copy_from_slice(&(i as u64).to_le_bytes());
    k
}

fn bytes_of(idx: usize, i: usize) -> Vec<u8> {
    format!("process {idx} wrote key {i} {}", "x".repeat(200 + idx)).into_bytes()
}

fn manifest(idx: usize) -> DeterminantManifest {
    DeterminantManifest {
        schema_version: 5,
        recipe_namespace: format!("proc-{idx}"),
        key: "ab".repeat(32),
        command_hash: idx as u64,
        env_contribution: 0,
        seal_contribution: 0,
        inputs: Default::default(),
        output_paths: vec![format!("out-{idx}"); 50],
        empty_dir_outputs: Vec::new(),
        consulted_env: Default::default(),
        sealed_probes: Default::default(),
        observation: None,
    }
}

fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

// ─── parent side ────────────────────────────────────────────────────────

struct Run {
    store: tempfile::TempDir,
    work: tempfile::TempDir,
    children: Vec<Child>,
}

impl Run {
    fn new() -> Self {
        Run {
            store: tempfile::tempdir().unwrap(),
            work: tempfile::tempdir().unwrap(),
            children: Vec::new(),
        }
    }

    fn spawn(&mut self, role: &str, idx: usize, keys: usize) {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "child_role", "--nocapture", "--test-threads=1"])
            .env(ENV_ROLE, role)
            .env(ENV_STORE, self.store.path())
            .env(ENV_WORK, self.work.path())
            .env(ENV_IDX, idx.to_string())
            .env(ENV_KEYS, keys.to_string())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        self.children.push(child);
    }

    fn go(&self) {
        fs::write(self.work.path().join("go"), b"").unwrap();
    }

    /// Wait for the first `n` spawned children (the writers).
    fn wait_first(&mut self, n: usize) {
        for i in 0..n {
            assert!(wait_bounded(&mut self.children[i], i).success(), "writer child {i} failed");
        }
        self.children.drain(..n);
    }

    fn stop_rest(&mut self) {
        fs::write(self.work.path().join("stop"), b"").unwrap();
        for (i, c) in self.children.iter_mut().enumerate() {
            assert!(wait_bounded(c, i).success(), "side child {i} failed");
        }
        self.children.clear();
    }

    fn results(&self, role: &str, idx: usize) -> Vec<String> {
        fs::read_to_string(self.work.path().join(format!("{role}-{idx}")))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn backend(&self) -> LocalBackend {
        LocalBackend::new(self.store.path().to_path_buf())
    }
}

/// A failed parent must not leave children running: stop them, then kill and reap.
impl Drop for Run {
    fn drop(&mut self) {
        let _ = fs::write(self.work.path().join("stop"), b"");
        for c in &mut self.children {
            if matches!(c.try_wait(), Ok(None)) {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

/// Wait for a child up to a generous deadline, then kill it and panic.
fn wait_bounded(c: &mut Child, n: usize) -> std::process::ExitStatus {
    let end = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = c.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= end {
            let _ = c.kill();
            let _ = c.wait();
            panic!("child {n} (pid {}) did not finish within 120s", c.id());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Every shard dir holds only 62-hex blobs with their sidecars, and the lock file.
fn assert_tidy(store: &Path) {
    for shard in fs::read_dir(store).unwrap().flatten() {
        for f in fs::read_dir(shard.path()).unwrap().flatten() {
            let name = f.file_name().into_string().unwrap();
            let stem = name.strip_suffix(".meta.json").unwrap_or(&name);
            let ok = name == "lock" || (stem.len() == 62 && stem.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(ok, "stray file in the store: {}", f.path().display());
            if name.ends_with(".meta.json") {
                assert!(
                    shard.path().join(stem).exists(),
                    "sidecar without a blob: {}",
                    f.path().display()
                );
            }
        }
    }
}

/// Every stored entry verifies against its own sidecar's hash.
fn assert_pairs_agree(backend: &LocalBackend) -> usize {
    let found = backend.enumerate().unwrap();
    for c in &found {
        let (mut r, m) = backend
            .get_with_meta(&c.key)
            .unwrap()
            .expect("blob without a readable sidecar");
        let mut got = Vec::new();
        r.read_to_end(&mut got).expect("blob disagrees with its sidecar");
        assert_eq!(sha(&got), m.content_hash);
    }
    found.len()
}

const PROCS: usize = 8;
const KEYS: usize = 400;

#[test]
fn differing_bytes_one_winner_per_key() {
    let mut run = Run::new();
    for idx in 0..PROCS {
        run.spawn("writer", idx, KEYS);
    }
    run.go();
    run.wait_first(PROCS);

    let all: Vec<Vec<String>> = (0..PROCS).map(|i| run.results("writer", i)).collect();
    let backend = run.backend();
    for i in 0..KEYS {
        let mut oks = Vec::new();
        for (idx, lines) in all.iter().enumerate() {
            match lines[i].as_str() {
                "ok" => oks.push(idx),
                l if l.contains(CONFLICT) => {}
                l => panic!("key {i}: process {idx}: put failed as something else: {l}"),
            }
        }
        assert_eq!(oks.len(), 1, "key {i}: winners {oks:?}");
        let (mut r, m) = backend.get_with_meta(&key(i)).unwrap().expect("entry");
        let mut got = Vec::new();
        r.read_to_end(&mut got).expect("verifies at EOF");
        assert_eq!(got, bytes_of(oks[0], i), "key {i}: stored bytes are not the winner's");
        assert_eq!(m.content_hash, sha(&got));
    }
    assert_eq!(assert_pairs_agree(&backend), KEYS);
    assert_tidy(run.store.path());
}

#[test]
fn same_bytes_from_every_process_all_succeed() {
    let mut run = Run::new();
    for idx in 0..PROCS {
        run.spawn("same", idx, KEYS);
    }
    run.go();
    run.wait_first(PROCS);
    for idx in 0..PROCS {
        assert!(run.results("same", idx).iter().all(|l| l == "ok"), "process {idx}");
    }
    let backend = run.backend();
    assert_eq!(assert_pairs_agree(&backend), KEYS);
    assert_tidy(run.store.path());
}

#[test]
fn gc_beside_writers_leaves_no_mismatched_pair_and_no_put_failure() {
    let mut run = Run::new();
    for idx in 0..PROCS {
        run.spawn("writer", idx, KEYS);
    }
    run.spawn("gc", 0, KEYS);
    run.spawn("reader", 0, KEYS);
    run.go();
    run.wait_first(PROCS);
    run.stop_rest();

    for idx in 0..PROCS {
        for (i, l) in run.results("writer", idx).iter().enumerate() {
            assert!(l == "ok" || l.contains(CONFLICT), "key {i}: process {idx}: {l}");
        }
    }
    let seen = run.results("reader", 0);
    assert!(seen.is_empty(), "reader saw: {seen:?}");
    assert_pairs_agree(&run.backend());
    assert_tidy(run.store.path());
}

#[test]
fn manifests_from_every_process_all_land_whole() {
    let mut run = Run::new();
    for idx in 0..PROCS {
        run.spawn("manifest", idx, KEYS);
    }
    run.go();
    run.wait_first(PROCS);
    for idx in 0..PROCS {
        let lines = run.results("manifest", idx);
        assert!(lines.iter().all(|l| l == "ok"), "process {idx}: {lines:?}");
    }
    let got = run.backend().get_manifest(&key(0)).unwrap().expect("a manifest");
    assert!((0..PROCS).any(|i| got == manifest(i)), "a manifest nobody wrote");
}

// ─── child side ─────────────────────────────────────────────────────────

fn wait_for(path: &Path) {
    let end = Instant::now() + Duration::from_secs(60);
    while !path.exists() {
        assert!(Instant::now() < end, "go-file never appeared");
        // An orphan (parent gone, work dir removed) must not wait on.
        assert!(path.parent().is_some_and(Path::exists), "work dir is gone");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Runs only when re-executed by a parent test above; a bare `cargo test` skips it.
#[test]
fn child_role() {
    let Ok(role) = std::env::var(ENV_ROLE) else { return };
    let env = |n: &str| std::env::var(n).unwrap();
    let work = PathBuf::from(env(ENV_WORK));
    let idx: usize = env(ENV_IDX).parse().unwrap();
    let keys: usize = env(ENV_KEYS).parse().unwrap();
    let backend = LocalBackend::new(PathBuf::from(env(ENV_STORE)));
    // Also stops when the work dir is gone, so an orphan never spins forever.
    let stopped = || work.join("stop").exists() || !work.exists();
    wait_for(&work.join("go"));

    let mut out = Vec::new();
    match role.as_str() {
        "writer" | "same" => {
            for i in 0..keys {
                let bytes = if role == "same" { bytes_of(0, i) } else { bytes_of(idx, i) };
                out.push(match put_bytes(&backend, &key(i), &bytes, &mut meta()) {
                    Ok(()) => "ok".to_string(),
                    Err(e) => format!("{e:?}"),
                });
            }
        }
        "manifest" => {
            let m = manifest(idx);
            for _ in 0..keys {
                out.push(match backend.put_manifest(&key(0), &m) {
                    Ok(()) => "ok".to_string(),
                    Err(e) => format!("{e:?}"),
                });
            }
        }
        "gc" => {
            while !stopped() {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                let found = backend.enumerate().unwrap();
                let plan = plan_eviction(&found, &EvictPolicy::manual(Some(0), None), now);
                backend.apply_eviction(&plan).unwrap();
            }
        }
        // Problems only: an empty result file is a clean reader.
        "reader" => {
            while !stopped() {
                for i in 0..keys {
                    match backend.get_with_meta(&key(i)) {
                        Ok(None) => {}
                        Ok(Some((mut r, m))) => {
                            let mut got = Vec::new();
                            match r.read_to_end(&mut got) {
                                Ok(_) if sha(&got) == m.content_hash => {}
                                Ok(_) => out.push(format!("key {i}: unverified bytes returned")),
                                // A lock-free reader can briefly see one put's blob beside another's
                                // sidecar; verification turns that into a miss, not wrong bytes.
                                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {}
                                Err(e) => out.push(format!("key {i}: read: {e}")),
                            }
                        }
                        Err(e) => out.push(format!("key {i}: get: {e:?}")),
                    }
                }
            }
        }
        other => panic!("unknown role {other}"),
    }
    fs::write(work.join(format!("{role}-{idx}")), out.join("\n")).unwrap();
}
