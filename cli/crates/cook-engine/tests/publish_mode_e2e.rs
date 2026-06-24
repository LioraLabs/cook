//! COOK-168 end-to-end: publish-off mode fetches by key, publishes nothing.
//!
//! Two properties are proven against the real `cook` binary:
//!
//!   1. With `[cloud] publish = false`, a pre-populated shared store still
//!      serves its artifact by key (fetch-by-key is UNAFFECTED by publish-off)
//!      and NO additional artifacts are written to the store after the run.
//!
//!   2. With `[cloud] publish = false`, a fresh build (empty shared store)
//!      succeeds and produces its output, but the shared store remains empty
//!      (no uploads happen).
//!
//! These tests mirror the harness in `sharing_disposition_e2e.rs` exactly.
//! The only deltas are the cloud.toml contents (which set `publish = false`)
//! and the assertions on artifact-file counts.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn cook_binary() -> std::path::PathBuf {
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

/// Write `.cook/cloud.toml` with the given `cloud_toml_body` content,
/// plus `src/in.txt` and a `Cookfile` body.
fn write_fixture(wd: &Path, cache_dir: &Path, cloud_toml_body: &str, cookfile: &str) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(wd.join(".cook/cloud.toml"), cloud_toml_body).unwrap();
    // Ensure cache_dir is mentioned in what we wrote (debug aid).
    let _ = cache_dir; // caller bakes it into cloud_toml_body
    fs::create_dir_all(wd.join("src")).unwrap();
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(wd.join("src/in.txt"), "src-content\n").unwrap();
    fs::write(wd.join("Cookfile"), cookfile).unwrap();
}

/// Build a `cloud.toml` body pointing at `cache_dir` with publish ENABLED.
fn cloud_toml_publish_on(cache_dir: &Path) -> String {
    format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy())
}

/// Build a `cloud.toml` body pointing at `cache_dir` with publish DISABLED.
fn cloud_toml_publish_off(cache_dir: &Path) -> String {
    format!(
        "[cache]\ncache_dir = {:?}\n\n[cloud]\npublish = false\n",
        cache_dir.to_string_lossy()
    )
}

/// Run `cook <recipe>` in `wd`, asserting success.
fn build(wd: &Path, recipe: &str) {
    let out = run(wd, recipe);
    assert!(
        out.status.success(),
        "cook {recipe} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Run `cook <recipe>` in `wd` and return the raw `Output` (status + streams)
/// WITHOUT asserting success.
fn run(wd: &Path, recipe: &str) -> Output {
    Command::new(cook_binary())
        .arg(recipe)
        .current_dir(wd)
        .output()
        .expect("cook invocation")
}

/// Number of times the unit behind `runlog` actually executed (a re-run
/// appends one line). A cache HIT — local or a cold fetch-by-key — leaves the
/// file untouched, so a freshly-deleted runlog stays absent and counts 0.
fn runs(wd: &Path, runlog: &str) -> usize {
    match fs::read_to_string(wd.join("out").join(runlog)) {
        Ok(s) => s.lines().count(),
        Err(_) => 0,
    }
}

/// Count regular files anywhere under `dir` (recursively). The `LocalBackend`
/// lays each published artifact under `<aa>/<rest>` (two-level hex split), so
/// a non-empty shared store has >= 1 regular file here.
fn artifact_file_count(dir: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                count += 1;
            }
        }
    }
    count
}

/// Wipe the local `.cook` index while keeping `cloud.toml` — simulates a
/// fresh consumer with the same shared store.
fn wipe_local_index_keep_cloud_toml(wd: &Path) {
    let dot_cook = wd.join(".cook");
    for entry in fs::read_dir(&dot_cook).unwrap().flatten() {
        if entry.file_name() == "cloud.toml" {
            continue;
        }
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            fs::remove_dir_all(&path).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
    }
}

/// Test A: with `publish = false`, a pre-populated shared store still serves
/// its artifact by key without re-running the command, and NO new artifacts
/// are written to the store after the fetch-by-key run.
///
/// Protocol:
///   Phase 1 — Seed with publish ON: run once to populate the shared store.
///   Phase 2 — Switch to publish OFF: wipe local index, update cloud.toml,
///              delete the output, re-run. The artifact must be fetched from
///              the shared store (command does not re-run) and the artifact
///              count in the store must be unchanged (no new upload).
#[test]
fn publish_off_serves_prepopulated_and_publishes_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();

    let cookfile = r#"recipe make
    ingredients "src/in.txt"
    cook "out/art.txt" {
        cp src/in.txt out/art.txt
        echo ran >> out/art.runlog
    }
