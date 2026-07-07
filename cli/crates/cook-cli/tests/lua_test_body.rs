//! Engine-side execution of Lua-body `test` units (`test >{ ... }`, CS-0127
//! §22.4). Task 3 wired register/codegen so `cook.add_test` accepts
//! `lua_code` XOR `command`; this file locks the worker-pool execution path
//! at the binary level — a lua test body must actually run on the worker VM
//! and its pass/fail/should_fail semantics must match the shell-test path.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn write_cookfile(body: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Cookfile"), body).unwrap();
    dir
}

fn run_recipe(dir: &Path, recipe: &str) -> std::process::Output {
    Command::new(cook_bin())
        .arg(recipe)
        .current_dir(dir)
        // Keep e2e runs out of the shared artifact store.
        .env("COOK_NO_PUBLISH", "1")
        .output()
        .expect("run cook")
}

fn combined(out: &std::process::Output) -> String {
    format!(
        "STDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn lua_body_test_passes() {
    let dir = write_cookfile(
        "recipe check\n    test >{\n        assert(1 + 1 == 2)\n    }\n",
    );
    let out = run_recipe(dir.path(), "check");
    let c = combined(&out);
    assert!(out.status.success(), "a passing lua-body test must exit 0.\n{c}");
}

#[test]
fn lua_body_test_failure_exits_one() {
    let dir = write_cookfile(
        "recipe check\n    test >{\n        error(\"boom\")\n    }\n",
    );
    let out = run_recipe(dir.path(), "check");
    let c = combined(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a failing lua-body test must fail the run with exit 1.\n{c}"
    );
    assert!(
        c.contains("boom"),
        "combined output must surface the lua error text.\n{c}"
    );
}

#[test]
fn lua_body_test_should_fail_inverts() {
    let dir = write_cookfile(
        "recipe check\n    test >{\n        error(\"expected\")\n    } should_fail\n",
    );
    let out = run_recipe(dir.path(), "check");
    let c = combined(&out);
    assert!(
        out.status.success(),
        "a should_fail lua-body test whose body errors must exit 0.\n{c}"
    );
}
