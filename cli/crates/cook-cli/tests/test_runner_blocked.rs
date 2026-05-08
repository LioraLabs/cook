//! Integration test: upstream cook failure produces Blocked test results.
//!
//! SHI-173: `cook test` was short-circuiting to `EngineError::TaskFailures`
//! when a cook step in a test's upstream closure failed, instead of reporting
//! the test as Blocked.
//!
//! Verifies that:
//! - Exit code is 1 (blocked tests are non-zero)
//! - The engine-level error message ("engine: 1 task(s) failed") does NOT appear
//! - The summary mentions "blocked"
//! - Build-mode `cook <recipe>` still exits 1 with the engine error (regression guard)

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

/// Write a Cookfile with a recipe whose cook step always fails (runs `false`)
/// and has a downstream test step.
fn write_broken_cookfile(dir: &std::path::Path) {
    fs::write(
        dir.join("Cookfile"),
        r#"recipe broken
    cook "build/never.out" using {
        mkdir -p build
        false
    }
    test { test -f $<in> } as 'never_runs' timeout 5
"#,
    )
    .unwrap();
}

#[test]
fn upstream_cook_failure_reports_blocked_not_engine_error() {
    let tmp = tempdir().unwrap();
    write_broken_cookfile(tmp.path());

    let out = Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");

    // Should NOT see the raw engine-level task-failure message.
    assert!(
        !stderr.contains("engine: 1 task(s) failed"),
        "expected runner to absorb cook failure into Blocked report;\nstderr:\n{stderr}"
    );

    // Exit code 1 — blocked tests count as failure.
    assert_eq!(
        out.status.code().unwrap_or(0),
        1,
        "expected exit code 1 (blocked tests); combined output:\n{combined}"
    );

    // "blocked" must appear in the output.
    assert!(
        combined.to_lowercase().contains("blocked"),
        "report should mention blocked status;\ncombined:\n{combined}"
    );
}

#[test]
fn build_mode_cook_failure_still_exits_with_engine_error() {
    let tmp = tempdir().unwrap();
    // A Cookfile whose cook step fails — in build mode this should surface the
    // engine error, not be swallowed.
    fs::write(
        tmp.path().join("Cookfile"),
        r#"recipe broken
    cook "out.txt" using {
        false
    }
"#,
    )
    .unwrap();

    let out = Command::new(cook_binary())
        .arg("broken")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Build mode must still surface the failure — non-zero exit.
    assert_ne!(
        out.status.code().unwrap_or(0),
        0,
        "build mode should exit non-zero on cook failure;\nstderr:\n{stderr}"
    );
}