"#;

    // ── Phase 1: run with publish ENABLED to populate the shared store ────
    write_fixture(
        wd,
        cache.path(),
        &cloud_toml_publish_on(cache.path()),
        cookfile,
    );

    build(wd, "make");
    assert_eq!(
        runs(wd, "art.runlog"),
        1,
        "phase1: unit must execute once (cold run)"
    );
    assert!(wd.join("out/art.txt").exists(), "phase1: output produced");

    let count_after_seed = artifact_file_count(cache.path());
    assert!(
        count_after_seed >= 1,
        "phase1: the publish-enabled run must place at least one artifact file \
         in the shared store; found none under {}",
        cache.path().display(),
    );

    // ── Phase 2: switch to publish OFF, simulate fresh consumer ──────────
    // Update cloud.toml to disable publish before wiping the local index.
    fs::write(
        wd.join(".cook/cloud.toml"),
        cloud_toml_publish_off(cache.path()),
    )
    .unwrap();

    wipe_local_index_keep_cloud_toml(wd);
    // Delete the output so a re-run would produce a different file, but a
    // fetch-by-key should restore it.
    fs::remove_file(wd.join("out/art.txt")).unwrap();
    // Delete the runlog so a body re-run would be observable.
    let _ = fs::remove_file(wd.join("out/art.runlog")); // may already be gone

    build(wd, "make");

    // (1) Output restored from the shared store.
    assert!(
        wd.join("out/art.txt").exists(),
        "phase2 (publish-off): the pre-populated artifact MUST be served by \
         fetch-by-key — output must be restored"
    );

    // (2) Command body did NOT re-run: the deleted runlog must stay absent.
    assert_eq!(
        runs(wd, "art.runlog"),
        0,
        "phase2 (publish-off): the command MUST NOT re-run — fetch-by-key \
         restores the output; the deleted runlog stays absent (0)"
    );

    // (3) No new artifacts uploaded: count must equal the seeded count.
    let count_after_fetch = artifact_file_count(cache.path());
    assert_eq!(
        count_after_fetch,
        count_after_seed,
        "phase2 (publish-off): artifact count in shared store MUST NOT grow — \
         publish is disabled, so no new artifact may be uploaded \
         (before={count_after_seed}, after={count_after_fetch})"
    );
}

/// Test B: with `publish = false` and an EMPTY shared store, a fresh build
/// succeeds, produces its output, but the shared store remains empty
/// (nothing is uploaded).
#[test]
fn publish_off_fresh_build_succeeds_and_publishes_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();

    // Confirm the cache starts empty.
    assert_eq!(
        artifact_file_count(cache.path()),
        0,
        "precondition: cache tempdir must start empty"
    );

    write_fixture(
        wd,
        cache.path(),
        &cloud_toml_publish_off(cache.path()),
        r#"recipe make
    ingredients "src/in.txt"
    cook "out/art.txt" {
        cp src/in.txt out/art.txt
        echo ran >> out/art.runlog
    }
"#,
    );

    // Run with publish OFF and empty shared store (cold both sides).
    build(wd, "make");

    // (1) The output was produced — the engine tolerated the missing publish
    //     and ran the unit to completion.
    assert!(
        wd.join("out/art.txt").exists(),
        "publish-off fresh build: the unit MUST run and produce its output \
         even though no artifact is uploaded"
    );

    // (2) The run succeeded — confirmed by `build()` above (no panic).

    // (3) The shared store holds nothing — no artifact was uploaded.
    let count = artifact_file_count(cache.path());
    assert_eq!(
        count,
        0,
        "publish-off fresh build: the shared store MUST remain empty — \
         publish is disabled so no artifact may be uploaded \
         (found {count} artifact file(s) under {})",
        cache.path().display(),
    );
}

/// Test C: a fresh build that includes a `probe` succeeds under publish-off
/// and uploads NOTHING — including the probe value. The probe-value put is the
/// fifth shared-store upload site; publish-off must suppress it too so the
/// §17.1.3 / CS-0111 "no artifact for ANY unit" guarantee holds. The probe's
/// canonical local copy (`.cook/probes/<key>.json`) and the per-run store are
/// still populated, so the consuming unit reads the value and runs to
/// completion.
#[test]
fn publish_off_probe_build_succeeds_and_publishes_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();

    assert_eq!(
        artifact_file_count(cache.path()),
        0,
        "precondition: cache tempdir must start empty"
    );

    // A top-level `probe` plus a unit that seals on it. Executing a reachable
    // sealed unit forces the probe to run (its value folds into the unit's
    // key), which exercises the probe-value upload site — the fifth shared-
    // store put. Under publish-off it must upload nothing. (Sealing, as in
    // `seal_host_key_e2e.rs`, avoids entangling probe value-substitution
    // syntax; the point here is purely that the probe's put is suppressed.)
    write_fixture(
        wd,
        cache.path(),
        &cloud_toml_publish_off(cache.path()),
        r#"probe tag
    produce { echo PUBOFF }

recipe make
    ingredients "src/in.txt"
    seal tag
    cook "out/art.txt" {
        cp src/in.txt out/art.txt
        echo ran >> out/art.runlog
    }
"#,
    );

    // Cold both sides, publish OFF.
    build(wd, "make");

    // (1) The output was produced — the sealed unit ran and the probe executed
    //     to provide the sealed determinant, despite publish being off.
    assert!(
        wd.join("out/art.txt").exists(),
        "publish-off probe build: the sealed unit MUST run and produce its output"
    );

    // (2) The probe's canonical local copy exists — local materialisation is
    //     unaffected by publish-off.
    assert!(
        wd.join(".cook").join("probes").exists(),
        "publish-off probe build: the local probe materialisation dir MUST exist"
    );

    // (3) The shared store holds NOTHING — neither the unit artifact NOR the
    //     probe value was uploaded.
    let count = artifact_file_count(cache.path());
    assert_eq!(
        count,
        0,
        "publish-off probe build: the shared store MUST remain empty — the probe \
         value is a shared-store upload site and publish-off suppresses it too \
         (found {count} artifact file(s) under {})",
        cache.path().display(),
    );
}
