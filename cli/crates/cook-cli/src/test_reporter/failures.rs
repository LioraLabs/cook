//! Failure / blocked detail rendering, per §3.3 of the test-runner output design.

use cook_engine::{TestOutcome, TestResult};
use crate::test_reporter::style::Style;

const STDOUT_STDERR_LINE_CAP: usize = 10_000;

/// Render the full failure-detail block as a single string.
///
/// Caller passes the labels keyed by `TestId.0` so this module stays free
/// of label-formatting policy (which lives in `label.rs`).
pub fn render(
    results: &[TestResult],
    label_for_id: &dyn Fn(&str) -> String,
    style: &Style,
) -> String {
    let mut failed: Vec<&TestResult> = results.iter()
        .filter(|r| matches!(r.outcome, TestOutcome::Failed | TestOutcome::TimedOut))
        .collect();
    let mut blocked: Vec<&TestResult> = results.iter()
        .filter(|r| matches!(r.outcome, TestOutcome::Blocked))
        .collect();
    failed.sort_by(|a, b| {
        sort_key(a).cmp(&sort_key(b))
            .then_with(|| a.id.0.cmp(&b.id.0))
    });
    blocked.sort_by(|a, b| label_for_id(&a.id.0).cmp(&label_for_id(&b.id.0))
        .then_with(|| a.id.0.cmp(&b.id.0)));

    let mut out = String::new();
    if failed.is_empty() && blocked.is_empty() {
        return out;
    }

    if !failed.is_empty() {
        out.push_str(&format!("\n{}\n\n", style.bold_red("failures:")));
        for r in &failed {
            let label = label_for_id(&r.id.0);
            // stdout block
            out.push_str(&format!(
                "{}\n",
                style.dim_cyan(&format!("---- {label} stdout ----"))
            ));
            out.push_str(&format_stream(&r.stdout));
            out.push('\n');
            // stderr block
            out.push_str(&format!(
                "{}\n",
                style.dim_cyan(&format!("---- {label} stderr ----"))
            ));
            out.push_str(&format_stream(&r.stderr));
            out.push('\n');
            // trailer
            let trailer = if matches!(r.outcome, TestOutcome::TimedOut) {
                format!("---- {label} ---- timed out after {:.1}s", r.duration.as_secs_f64())
            } else {
                let ms = r.duration.as_millis();
                let exit = r.exit_code
                    .map(|c| format!("exit {c}"))
                    .unwrap_or_else(|| "exit unknown".to_string());
                format!("---- {label} ---- {exit}, finished in {ms}ms")
            };
            out.push_str(&format!("{}\n\n", style.dim(&trailer)));
        }
    }

    if !blocked.is_empty() {
        out.push_str(&format!("\n{}\n\n", style.bold_yellow("blocked:")));
        for r in &blocked {
            let label = label_for_id(&r.id.0);
            out.push_str(&format!(
                "{}\n",
                style.dim_cyan(&format!("---- {label} ----"))
            ));
            let cause = r.blocked_by.as_deref().unwrap_or("upstream cook step");
            let one_line = single_line(cause);
            out.push_str(&format!(
                "blocked by upstream cook step: `{one_line}`\n\n"
            ));
        }
    }

    // Flat name list at the end
    if !failed.is_empty() {
        out.push_str(&format!("{}\n", style.bold_red("failures:")));
        for r in &failed {
            out.push_str(&format!("    {}\n", style.red(&label_for_id(&r.id.0))));
        }
        out.push('\n');
    }
    if !blocked.is_empty() {
        out.push_str(&format!("{}\n", style.bold_yellow("blocked:")));
        for r in &blocked {
            out.push_str(&format!(
                "    {}\n",
                style.yellow(&label_for_id(&r.id.0))
            ));
        }
        out.push('\n');
    }

    out
}

fn sort_key(r: &TestResult) -> u8 {
    match r.outcome {
        TestOutcome::Failed => 0,
        TestOutcome::TimedOut => 1,
        _ => 2,
    }
}

fn format_stream(s: &str) -> String {
    if s.is_empty() {
        return "(empty)\n".to_string();
    }
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= STDOUT_STDERR_LINE_CAP {
        let mut out = s.to_string();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    } else {
        let head: String = lines.iter().take(STDOUT_STDERR_LINE_CAP).cloned()
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "{head}\n(truncated, see .cook/test-report.json for full output)\n"
        )
    }
}

fn single_line(s: &str) -> String {
    let trimmed = s.trim();
    match trimmed.find('\n') {
        Some(idx) => format!("{}…", &trimmed[..idx]),
        None => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cook_engine::{TestId, TestOutcome};
    use std::time::Duration;

    fn mk_failed(id: &str, stdout: &str, stderr: &str, exit_code: Option<i32>) -> TestResult {
        TestResult {
            id: TestId(id.to_string()),
            namespace: String::new(),
            recipe: id.split(':').next().unwrap_or("").to_string(),
            name: id.split(':').nth(1).unwrap_or("").to_string(),
            suite: String::new(),
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
        assert!(out.contains("blocked by upstream cook step: `set -e…`"), "{out}");
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
}
