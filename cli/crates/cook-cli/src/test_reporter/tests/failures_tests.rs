use super::*;
use cook_engine::{TestId, TestOutcome};
use std::time::Duration;

fn mk_failed(id: &str, stdout: &str, stderr: &str, exit_code: Option<i32>) -> TestResult {
    TestResult {
        id: TestId(id.to_string()),
        namespace: String::new(),
        recipe: id.split(':').next().unwrap_or("").to_string(),
        name: id.split(':').nth(1).unwrap_or("").to_string(),
        iteration_item: None,
        outcome: TestOutcome::Failed,
        duration: Duration::from_millis(23),
        from_cache: false,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        fingerprint: None,
        blocked_by: None,
        should_fail: false,
        timed_out: false,
        line: 5,
        exit_code,
    }
}

fn mk_blocked(id: &str, cause: &str) -> TestResult {
    let mut r = mk_failed(id, "", "", None);
    r.outcome = TestOutcome::Blocked;
    r.blocked_by = Some(cause.to_string());
    r.exit_code = None;
    r
}

#[test]
fn empty_when_no_failures_or_blocked() {
    let s = Style::new(false);
    let out = render(&[], &|id| id.into(), &s);
    assert!(out.is_empty());
}

#[test]
fn renders_failure_with_exit_code_and_duration() {
    let s = Style::new(false);
    let r = mk_failed("recipe:t", "out", "err", Some(1));
        let out = render(&[r], &|id| id.replace(':', "@"), &s);
    assert!(out.contains("---- recipe@t stdout ----"), "{out}");
    assert!(out.contains("out\n"), "{out}");
    assert!(out.contains("---- recipe@t stderr ----"), "{out}");
    assert!(out.contains("err\n"), "{out}");
    assert!(out.contains("exit 1, finished in 23ms"), "{out}");
    assert!(out.contains("\nfailures:\n    recipe@t\n"), "{out}");
}

#[test]
fn empty_streams_print_explicit_marker() {
    let s = Style::new(false);
    let r = mk_failed("r:t", "", "", Some(2));
    let out = render(&[r], &|id| id.into(), &s);
    assert!(out.contains("(empty)\n"), "{out}");
}

#[test]
fn timeout_trailer_uses_seconds() {
    let s = Style::new(false);
    let mut r = mk_failed("r:t", "stdout-line", "", None);
    r.outcome = TestOutcome::TimedOut;
    r.duration = Duration::from_millis(1500);
    r.timed_out = true;
    let out = render(&[r], &|id| id.into(), &s);
    assert!(out.contains("timed out after 1.5s"), "{out}");
}

#[test]
fn blocked_renders_single_line_cause() {
    let s = Style::new(false);
    let r = mk_blocked("r:t", "set -e\nmkdir -p build\nfalse");
    let out = render(&[r], &|id| id.into(), &s);
    assert!(out.contains("blocked by upstream cook step: `mkdir -p build…`"), "{out}");
    assert!(out.contains("\nblocked:\n    r:t\n"), "{out}");
}

#[test]
fn failed_sorted_before_timeout_and_alphabetical_within() {
    let s = Style::new(false);
    let mut t = mk_failed("z:t", "", "", None);
    t.outcome = TestOutcome::TimedOut;
    let f1 = mk_failed("b:t", "", "", Some(1));
    let f2 = mk_failed("a:t", "", "", Some(1));
    let out = render(&[t, f1, f2], &|id| id.into(), &s);
    let pos_a = out.find("---- a:t stdout").unwrap();
    let pos_b = out.find("---- b:t stdout").unwrap();
    let pos_z = out.find("---- z:t stdout").unwrap();
    assert!(pos_a < pos_b, "{out}");
    assert!(pos_b < pos_z, "{out}");
}

#[test]
fn unknown_exit_code_falls_back() {
    let s = Style::new(false);
    let r = mk_failed("r:t", "", "", None);
    let out = render(&[r], &|id| id.into(), &s);
    assert!(out.contains("exit unknown, finished in"), "{out}");
}
