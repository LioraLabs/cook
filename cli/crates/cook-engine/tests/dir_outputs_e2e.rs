//! CS-0119 end-to-end: a recipe with a directory output (`"out/"`) owns its
//! subtree. On both a cache hit and a rebuild, files not produced by the
//! recipe are swept from the directory.
//!
//! Test 1 — cache-hit stray deletion: a file dropped into the output dir
//! between runs must vanish on the next cache hit (even though the command
//! does not re-execute).
//!
//! Test 2 — rebuild orphan deletion: when gen.sh shrinks from writing two
//! files to one, the file the recipe no longer produces must be deleted after
//! the rebuild.

use std::fs;

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

/// Point the cook cache at a private per-test directory so test runs sharing
/// the same source content / command hash do not collide on artifact keys in
/// the system-wide local backend (`~/.cache/cook/cloud`).
fn write_isolated_cache_config(wd: &std::path::Path, cache_dir: &std::path::Path) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

fn run_cook(wd: &std::path::Path, recipe: &str) -> std::process::Output {
    std::process::Command::new(cook_binary())
        .arg(format!("+{recipe}"))
        .current_dir(wd)
        .output()
        .unwrap_or_else(|e| panic!("cook invocation failed: {e}"))
}

/// A `gen` recipe whose only input is `gen.sh` and whose output is the
/// directory `out/`.  Changing `gen.sh` content forces a rebuild; leaving it
/// unchanged yields a cache hit on the second run.
fn write_cookfile(wd: &std::path::Path) {
    fs::write(
        wd.join("Cookfile"),
        r#"recipe gen
        cook.add_unit({
            inputs  = { "gen.sh" },
            outputs = { "out/" },
            command = "sh gen.sh",
        })
"#,
    )
    .unwrap();
}

// ── Test 1: cache-hit stray deletion ─────────────────────────────────────────

#[test]
fn cache_hit_deletes_strays_from_directory_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_tmp = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_isolated_cache_config(wd, cache_tmp.path());
    write_cookfile(wd);

    // gen.sh v1: produces out/a, out/b, and a date stamp (so we can detect
    // whether the command re-ran on the second invocation).
    fs::write(
        wd.join("gen.sh"),
        b"#!/bin/sh\nmkdir -p out\necho a > out/a\necho b > out/b\ndate > out/stamp\n",
    )
    .unwrap();

    // ── Cold run ──────────────────────────────────────────────────────────────
    let out1 = run_cook(wd, "gen");
    assert!(
        out1.status.success(),
        "cold run failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out1.stdout),
        String::from_utf8_lossy(&out1.stderr),
    );
    assert!(wd.join("out/a").exists(), "cold run must produce out/a");
    assert!(wd.join("out/b").exists(), "cold run must produce out/b");
    assert!(wd.join("out/stamp").exists(), "cold run must produce out/stamp");
    let stamp1 = fs::read(wd.join("out/stamp")).unwrap();

    // Drop a stray file into the output directory.
    fs::write(wd.join("out/STRAY.txt"), b"I am a stray").unwrap();

    // Sleep so that `date` would produce different output if the command ran.
    std::thread::sleep(std::time::Duration::from_secs(1));

    // ── Hot run (cache hit) ───────────────────────────────────────────────────
    let out2 = run_cook(wd, "gen");
    assert!(
        out2.status.success(),
        "hot run failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr),
    );
    // Cook writes progress to stderr; combine both streams for the assertion.
    let combined2 = format!(
        "{}{}",
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(
        combined2.contains("cached"),
        "expected a cache hit on the second run; combined output:\n{combined2}"
    );
    assert!(
        !wd.join("out/STRAY.txt").exists(),
        "stray file must be deleted by the cache-hit reconciliation"
    );
    assert!(wd.join("out/a").exists(), "out/a must survive the cache hit");
    assert!(wd.join("out/b").exists(), "out/b must survive the cache hit");
    // Verify it really was a cache hit (command did not re-run).
    let stamp2 = fs::read(wd.join("out/stamp")).unwrap();
    assert_eq!(
        stamp1, stamp2,
        "stamp changed — the command must not have re-run on a cache hit"
    );
}

// ── Test 2: rebuild orphan deletion ──────────────────────────────────────────

#[test]
fn rebuild_deletes_orphan_from_directory_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_tmp = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_isolated_cache_config(wd, cache_tmp.path());
    write_cookfile(wd);

    // gen.sh v1: produces out/a AND out/b.
    fs::write(
        wd.join("gen.sh"),
        b"#!/bin/sh\nmkdir -p out\necho a > out/a\necho b > out/b\n",
    )
    .unwrap();

    // ── Cold run ──────────────────────────────────────────────────────────────
    let out1 = run_cook(wd, "gen");
    assert!(
        out1.status.success(),
        "cold run failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out1.stdout),
        String::from_utf8_lossy(&out1.stderr),
    );
    assert!(wd.join("out/a").exists(), "cold run must produce out/a");
    assert!(wd.join("out/b").exists(), "cold run must produce out/b");

    // Shrink gen.sh: now only produces out/a.
    // Changing gen.sh content changes its hash → forces a rebuild.
    fs::write(
        wd.join("gen.sh"),
        b"#!/bin/sh\nmkdir -p out\necho a > out/a\n",
    )
    .unwrap();

    // ── Rebuild run ───────────────────────────────────────────────────────────
    let out2 = run_cook(wd, "gen");
    assert!(
        out2.status.success(),
        "rebuild failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr),
    );
    assert!(
        wd.join("out/a").exists(),
        "out/a must exist after rebuild"
    );
    assert!(
        !wd.join("out/b").exists(),
        "out/b is an orphan and must be deleted after the rebuild"
    );
}
