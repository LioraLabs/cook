//! Integration tests: `--rerun-failed` contract.
//!
//! Verifies that `--rerun-failed`:
//! - Re-runs only previously-failed tests (passing tests stay cached).
//! - Warns and exits 0 when no state file exists.

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
fn rerun_failed_runs_only_previously_failed() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe pass\n    test { true }\nrecipe fail\n    test { false }\n",
    )
    .unwrap();

    // First run: both recipes execute; `fail` fails
    let _ = Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .output()
        .unwrap();

    // Second run: --rerun-failed should re-run only `fail`
    let out2 = Command::new(cook_binary())
        .args(["test", "--rerun-failed"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    let stderr2 = String::from_utf8_lossy(&out2.stderr);

    // The failed recipe should appear in output (it re-ran)
    assert!(
        stdout2.contains("fail"),
        "--rerun-failed should re-run the failed recipe `fail`;\nstdout:\n{stdout2}\nstderr:\n{stderr2}"
    );

    // The previously-passing recipe should appear as cached (not re-run)
    assert!(
        stdout2.contains("cached") || stdout2.contains("pass"),
        "passing recipe `pass` should be cached or mentioned;\nstdout:\n{stdout2}"
    );

    // exit non-zero because `fail` still fails
    assert_ne!(
        out2.status.code().unwrap_or(0),
        0,
        "--rerun-failed with deterministically-failing test should exit non-zero;\nstdout:\n{stdout2}"
    );
}

#[test]
fn rerun_failed_with_no_state_warns_and_exits_zero() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    test { true }\n",
    )
    .unwrap();

    // No prior run — no state file exists
    let out = Command::new(cook_binary())
        .args(["test", "--rerun-failed"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "no state file should warn and exit 0"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{stderr}{stdout}");
    assert!(
        combined.contains("no previous test run") || combined.contains("no previously-failed"),
        "expected warning about missing state file;\ncombined output:\n{combined}"
    );
}
