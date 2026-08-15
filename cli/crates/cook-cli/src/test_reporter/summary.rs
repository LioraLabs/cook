//! Summary line + footer rendering, per §3.4 of the test-runner output design.

use std::time::Duration;
use crate::test_reporter::style::Style;

#[derive(Default)]
pub struct Tally {
    pub passed: usize,
    pub failed: usize,
    pub blocked: usize,
    pub timed_out: usize,
    pub cached: usize,
}

pub fn render(t: &Tally, wall: Duration, style: &Style) -> String {
    let any_problem = t.failed > 0 || t.blocked > 0 || t.timed_out > 0;
    let verb = if any_problem {
        style.bold_red("FAILED")
    } else {
        style.green("ok")
    };

    let mut parts = vec![format!("{} passed", t.passed)];
    if any_problem {
        if t.failed > 0 { parts.push(format!("{} failed", t.failed)); }
        if t.timed_out > 0 { parts.push(format!("{} timed out", t.timed_out)); }
        if t.blocked > 0 { parts.push(format!("{} blocked", t.blocked)); }
        if t.cached > 0 { parts.push(format!("{} cached", t.cached)); }
        parts.push(format!("finished in {:.1}s", wall.as_secs_f64()));
        let mut line = format!(
            "{} {}. {}",
            style.bold("test result:"),
            verb,
            parts.join("; "),
        );
        line.push_str(&format!(
            "\n\n  {}\n",
            style.dim("rerun: cook test --rerun-failed"),
        ));
        line
    } else {
        // Success: cached is parenthesized after "passed" instead of being its own field
        if t.cached > 0 {
            parts[0] = format!("{} passed ({} cached)", t.passed, t.cached);
        }
        parts.push(format!("finished in {:.1}s", wall.as_secs_f64()));
        format!(
            "{} {}. {}",
            style.bold("test result:"),
            verb,
            parts.join("; "),
        )
    }
}

#[cfg(test)]
#[path = "tests/summary_tests.rs"]
mod tests;
