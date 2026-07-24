//! Streaming per-test line rendering, per §3.2 of the test-runner output design.

use crate::test_reporter::style::Style;

/// One-line outcome verb + modifiers, with color applied via `style`.
pub fn outcome_line(
    label: &str,
    verb: Outcome,
    cached: bool,
    should_fail: bool,
    style: &Style,
) -> String {
    let verb_str = match verb {
        Outcome::Ok => style.green("ok"),
        Outcome::Failed => style.bold_red("FAILED"),
        Outcome::Timeout => style.bold_red("TIMEOUT"),
        Outcome::Blocked => style.yellow("BLOCKED"),
    };
    let modifier = match (verb, cached, should_fail) {
        (Outcome::Ok, true, _) => format!(" {}", style.dim("(cached)")),
        (Outcome::Ok, false, true) => format!(" {}", style.dim("(should-fail)")),
        _ => String::new(),
    };
    format!("test {label} ... {verb_str}{modifier}")
}

#[derive(Clone, Copy)]
pub enum Outcome {
    Ok,
    Failed,
    Timeout,
    Blocked,
}

#[cfg(test)]
#[path = "tests/live_tests.rs"]
mod tests;
