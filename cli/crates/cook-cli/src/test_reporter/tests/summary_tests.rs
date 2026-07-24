use super::*;

fn t(p: usize, f: usize, b: usize, to: usize, c: usize) -> Tally {
    Tally { passed: p, failed: f, blocked: b, timed_out: to, cached: c }
}

#[test]
fn all_pass() {
    let s = Style::new(false);
    let line = render(&t(60, 0, 0, 0, 0), Duration::from_secs_f64(1.1), &s);
    assert_eq!(line, "test result: ok. 60 passed; finished in 1.1s");
}

#[test]
fn all_pass_with_cached_uses_parenthetical() {
    let s = Style::new(false);
    let line = render(&t(60, 0, 0, 0, 1), Duration::from_secs_f64(1.1), &s);
    assert_eq!(
        line,
        "test result: ok. 60 passed (1 cached); finished in 1.1s"
    );
}

#[test]
fn failures_use_full_field_form() {
    let s = Style::new(false);
    let line = render(&t(46, 6, 1, 1, 1), Duration::from_secs_f64(1.1), &s);
    assert!(line.starts_with(
        "test result: FAILED. 46 passed; 6 failed; 1 timed out; 1 blocked; 1 cached; finished in 1.1s"
    ), "{line}");
    assert!(line.contains("rerun: cook test --rerun-failed"), "{line}");
}

#[test]
fn no_tests_ran() {
    let s = Style::new(false);
    let line = render(&t(0, 0, 0, 0, 0), Duration::ZERO, &s);
    assert_eq!(line, "test result: ok. 0 passed; finished in 0.0s");
}

#[test]
fn rerun_hint_only_when_failures() {
    let s = Style::new(false);
    let pass_line = render(&t(1, 0, 0, 0, 0), Duration::from_secs_f64(0.1), &s);
    assert!(!pass_line.contains("rerun:"));
}

#[test]
fn zero_count_fields_omitted_in_failure_case() {
    let s = Style::new(false);
    // 1 failed, no blocked / timed out / cached — those fields must be absent
    let line = render(&t(0, 1, 0, 0, 0), Duration::from_secs_f64(0.1), &s);
    assert!(line.contains("0 passed; 1 failed;"), "{line}");
    assert!(!line.contains("blocked"));
    assert!(!line.contains("timed out"));
    assert!(!line.contains("cached"));
}
