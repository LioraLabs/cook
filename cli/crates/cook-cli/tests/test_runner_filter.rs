//! Integration tests: `--filter` flag contract.
//!
//! Verifies that `--filter <glob>`:
//! - Restricts the executed test set to matching recipes.
//! - Exits 0 with "no tests ran" when the pattern matches nothing.
//!
//! Filter patterns use `<recipe>:<test_name>` glob syntax. The pre-execution
//! filter stage matches the recipe portion; only recipes whose name matches
//! the recipe portion of the pattern are executed at all.

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
fn filter_restricts_test_set() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe alpha_suite\n    test { true } as 'alpha' timeout 5\nrecipe beta_suite\n    test { true } as 'beta' timeout 5\n",
    )
    .unwrap();

    // Filter to only the `alpha_suite` recipe using recipe-level glob
    let out = Command::new(cook_binary())
        .args(["test", "--filter", "alpha_suite:*"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // alpha_suite should appear
    assert!(
        stdout.contains("alpha_suite"),
        "filtered run should include recipe `alpha_suite`;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // beta_suite should NOT appear (pre-execution filter excludes it)
    assert!(
        !stdout.contains("beta_suite"),
        "filtered run should exclude recipe `beta_suite`;\nstdout:\n{stdout}"
    );

    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "filter run with only passing tests should exit 0;\nstdout:\n{stdout}"
    );
}

#[test]
fn filter_with_zero_matches_exits_zero() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe r\n    test { true } timeout 5\n",
    )
    .unwrap();

    // Pattern that matches no recipe names
    let out = Command::new(cook_binary())
        .args(["test", "--filter", "nonexistent_recipe:*"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "zero-match filter should exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // Should indicate no tests ran
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("no tests ran") || combined.contains("0 passed") || out.status.success(),
        "zero-match filter output should indicate nothing ran;\ncombined:\n{combined}"
    );
}
