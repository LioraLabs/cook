//! End-to-end proof that `cook why` explains a shared cache MISS by diffing
//! the consumer's resolved determinants against the producer determinant
//! manifest (COOK-165, §17.1.6 / COOK-166).
//!
//! The bar this test proves: "why did I miss the shared artifact" is answerable
//! from `cook why` output ALONE — the output must NAME the differing determinant,
//! which is only possible if the producer manifest was fetched (via
//! `get_manifest(K)`) and diffed against the consumer's live determinants.
//!
//! ## Why the scenario is shaped the way it is
//!
//! The engine (`why::classify` → `why::manifest_diff`) fetches the producer
//! manifest at the CONSUMER's recomputed key K. A NAMED diff therefore only
//! appears when:
//!   1. a manifest EXISTS at the consumer's K, AND
//!   2. the artifact bytes are ABSENT at K (else `shared_artifacts_present`
//!      classifies the unit as a `SharedHit` and no diff is computed), AND
//!   3. some recorded determinant VALUE in that manifest differs from the
//!      consumer's live value.
//!
//! If the consumer changed a key-folded determinant (e.g. an env value), its
//! recomputed K would differ and `get_manifest(consumer_K)` would return None —
//! yielding "no producer manifest published for this key", NOT a named diff.
//!
//! So we drive a real build round-trip, then surgically reproduce the
//! real-world "manifest present, artifact gone/unfetchable" diagnostic case:
//!   * Phase A — `cook build` publishes both the artifact AND the producer
//!     manifest (`<K>.provenance.json`) to the shared backend.
//!   * Phase B — delete the artifact bytes from the shared store (keep ONLY the
//!     `*.provenance.json` manifest), MUTATE one recorded determinant value in
//!     the manifest WITHOUT changing the filename K (we flip the recorded
//!     `inputs` hash for `src/in.txt`), wipe the local `.cook/cache` index, then
//!     run `cook why build` against the SAME workspace (so the consumer
//!     recomputes the SAME K, the mutated manifest is found, but the recorded
//!     input hash differs from the live one).
//!
//! Result: `MISS (shared)` + a `shared-miss diff vs producer manifest:` section
//! naming `input src/in.txt: ours Some(...) != producer Some(...)`.
//!
//! This is the manifest-mutation path (NOT the absent-manifest fallback).

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

/// One cook unit consuming a declared input file `src/in.txt`, producing
/// `out.txt`. The input file's content hash is recorded in the producer
/// manifest under `inputs`, giving us a determinant whose recorded VALUE we can
/// flip on disk without disturbing the cache key K (the manifest filename).
const COOKFILE: &str = r#"
recipe build
        cook.add_unit({
            name    = "build-step",
            inputs  = {"src/in.txt"},
            outputs = {"out.txt"},
            command = "cp src/in.txt out.txt",
        })
"#;

/// Init a tempdir workspace with a private shared-cache backend (isolated from
/// the host-wide `~/.cache/cook/cloud` store) and the Cookfile + input.
fn init_workspace() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join(".cook")).unwrap();
    // Point the shared backend at a private subdir so the producer manifest is
    // published there and we control its bytes. Without this the global store
    // could serve a prior run's artifact as a spurious hit.
    let shared = dir.path().join(".cook/shared-cache");
    fs::write(
        dir.path().join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", shared.to_string_lossy()),
    )
    .unwrap();
    fs::write(dir.path().join("Cookfile"), COOKFILE).unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/in.txt"), "hello-input\n").unwrap();
    dir
}

fn run_cook(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_bin())
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

/// Walk the shared-cache dir and return the single `*.provenance.json` manifest
/// path. Panics if there isn't exactly one.
fn find_manifest(shared: &Path) -> std::path::PathBuf {
    let manifests: Vec<_> = walkdir::WalkDir::new(shared)
        .into_iter()
        .flatten()
        .filter(|e| {
            e.path().is_file()
                && e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".provenance.json"))
                    .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect();
    assert_eq!(
        manifests.len(),
        1,
        "expected exactly one producer manifest under {}, found {:?}",
        shared.display(),
        manifests,
    );
    manifests.into_iter().next().unwrap()
}

