//! COOK-163 end-to-end: the `record` disposition bar, against the real `cook`
//! binary in an isolated tmpdir.
//!
//! `record` marks a unit whose artifact is intrinsically NON-reproducible (LLM /
//! image generation): re-running the producer would yield a different output for
//! the same inputs. The two properties this proves:
//!
//!   1. A `record` unit's WARM HIT does NOT re-run its producer. The same cache
//!      key reuses the existing recording even though the output (`date +%N`) is
//!      non-reproducible — the present recording is authoritative.
//!   2. Changing a SEALED determinant re-keys the unit and FORCES a re-generate:
//!      the sealed probe value folds into the single cache key, so the key
//!      changes and the producer runs again.
//!
//! The producer counts its runs by appending one byte to `counter.side`, so a
//! re-run is observable as a length bump independent of any build-summary
//! wording. The cache is pointed at a private dir via `.cook/cloud.toml` and the
//! probe values materialise under the tmpdir's `.cook/probes/`, so each test is
//! hermetic against the system-wide local backend (`~/.cache/cook/cloud`).

use std::fs;
use std::path::Path;
use std::process::Command;

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

/// Point the artifact cache at a private dir so runs do not collide on keys in
/// the system-wide local backend (`~/.cache/cook/cloud`).
fn write_cloud_toml(wd: &Path, cache_dir: &Path) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

/// Run `cook build` in `wd`. Asserts the invocation succeeded.
fn build(wd: &Path) {
    let out = Command::new(cook_binary())
        .arg("build")
        .current_dir(wd)
        .output()
        .expect("cook invocation");
    assert!(
        out.status.success(),
        "cook build failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Number of times the producer actually ran. The producer appends exactly one
/// byte to `counter.side` per execution, so the byte length is the run count. A
/// cache HIT leaves the file untouched.
fn runs(wd: &Path) -> u64 {
    fs::metadata(wd.join("counter.side"))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Scenario A: a `record` unit's warm hit reuses the recording — the
/// (non-reproducible) producer does NOT run again on a key-stable warm rerun.
#[test]
fn record_warm_hit_does_not_rerun_producer() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_cloud_toml(wd, cache.path());

    fs::write(
        wd.join("Cookfile"),
        r#"recipe build
    cook "out.txt" {
        printf x >> counter.side
        date +%N > out.txt
    } nondet
"#,
    )
    .unwrap();

    // Run 1 (cold): the producer runs, appends one byte, writes the recording.
    build(wd);
    assert_eq!(runs(wd), 1, "run1: record producer must run cold");
    assert!(wd.join("out.txt").exists(), "run1: recording must exist");

    // Run 2 (warm, nothing changed): the key is unchanged, so the existing
    // recording is authoritative — the non-reproducible producer must NOT run.
    build(wd);
    assert_eq!(
        runs(wd),
        1,
        "run2: record warm hit MUST reuse the recording — the producer must not \
         re-run even though its output is non-reproducible"
    );
    assert!(wd.join("out.txt").exists(), "run2: recording still present");
}

/// Scenario B: changing a SEALED determinant re-keys the unit and forces a
/// re-generate, even for a `record` unit. A `knob` probe reads `knob.txt`
/// (declared as an ingredient so a content change re-runs the probe); `seal knob`
/// folds its value into the unit's single cache key.
#[test]
fn record_sealed_determinant_change_regenerates() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_cloud_toml(wd, cache.path());

    fs::write(
        wd.join("Cookfile"),
        r#"probe knob
    ingredients "knob.txt"
    { cat knob.txt }

recipe build
    seal knob
    cook "out.txt" {
        printf x >> counter.side
        date +%N > out.txt
    } nondet
"#,
    )
    .unwrap();

    // Sealed knob = "a".
    fs::write(wd.join("knob.txt"), "a").unwrap();

    // Run 1 (cold): producer runs.
    build(wd);
    assert_eq!(runs(wd), 1, "run1: cold build runs the producer");

    // Run 2 (knob unchanged): the sealed value is stable, so the key is stable —
    // hit, no re-run.
    build(wd);
    assert_eq!(
        runs(wd),
        1,
        "run2: sealed knob unchanged ⇒ key unchanged ⇒ record hit, no re-run"
    );

    // Run 3 (knob = "b"): the probe re-runs (its ingredient changed), its value
    // changes, `seal knob` folds the new value into the key — the key changes, so
    // the record unit MUST re-generate.
    fs::write(wd.join("knob.txt"), "b").unwrap();
    build(wd);
    assert_eq!(
        runs(wd),
        2,
        "run3: changing the sealed knob value MUST re-key and re-generate the \
         record unit — a sealed determinant change is not waivable"
    );
}
