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

fn write_unique_seed(dir: &std::path::Path) {
    fs::write(dir.join("data.txt"), format!("{}\n", dir.display())).unwrap();
}

#[test]
fn passing_test_caches_and_replays() {
    let tmp = tempdir().unwrap();
    // CS-0135 §17.4: a test caches only when it declares a file source
    // (here `ingredients`), which gives it a cache key. A source-less test
    // is covered by `source_less_test_always_runs` below.
    write_unique_seed(tmp.path());
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    ingredients \"data.txt\"\n    test { true }\n",
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
fn cached_test_replays_its_duration_into_junit() {
    let tmp = tempdir().unwrap();
    write_unique_seed(tmp.path());
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    ingredients \"data.txt\"\n    test { sleep 0.05 }\n",
    )
    .unwrap();
    let report = tmp.path().join("junit.xml");

    assert!(Command::new(cook_binary())
        .args(["test", "--report-junit"])
        .arg(&report)
        .current_dir(tmp.path())
        .status()
        .unwrap()
        .success());
    assert!(Command::new(cook_binary())
        .args(["test", "--report-junit"])
        .arg(&report)
        .current_dir(tmp.path())
        .status()
        .unwrap()
        .success());

    let xml = fs::read_to_string(report).unwrap();
    assert!(!xml.contains("time=\"0.000\""), "{xml}");
    assert!(
        xml.contains("<property name=\"cook.cached\" value=\"true\"/>"),
        "{xml}"
    );
}

#[test]
fn observation_serves_a_machine_without_the_local_index() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("data.txt"),
        format!("{}\n", tmp.path().display()),
    )
    .unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    ingredients \"data.txt\"\n    test { echo ran >> executions.txt }\n",
    )
    .unwrap();

    assert!(Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .status()
        .unwrap()
        .success());
    fs::remove_dir_all(tmp.path().join(".cook/cache")).unwrap();
    assert!(Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .status()
        .unwrap()
        .success());

    assert_eq!(
        fs::read_to_string(tmp.path().join("executions.txt")).unwrap(),
        "ran\n"
    );
}

#[test]
fn a_cached_tests_streams_are_opt_in() {
    // §{exec.cache.observation}: replay MUST NOT be the default for a hit.
    // The duration is a scalar the index lookup already holds and is reported
    // either way (see `cached_test_replays_its_duration_into_junit`); the
    // STREAMS cost a content-addressed fetch per hit, which is the cost the
    // opt-in rule exists to keep off a warm build.
    let tmp = tempdir().unwrap();
    write_unique_seed(tmp.path());
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    ingredients \"data.txt\"\n    test { echo RECORDED-MARKER }\n",
    )
    .unwrap();
    let sidecar = tmp.path().join(".cook/test-report.json");

    // Cold: the test runs, so the marker is genuinely produced this run.
    assert!(Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .status()
        .unwrap()
        .success());
    assert!(
        fs::read_to_string(&sidecar).unwrap().contains("RECORDED-MARKER"),
        "the cold run really did print the marker"
    );

    // Warm, no flag: a hit, and the streams stay unfetched.
    assert!(Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .status()
        .unwrap()
        .success());
    let warm = fs::read_to_string(&sidecar).unwrap();
    assert!(warm.contains("\"from_cache\": true"), "expected a hit: {warm}");
    assert!(
        !warm.contains("RECORDED-MARKER"),
        "a default warm hit must not replay the streams: {warm}"
    );

    // Warm, asked for: the recorded streams come back.
    assert!(Command::new(cook_binary())
        .args(["--replay-logs", "test"])
        .current_dir(tmp.path())
        .status()
        .unwrap()
        .success());
    let replayed = fs::read_to_string(&sidecar).unwrap();
    assert!(
        replayed.contains("\"from_cache\": true"),
        "still expected a hit: {replayed}"
    );
    assert!(
        replayed.contains("RECORDED-MARKER"),
        "--replay-logs must reproduce the recorded streams: {replayed}"
    );
}

#[test]
fn replay_logs_prints_a_cached_cook_units_log_on_request() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("seed.txt"),
        tmp.path().display().to_string(),
    )
    .unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    ingredients \"seed.txt\"\n    cook \"out.txt\" { echo RECORDED-MARKER; touch $<out> }\n",
    )
    .unwrap();
    assert!(Command::new(cook_binary())
        .arg("r")
        .current_dir(tmp.path())
        .status()
        .unwrap()
        .success());
    fs::remove_dir_all(tmp.path().join(".cook/cache")).unwrap();
    fs::remove_file(tmp.path().join("out.txt")).unwrap();

    let replay = Command::new(cook_binary())
        .args(["--replay-logs", "r"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&replay.stdout),
        String::from_utf8_lossy(&replay.stderr)
    );
    assert!(replay.status.success(), "{rendered}");
    assert!(rendered.contains("RECORDED-MARKER"), "{rendered}");
    assert!(rendered.contains("replayed"), "{rendered}");
}

#[test]
fn source_less_test_always_runs() {
    // CS-0135 §8.6.1/§5: a source-less test — no `ingredients`, no upstream
    // `cook` — has no cache key and MUST always run. A stable command-text-only
    // key would be a false green (the true inputs of `cargo test` etc. are
    // opaque to Cook), so such a test is never cached and never shows `(cached)`.
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    test { true }\n",
    )
    .unwrap();

    // First run
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

    // Second run — a source-less test must run AGAIN, never replay from cache
    let out2 = Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("cached"),
        "a source-less test must never cache; second-run stdout:\n{stdout2}"
    );
    assert_eq!(
        out2.status.code().unwrap_or(-1),
        0,
        "source-less passing test should exit 0; stdout:\n{stdout2}"
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
    // Needs a file source so the test actually caches (CS-0135: a source-less
    // test never caches, which would make the --rerun assertion vacuous).
    write_unique_seed(tmp.path());
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    ingredients \"data.txt\"\n    test { true }\n",
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
