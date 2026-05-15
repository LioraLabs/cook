//! End-to-end integration test for probe units (CS-0074).
//!
//! Builds a temporary Cookfile with a probe + consumer unit, runs `cook`
//! against it twice, and verifies:
//!   1. First run: probe executes, consumer reads its value, build succeeds.
//!   2. Second run: probe cache hit (artifact persists in .cook/cache),
//!      consumer still produces correct output.
//!
//! # Runtime bug (CS-0074, probe-execution path not wired)
//!
//! This test is currently `#[ignore]` because two gaps remain in the
//! probe-execution path that prevent end-to-end runs:
//!
//! 1. **Probe nodes never enter the DAG.**  `cook.probe()` in
//!    `cook-register/src/probe_api.rs` populates only the `ProbeRegistry` /
//!    `RecipeUnits.probes` metadata (used for fingerprinting).  It does NOT
//!    add a `CapturedUnit { payload: WorkPayload::Probe { … } }` to
//!    `capture_state.units`, so probe work never reaches the scheduler.
//!
//! 2. **Consumer `requires` edges are never wired in `build_dag`.**
//!    `CapturedUnit.requires` (set by `cook.add_unit({ requires = {…} })`)
//!    is captured but never read by `cook-engine/src/dag_builder.rs` to
//!    create DAG edges from probe nodes to consumer nodes.
//!
//! The executor and cache plumbing (`WorkPayload::Probe` dispatch,
//! `SharedProbeValueStore`, fingerprint/cache lookup in `executor.rs`)
//! are fully implemented and unit-tested.  What remains is:
//!
//!   a) `probe_api.rs`: also push `CapturedUnit { WorkPayload::Probe }` into
//!      `capture_state.units` after adding to the registry.
//!   b) `dag_builder.rs`: when building the DAG, read `unit.requires` and add
//!      DAG edges from the corresponding probe node to each consumer unit.
//!
//! Once those two wiring pieces land, remove the `#[ignore]` attribute.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // /target/debug/deps  →  /target/debug
    path.pop(); // /target/debug       →  /target
    path.push("cook");
    if !path.exists() {
        panic!(
            "cook binary not found at {} — run `cargo build --bin cook` first",
            path.display()
        );
    }
    path
}

fn run_cook(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let out = Command::new(cook_binary())
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "cook failed (exit={:?}): stdout={}, stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(out)
}

/// End-to-end probe + consumer test.
///
/// Currently ignored because probe nodes are not wired into the DAG
/// (see module-level doc for the two gaps).  Remove `#[ignore]` once
/// probe_api.rs emits WorkPayload::Probe units and dag_builder.rs
/// wires the `requires` edges.
#[test]
fn probe_consumer_end_to_end_first_run_then_cache_hit() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
register
    cook.probe("test:greet", {
        inputs = {},
        produce = "return { word = \"hello-from-probe\" }",
    })
    cook.add_unit({
        name = "echo",
        inputs = {},
        outputs = {"done.marker"},
        requires = {"test:greet"},
        command = "echo $<test:greet.word> > done.marker",
    })

recipe build
    > cook.sh("cat done.marker")
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    // First run.
    let out1 = run_cook(tmp.path(), &["build"]).expect("first run should succeed");
    let marker = fs::read_to_string(tmp.path().join("done.marker"))
        .expect("done.marker should exist after first run");
    assert!(
        marker.contains("hello-from-probe"),
        "marker should contain probe value; got: {:?}\nstdout: {}\nstderr: {}",
        marker,
        String::from_utf8_lossy(&out1.stdout),
        String::from_utf8_lossy(&out1.stderr)
    );

    // Probe artifact should exist in cache.
    let cache_dir = tmp.path().join(".cook/cache");
    let entries: Vec<_> = fs::read_dir(&cache_dir)
        .unwrap_or_else(|_| panic!("cache dir {} missing", cache_dir.display()))
        .filter_map(|e| e.ok())
        .collect();
    assert!(!entries.is_empty(), "expected at least one cache artifact after first run");

    // Second run — should still succeed and produce the same output.
    let _out2 = run_cook(tmp.path(), &["build"]).expect("second run should succeed");
    let marker2 = fs::read_to_string(tmp.path().join("done.marker")).unwrap();
    assert_eq!(marker, marker2, "probe output should be identical on second run (cache hit)");
}