#[test]
fn cook_why_explains_shared_miss_via_producer_determinant_diff() {
    let dir = init_workspace();
    let shared = dir.path().join(".cook/shared-cache");

    // ── Phase A: build → publishes artifact + producer manifest to shared store.
    let build = run_cook(dir.path(), &["build"]);
    assert!(
        build.status.success(),
        "build failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );
    assert!(
        dir.path().join("out.txt").exists(),
        "build should have produced out.txt"
    );

    // The producer manifest must have been published to the SHARED store.
    let manifest_path = find_manifest(&shared);
    let manifest_before = fs::read_to_string(&manifest_path).unwrap();
    // Sanity: the manifest records the input hash we are about to flip.
    assert!(
        manifest_before.contains("src/in.txt"),
        "manifest must record the src/in.txt input determinant; got: {manifest_before}"
    );

    // ── Phase B, step 1: force the SharedMiss-with-manifest-present path by
    // deleting the artifact bytes (and their .meta.json sidecar) from the shared
    // store while KEEPING the *.provenance.json manifest. With the artifact gone,
    // `shared_artifacts_present` returns false → SharedMiss → manifest_diff runs.
    for entry in walkdir::WalkDir::new(&shared).into_iter().flatten() {
        let p = entry.path();
        if p.is_file()
            && !p
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".provenance.json"))
                .unwrap_or(false)
        {
            fs::remove_file(p).unwrap();
        }
    }
    // Manifest survived the cull.
    assert!(
        manifest_path.exists(),
        "the .provenance.json manifest must survive the artifact-bytes cull"
    );

    // ── Phase B, step 2: mutate ONE recorded determinant value in the manifest
    // WITHOUT changing the filename K. The manifest records the input hash as a
    // zero-padded lowercase-hex u64 (the `hex_u64_map` convention). We extract
    // the recorded src/in.txt hash and replace it with a clearly-wrong sentinel,
    // so the consumer's live hash will differ from the recorded one.
    //
    // (serde_json is not a dev-dep of cook-cli; a targeted string replace on the
    // unique recorded hex value is sufficient and avoids adding one.)
    let recorded_hash = extract_input_hash(&manifest_before, "src/in.txt");
    assert_ne!(
        recorded_hash, "deadbeefdeadbeef",
        "sentinel must differ from the real recorded hash"
    );
    let mutated = manifest_before.replace(
        &format!("\"{recorded_hash}\""),
        "\"deadbeefdeadbeef\"",
    );
    assert_ne!(
        mutated, manifest_before,
        "manifest mutation must have replaced the recorded input hash"
    );
    fs::write(&manifest_path, &mutated).unwrap();

    // ── Phase B, step 3: wipe the local index so the consumer can't take a
    // trivial local hit and must consult the shared store. The input file on
    // disk is UNCHANGED, so the consumer recomputes the SAME key K → the mutated
    // manifest is found at K, but its recorded input hash differs from ours.
    let _ = fs::remove_dir_all(dir.path().join(".cook/cache"));

    // ── Run `cook why build` and assert the named-diff output.
    let why = run_cook(dir.path(), &["why", "build"]);
    assert!(
        why.status.success(),
        "why failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&why.stdout),
        String::from_utf8_lossy(&why.stderr),
    );
    let out = String::from_utf8_lossy(&why.stdout);

    // 1. The unit is reported as a shared MISS.
    assert!(
        out.contains("MISS (shared)"),
        "expected a shared MISS for the unit; got:\n{out}"
    );
    // 2. The shared-miss diff section is present (proving the manifest was
    //    fetched via get_manifest and diffed — NOT the "no manifest" branch).
    assert!(
        out.contains("shared-miss diff vs producer manifest:"),
        "expected the producer-manifest diff section; got:\n{out}"
    );
    assert!(
        !out.contains("no producer manifest published for this key"),
        "must NOT take the absent-manifest branch (manifest is present at K); got:\n{out}"
    );
    // 3. The diff NAMES the differing determinant: input src/in.txt, with the
    //    producer's mutated value (0xdeadbeefdeadbeef == 16045690984833335023).
    assert!(
        out.contains("input src/in.txt: ours Some(")
            && out.contains("producer Some(16045690984833335023)"),
        "diff must name `input src/in.txt` with the mutated producer hash; got:\n{out}"
    );

    // ── --json must carry the same attributed diff (structured form).
    let why_json = run_cook(dir.path(), &["why", "build", "--json"]);
    assert!(why_json.status.success());
    let js = String::from_utf8_lossy(&why_json.stdout);
    assert!(
        js.contains("\"status\": \"shared_miss\""),
        "json must report shared_miss; got:\n{js}"
    );
    assert!(
        js.contains("\"determinant\": \"input:src/in.txt\"")
            && js.contains("\"producer\": \"deadbeefdeadbeef\""),
        "json manifest_diff must name the input determinant with the producer's mutated hash; got:\n{js}"
    );
}

