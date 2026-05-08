//! Integration tests: workspace discovery + recipe-level scope + JSON sidecar.
//!
//! Exercises `cook test` against a two-Cookfile workspace (root + sub/).
//! Tests run in isolated tempdirs so they don't share cache state.

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

/// Write a two-Cookfile workspace:
/// - root Cookfile imports sub/
/// - sub/Cookfile has `pass` (exits 0) and `fail_one` (exits 1)
fn write_workspace(root: &std::path::Path) {
    fs::write(
        root.join("Cookfile"),
        "import sub ./sub\nrecipe build\n    cook \"build/r.txt\" using { mkdir -p build; printf '' > $<out> }\n",
    )
    .unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(
        root.join("sub/Cookfile"),
        "recipe pass\n    test { true } timeout 5\nrecipe fail_one\n    test { false } timeout 5\n",
    )
    .unwrap();
}

#[test]
fn cook_test_discovers_workspace_recipes() {
    let tmp = tempdir().unwrap();
    write_workspace(tmp.path());

    let out = Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");

    // Engine workspace discovery should pick up the recipes from sub/Cookfile.
    // New label rule strips the namespace prefix when only one namespace is touched,
    // so we look for the bare recipe names (`pass`, `fail_one`) in the streamed
    // `test ... ok|FAILED` lines.
    assert!(
        combined.contains("test pass@") && combined.contains("test fail_one@"),
        "expected sub recipes pass and fail_one in test output; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // exit non-zero because sub.fail_one fails
    assert_ne!(
        out.status.code().unwrap_or(0),
        0,
        "should exit non-zero — sub.fail_one fails; stdout:\n{stdout}"
    );
}

#[test]
fn cook_test_recipe_scope_pass() {
    let tmp = tempdir().unwrap();
    write_workspace(tmp.path());

    let out = Command::new(cook_binary())
        .args(["test", "sub.pass"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "scoped to passing recipe sub.pass should exit 0;\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn cook_test_recipe_scope_fail() {
    let tmp = tempdir().unwrap();
    write_workspace(tmp.path());

    let out = Command::new(cook_binary())
        .args(["test", "sub.fail_one"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_ne!(
        out.status.code().unwrap_or(0),
        0,
        "scoped to failing recipe sub.fail_one should exit non-zero;\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn cook_test_writes_json_sidecar() {
    let tmp = tempdir().unwrap();
    write_workspace(tmp.path());

    // Run (may fail — we only care that the sidecar is written)
    let _ = Command::new(cook_binary())
        .arg("test")
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let report = tmp.path().join(".cook/test-report.json");
    assert!(
        report.exists(),
        "JSON sidecar not written at .cook/test-report.json"
    );

    let bytes = fs::read(&report).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes)
        .expect("JSON sidecar must be valid JSON");

    assert_eq!(
        v["schema_version"],
        serde_json::json!(1),
        "schema_version must be 1; got: {}",
        v["schema_version"]
    );
    assert!(
        v["summary"]["total"].as_u64().unwrap_or(0) >= 2,
        "expected at least 2 total tests (sub.pass + sub.fail_one); got: {}",
        v["summary"]["total"]
    );
}
