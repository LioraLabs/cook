//! Integration tests for chore parameter binding (COOK-36 Task 4).
//!
//! Exercises the argv → `__cook_params` table plumbing end-to-end:
//!   * Required param supplied: bound as a local in the chore body.
//!   * Defaulted param absent: falls back to the declared default.
//!   * Required param missing: clean diagnostic at invocation time.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // deps → debug (or release)
    path.pop(); // debug → target
    path.push("cook");
    if !path.exists() {
        panic!(
            "cook binary not found at {} — run `cargo build --bin cook` first",
            path.display()
        );
    }
    path
}

fn run_cook_raw(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_binary())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to spawn cook binary")
}

/// `cook greet world` where `chore greet msg` uses `msg` in the body.
/// The body runs `print("hello " .. msg)` and we assert stdout contains
/// "hello world".
#[test]
fn chore_required_param_visible_as_lua_local() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet msg\n    > print(\"hello \" .. msg)\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["greet", "world"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "cook greet world failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("hello world"),
        "expected stdout to contain 'hello world'\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// `cook greet` (no argv) where `chore greet msg="world"` declares a
/// defaulted parameter. The default must bind and the body must print
/// "hello world".
#[test]
fn chore_defaulted_param_falls_back_when_argv_absent() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet msg=\"world\"\n    > print(\"hello \" .. msg)\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["greet"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "cook greet (no argv) failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("hello world"),
        "expected stdout to contain 'hello world'\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// `cook greet` (no argv) where `chore greet msg` declares a *required*
/// parameter. The invocation must fail and stderr must contain the canonical
/// "requires parameter 'msg'" diagnostic.
#[test]
fn chore_missing_required_argv_errors() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet msg\n    > print(msg)\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["greet"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "cook greet (no argv) should have failed\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("requires parameter 'msg'"),
        "expected stderr to contain \"requires parameter 'msg'\"\nstderr: {stderr}"
    );
}