/// Pull the recorded zero-padded-hex u64 hash for `path` out of the (compact)
/// manifest JSON. Looks for `"path":"<16 hex chars>"` and returns the hex.
fn extract_input_hash(manifest: &str, path: &str) -> String {
    let needle = format!("\"{path}\":\"");
    let start = manifest
        .find(&needle)
        .unwrap_or_else(|| panic!("manifest must record input {path}; got: {manifest}"))
        + needle.len();
    let rest = &manifest[start..];
    let end = rest.find('"').expect("recorded hash must be quote-terminated");
    rest[..end].to_string()
}

// ---------------------------------------------------------------------------
// Import-fixture attribution: a unit belonging to an IMPORTED recipe must be
// attributed to its workspace-qualified name (`api.compile`), never to its
// bare local declaration name (`compile`) — which, worse, can collide with
// the querying recipe's own name when the two happen to share one.
// ---------------------------------------------------------------------------

const ROOT_COOKFILE: &str = r#"
import api ./api

recipe build: api.compile
    cook "build/top.txt" {
        mkdir -p build
        echo top > $<out>
    }
"#;

const API_COOKFILE: &str = r#"
recipe compile
    cook "build/api-build.stamp" {
        mkdir -p build
        echo stamp > $<out>
    }
"#;

/// Init a tempdir workspace with a root Cookfile importing `./api`, isolated
/// from the host-wide `~/.cache/cook/cloud` shared-cache store (same
/// isolation pattern as `init_workspace`) so this test never depends on, or
/// pollutes, host state.
fn init_import_workspace() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join(".cook")).unwrap();
    let shared = dir.path().join(".cook/shared-cache");
    fs::write(
        dir.path().join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", shared.to_string_lossy()),
    )
    .unwrap();
    fs::write(dir.path().join("Cookfile"), ROOT_COOKFILE).unwrap();
    fs::create_dir_all(dir.path().join("api")).unwrap();
    fs::write(dir.path().join("api/Cookfile"), API_COOKFILE).unwrap();
    dir
}

#[test]
fn cook_why_attributes_imported_unit_to_its_qualified_recipe_name() {
    let dir = init_import_workspace();

    // Build once so both units take a local hit on the subsequent `why`.
    let build = run_cook(dir.path(), &["build"]);
    assert!(
        build.status.success(),
        "build failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );
    assert!(
        dir.path().join("build/top.txt").exists(),
        "root recipe should have produced build/top.txt"
    );
    assert!(
        dir.path().join("api/build/api-build.stamp").exists(),
        "imported recipe should have produced api/build/api-build.stamp"
    );

    let why = run_cook(dir.path(), &["why", "build"]);
    assert!(
        why.status.success(),
        "why failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&why.stdout),
        String::from_utf8_lossy(&why.stderr),
    );
    let out = String::from_utf8_lossy(&why.stdout);

    // The imported dependency's unit is attributed to its workspace-qualified
    // recipe name, `api.compile` — never the bare local declaration name
    // `compile`, and never the querying recipe's own name `build`.
    assert!(
        out.contains("api.compile :: build/api-build.stamp"),
        "expected the imported unit attributed to `api.compile`; got:\n{out}"
    );
    assert!(
        !out.contains("\ncompile :: "),
        "must NOT attribute the imported unit to its bare local name `compile`; got:\n{out}"
    );

    // The root recipe's own unit keeps its (unqualified, since it lives at
    // the workspace root) attribution unchanged.
    assert!(
        out.contains("build :: build/top.txt"),
        "expected the root unit attributed to `build`; got:\n{out}"
    );

    // Cache keys / HIT status / all other fields are unaffected by the
    // attribution fix: the dep unit is still a clean local hit with a key.
    let dep_line = out
        .lines()
        .find(|l| l.starts_with("api.compile :: "))
        .unwrap_or_else(|| panic!("expected an `api.compile ::` line; got:\n{out}"));
    // COOK-276 dual-tier labels: the local tier reads HIT; the shared tier is
    // reported alongside it explicitly rather than swallowing the local answer.
    assert!(
        dep_line.contains("HIT (local)") && dep_line.contains("key "),
        "expected the dep unit's line to show a local-hit status and key; got:\n{dep_line}"
    );
}
