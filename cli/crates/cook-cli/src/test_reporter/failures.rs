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

/// Reported commands carry codegen's `set -e` prelude; strip it for display.
fn strip_set_e(cmd: &str) -> &str {
    cmd.strip_prefix("set -e\n").unwrap_or(cmd)
}

fn single_line(s: &str) -> String {
    let s = strip_set_e(s);
    let trimmed = s.trim();
    match trimmed.find('\n') {
        Some(idx) => format!("{}…", &trimmed[..idx]),
        None => trimmed.to_string(),
    }
}

#[cfg(test)]
#[path = "tests/failures_tests.rs"]
mod tests;
