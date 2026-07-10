//! §19.2 (CS-0124) — exit-code discipline under the recipe runner:
//! a failing `test` step under `cook <recipe>` must fail the run.
//! (`cook test` already exits 1 via cmd_test; this pins the cmd_run path.)

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
fn failing_test_step_under_runner_exits_one() {
    let dir = write_cookfile(
        "recipe failing\n    cook \"out/c.txt\" { echo hi > $<out> }\n    test { false }\n",
    );
    let out = run_recipe(dir.path(), "failing");
    let c = combined(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "failing test step must fail the run with exit 1.\n{c}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("failing test step"),
        "stderr must carry the one-line summary.\n{c}"
    );
}

#[test]
fn passing_test_step_under_runner_exits_zero() {
    let dir = write_cookfile(
        "recipe passing\n    cook \"out/c.txt\" { echo hi > $<out> }\n    test { true }\n",
    );
    let out = run_recipe(dir.path(), "passing");
    let c = combined(&out);
    assert!(out.status.success(), "passing test step must exit 0.\n{c}");
}

#[test]
fn inverted_test_whose_body_fails_as_expected_exits_zero() {
    let dir = write_cookfile(
        "recipe inverted\n    cook \"out/c.txt\" { echo hi > $<out> }\n    test { ! false }\n",
    );
    let out = run_recipe(dir.path(), "inverted");
    let c = combined(&out);
    assert!(
        out.status.success(),
        "satisfied inverted test must exit 0.\n{c}"
    );
}

#[test]
fn skipped_upstream_recipe_reports_skipped() {
    let dir = write_cookfile(
        "recipe counts\n    cook \"counts.txt\" { false }\n\nrecipe report\n    cook \"report.txt\" { cat $<counts> > $<out> }\n",
    );
    let out = run_recipe(dir.path(), "report");
    let c = combined(&out);
    assert_eq!(out.status.code(), Some(1), "upstream failure must exit 1.\n{c}");
    assert!(c.contains("counts"), "failed upstream recipe should be shown.\n{c}");
    assert!(c.contains("FAILED"), "failed upstream recipe should render FAILED.\n{c}");
    assert!(
        c.contains("report") && c.contains("skipped"),
        "dependent recipe should render skipped.\n{c}"
    );
    assert!(
        !c.lines().any(|line| line.contains("report") && line.contains("done")),
        "dependent recipe must not render done.\n{c}"
    );
    assert!(
        c.contains("skipped (upstream-failed)"),
        "node-level upstream skip should still render.\n{c}"
    );
}
