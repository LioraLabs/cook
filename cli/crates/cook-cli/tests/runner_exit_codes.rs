//! §19.2 (CS-0124) — exit-code discipline under the recipe runner:
//! a failing `test` step under `cook <recipe>` must fail the run.
//! (`cook test` already exits 1 via cmd_test; this pins the cmd_run path.)

use std::fs;
use std::path::Path;
use std::process::Command;
use serde_json::Value;
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
    run_cook(dir, &[recipe])
}

fn run_cook(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_bin())
        .args(args)
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
        "recipe failing\n    cook \"out/c.txt\" { echo hi > $<out> }\n    test { false }\n\nrecipe noisy\n    test { true }\n\nchore check: failing noisy\n",
    );
    let out = run_recipe(dir.path(), "check");
    let c = combined(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "failing test step must fail the run with exit 1.\n{c}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("1 failing test step(s): failing:failing_test3"),
        "stderr must name the failing unit in the summary.\n{c}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("rerun: cook test --rerun-failed"),
        "stderr must offer the existing failed-test rerun.\n{c}"
    );
    let rerun = run_cook(dir.path(), &["test", "--rerun-failed"]);
    let rerun_output = combined(&rerun);
    assert_eq!(
        rerun.status.code(),
        Some(1),
        "the hinted rerun command must rerun the failed test.\n{rerun_output}"
    );
    assert!(
        rerun_output.contains("test failing@3 ... FAILED"),
        "the hinted rerun command must name the rerun failing test.\n{rerun_output}"
    );
}

#[test]
fn blocked_test_step_under_runner_is_summarized() {
    let dir = write_cookfile(
        "recipe blocked\n    cook \"out/c.txt\" { false }\n    test { true }\n\nchore check: blocked\n",
    );
    let out = run_cook(dir.path(), &["--output", "json", "check"]);
    let c = combined(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "blocked test must fail the run.\n{c}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("1 failing test step(s): blocked:blocked_test3"),
        "stderr must name blocked tests from task-failure partial results.\n{c}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("rerun: cook test --rerun-failed"),
        "stderr must offer the existing failed-test rerun.\n{c}"
    );
    let diagnostic: Value = serde_json::from_str(
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .expect("JSON diagnostic"),
    )
    .unwrap_or_else(|e| panic!("stderr was not JSON ({e}): {c}"));
    assert_eq!(diagnostic["code"], "command-failed", "diagnostic: {diagnostic}");
    let state: Value = serde_json::from_slice(
        &fs::read(dir.path().join(".cook/test-state.json")).expect("saved test state"),
    )
    .expect("valid test state");
    assert_eq!(state["results"][0]["id"], "blocked:blocked_test3");
}

#[test]
fn passed_partial_test_does_not_hide_a_hard_failure() {
    let dir = write_cookfile(
        "recipe passing\n    test { true }\n\nrecipe broken\n    cook \"out.txt\" { false }\n\nchore check: passing broken\n",
    );
    let out = run_recipe(dir.path(), "check");
    let c = combined(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "an independent hard failure must stay red after a test passes.\n{c}"
    );
    assert!(c.contains("broken"), "the hard failure must remain visible.\n{c}");
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
fn last_failed_selects_a_newer_cook_test_failure() {
    let dir = write_cookfile("recipe old\n    cook \"old.txt\" { false }\n");
    assert_eq!(run_recipe(dir.path(), "old").status.code(), Some(1));

    fs::write(
        dir.path().join("Cookfile"),
        "recipe fresh\n    test { false }\n",
    )
    .unwrap();
    let fresh = run_recipe(dir.path(), "test");
    let fresh_output = combined(&fresh);
    assert_eq!(fresh.status.code(), Some(1), "fresh test must fail.\n{fresh_output}");
    assert!(
        fresh_output.contains("logs:  cook logs --last-failed"),
        "failed tests must advertise their logs.\n{fresh_output}"
    );

    let logs = Command::new(cook_bin())
        .args(["logs", "--last-failed"])
        .current_dir(dir.path())
        .output()
        .expect("read last failed build");
    let output = combined(&logs);
    assert!(logs.status.success(), "logs command failed.\n{output}");
    assert!(output.contains("fresh_test"), "selected stale build.\n{output}");
    assert!(!output.contains("old [Failed]"), "selected stale build.\n{output}");
    assert!(output.contains("exit 1"), "exit status leaked Option debug syntax.\n{output}");
    assert!(!output.contains("Some("), "exit status leaked Option debug syntax.\n{output}");
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
