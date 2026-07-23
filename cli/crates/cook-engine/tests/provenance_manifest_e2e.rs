//! COOK-166 end-to-end: a published artifact has a retrievable, MATCHING
//! determinant manifest.
//!
//! Mirrors `sharing_disposition_e2e.rs`: a real `cook` binary builds a tiny
//! Cookfile against a `LocalBackend` shared store rooted in a tempdir (configured
//! via `.cook/cloud.toml`). The single cacheable `cook` step reads one env var
//! via the `$<GREETING>` sigil (lowered to `cook.require_env(GREETING)`), so the
//! producer's `consulted_env` is non-empty.
//!
//! After the build, the manifest published as the `{key}.provenance.json` sidecar
//! (CS-0110 / §{exec.cache.single-key}) is located on disk and asserted to MATCH
//! the producer's key inputs: schema_version is `CACHE_VERSION`, `consulted_env`
//! records `GREETING`, `inputs` is non-empty (the declared source), `output_paths`
//! contains the declared output, and `get_manifest(K)` for the recorded key K
//! returns the very same manifest (the retrieve-by-key path COOK-165/167 build on).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cook_cache::backend::LocalBackend;
use cook_cache::CacheBackend;
use cook_fingerprint::backend::DeterminantManifest;
use cook_fingerprint::CACHE_VERSION;

fn cook_binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // .../target/debug/deps  ->  .../target/debug
    path.pop();
    path.push("cook");
    assert!(
        path.exists(),
        "cook binary not found at {} — run `cargo build --bin cook` first",
        path.display()
    );
    path
}

/// Write `.cook/cloud.toml` pointing the shared store at `cache_dir` (a SEPARATE
/// tempdir from `wd/.cook`), plus `src/in.txt` and a `Cookfile` body.
fn write_fixture(wd: &Path, cache_dir: &Path, cookfile: &str) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
    fs::create_dir_all(wd.join("src")).unwrap();
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(wd.join("src/in.txt"), "src-content\n").unwrap();
    fs::write(wd.join("Cookfile"), cookfile).unwrap();
}

/// Recursively find the single regular file under `dir` whose name ends with
/// `suffix`. Panics if zero or more than one match (the test publishes exactly
/// one unit, so exactly one manifest sidecar is expected). On failure it dumps
/// the whole tree to ease debugging WHERE the sidecar actually landed.
fn find_one_file_ending_with(dir: &Path, suffix: &str) -> Option<PathBuf> {
    let mut matches = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(suffix))
            {
                matches.push(path);
            }
        }
    }
    match matches.len() {
        0 => None,
        1 => Some(matches.pop().unwrap()),
        _ => panic!("expected exactly one *{suffix} under {}, found {matches:#?}", dir.display()),
    }
}

/// Test: a published artifact has a retrievable, MATCHING determinant manifest.
#[test]
fn published_artifact_has_matching_determinant_manifest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    let backend_root = cache.path().to_path_buf();

    // One cacheable `cook` step, one output, reading one env var via `$<GREETING>`
    // so the producer's consulted_env is non-empty. The unit is unannotated, so
    // it publishes to the shared store (the determinant manifest is gated on
    // !local — see executor.rs build_determinant_manifest call sites).
    write_fixture(
        wd,
        &backend_root,
        r#"config default
    env.GREETING = host.env("GREETING", "")

recipe make
    ingredients "src/in.txt"
    cook "out/art.txt" {
        printf '%s\n' "$<GREETING>" > out/art.txt
    }
"#,
    );

    // Run the build with GREETING set so it lands in consulted_env.
    let out = Command::new(cook_binary())
        .arg("make")
        .env("GREETING", "hello-cook")
        .current_dir(wd)
        .output()
        .expect("cook invocation");
    assert!(
        out.status.success(),
        "cook make failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(wd.join("out/art.txt").exists(), "output produced");

    // Locate the single *.provenance.json sidecar under the shared store root.
    let prov = find_one_file_ending_with(&backend_root, ".provenance.json").unwrap_or_else(|| {
        // Dump the tree to debug WHERE artifacts/manifests landed.
        let mut tree = Vec::new();
        let mut stack = vec![backend_root.clone()];
        while let Some(p) = stack.pop() {
            if let Ok(rd) = fs::read_dir(&p) {
                for e in rd.flatten() {
                    let path = e.path();
                    tree.push(path.display().to_string());
                    if e.file_type().map(|f| f.is_dir()).unwrap_or(false) {
                        stack.push(path);
                    }
                }
            }
        }
        panic!(
            "a determinant manifest (*.provenance.json) must have been published \
             under the shared store root {}, but none was found.\nTree:\n{}",
            backend_root.display(),
            tree.join("\n"),
        )
    });

    let manifest: DeterminantManifest =
        serde_json::from_slice(&fs::read(&prov).unwrap()).expect("manifest is valid JSON");

    // Producer key inputs must MATCH the manifest.
    assert_eq!(
        manifest.schema_version, CACHE_VERSION,
        "manifest schema_version must equal the current CACHE_VERSION"
    );
    assert!(
        !manifest.inputs.is_empty(),
        "inputs must record the declared source(s): {:?}",
        manifest.inputs
    );
    assert!(
        manifest.output_paths.iter().any(|p| p.contains("out")),
        "output_paths must contain the declared output: {:?}",
        manifest.output_paths
    );
    assert!(
        manifest.consulted_env.contains_key("GREETING"),
        "consulted_env should record the env var the step read: {:?}",
        manifest.consulted_env
    );

    // get_manifest(K) for the recorded key K returns the SAME manifest — the
    // retrieve-by-key path (the manifest is keyed by the unit's cloud_key K, and
    // manifest.key is the hex of that K).
    let key_bytes: [u8; 32] = hex::decode(&manifest.key)
        .expect("manifest.key is hex")
        .try_into()
        .expect("manifest.key is 32 bytes");
    let backend = LocalBackend::new(backend_root);
    assert_eq!(
        backend.get_manifest(&key_bytes).unwrap().unwrap(),
        manifest,
        "get_manifest(K) returns the manifest stored at K's sidecar"
    );
}
