//! Integration tests: test-result caching contract.
//!
//! Exercises the three caching invariants:
//! 1. Passing tests are cached on second run.
//! 2. Failing tests are NOT cached.
//! 3. `--rerun` busts the cache.

use std::fs;
use std::process::Command;
use tempfile::tempdir;

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

#[test]
fn passing_test_caches_and_replays() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    test { true }\n",
    )
    .unwrap();

    // First run — primes the cache
    let out1 = Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    assert!(
        !stdout1.contains("cached"),
        "first run should have no cache hits; stdout:\n{stdout1}"
    );

    // Second run — should replay from cache
    let out2 = Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        stdout2.contains("cached"),
        "second run should show cache hit; stdout:\n{stdout2}"
    );
    assert_eq!(
        out2.status.code().unwrap_or(-1),
        0,
        "cached passing test run should still exit 0; stdout:\n{stdout2}"
    );
}

#[test]
fn failing_test_is_not_cached() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    test { false }\n",
    )
    .unwrap();

    // First run
    let _ = Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .output()
        .unwrap();

    // Second run — failed tests must NOT be cached
    let out2 = Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("cached"),
        "failed test must not be cached; stdout:\n{stdout2}"
    );
    assert_ne!(
        out2.status.code().unwrap_or(0),
        0,
        "run with failing test must exit non-zero; stdout:\n{stdout2}"
    );
}

#[test]
fn rerun_busts_cache() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    test { true }\n",
    )
    .unwrap();

    // Prime the cache
    let _ = Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .output()
        .unwrap();

    // --rerun should bypass the cache
    let out2 = Command::new(cook_binary())
        .args(["test", "--rerun"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("cached"),
        "--rerun should bust cache and not show any cache hits; stdout:\n{stdout2}"
    );
    assert_eq!(
        out2.status.code().unwrap_or(-1),
        0,
        "--rerun with passing test should still exit 0; stdout:\n{stdout2}"
    );
}
