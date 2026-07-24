//! Terminal test reporter — live event accumulation + final summary block.

pub mod failures;
pub mod label;
pub mod live;
pub mod style;
pub mod summary;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io::IsTerminal;
use cook_engine::{EngineEvent, TestId, TestOutcome, TestResult};
pub struct Reporter {
    started: std::time::Instant,
    verbose: bool,
    style: style::Style,
    multi_ns: bool,
    label_meta: BTreeMap<String, LabelMeta>,
    header_printed: bool,
}

#[derive(Clone)]
struct LabelMeta {
    recipe: String,
    line: u32,
    iteration_item: Option<String>,
}

impl Reporter {
    pub fn new(globals: &crate::cli::Globals) -> Self {
        let no_color_env = std::env::var("NO_COLOR").ok();
        let is_tty = std::io::stdout().is_terminal();
        let colored = style::resolve_color_choice(
            globals.color.as_str(),
            no_color_env.as_deref(),
            is_tty,
        );
        Self {
            started: std::time::Instant::now(),
            verbose: globals.verbose,
            style: style::Style::new(colored),
            multi_ns: false,
            label_meta: BTreeMap::new(),
            header_printed: false,
        }
    }

    /// Fix the single- vs multi-namespace display decision from the run plan,
    /// before any event streams in. Previously the reporter grew a namespace
    /// set incrementally from `TestStarted` events and each outcome line
    /// consulted it mid-stream, so the same workspace printed full names on a
    /// parallel cold run and stripped ones on a fast cached run — and a mixed
    /// run could strip only its earliest lines. Root-namespace recipes (no
    /// dot) count as their own namespace so a root + import mix keeps
    /// prefixes rather than colliding after the strip.
    pub fn seed_run_namespaces<S: AsRef<str>>(&mut self, recipe_names: &[S]) {
        let namespaces: BTreeSet<&str> = recipe_names
            .iter()
            .map(|n| {
                let n = n.as_ref();
                n.find('.').map(|i| &n[..i]).unwrap_or("")
            })
            .collect();
        self.multi_ns = namespaces.len() > 1;
    }

    pub fn on_event(&mut self, evt: EngineEvent) {
        match evt {
            EngineEvent::TestStarted { id, recipe, name: _, line, iteration_item } => {
                if !self.header_printed {
                    println!("{}", self.style.bold("running tests"));
                    self.header_printed = true;
                }
                self.label_meta.insert(id.0.clone(), LabelMeta {
                    recipe: recipe.clone(),
                    line,
                    iteration_item: iteration_item.clone(),
                });
                if self.verbose {
                    println!("    test {} ...", self.label_for(&id.0));
                }
            }
            EngineEvent::TestPassed { id, cached, should_fail, .. } => {
                let lbl = self.label_for(&id.0);
                println!("{}", live::outcome_line(
                    &lbl, live::Outcome::Ok, cached, should_fail, &self.style,
                ));
            }
            EngineEvent::TestFailed { id, .. } => {
                let lbl = self.label_for(&id.0);
                println!("{}", live::outcome_line(
                    &lbl, live::Outcome::Failed, false, false, &self.style,
                ));
            }
            EngineEvent::TestTimedOut { id, .. } => {
                let lbl = self.label_for(&id.0);
                println!("{}", live::outcome_line(
                    &lbl, live::Outcome::Timeout, false, false, &self.style,
                ));
            }
            EngineEvent::TestBlocked { id, .. } => {
                let lbl = self.label_for(&id.0);
                println!("{}", live::outcome_line(
                    &lbl, live::Outcome::Blocked, false, false, &self.style,
                ));
            }
            _ => {}
        }
    }

    pub fn finish(&mut self, results: &[TestResult]) {
        let multi_ns = self.multi_ns;
        // Pre-build labels keyed by TestId.0 so the failure renderer doesn't
        // need to reach into self.
        let labels: BTreeMap<String, String> = results.iter()
            .map(|r| {
                let meta = self.label_meta.get(&r.id.0);
                let recipe = meta.map(|m| m.recipe.clone()).unwrap_or_else(|| r.recipe.clone());
                let ln = meta.map(|m| m.line).unwrap_or(r.line);
                let it = meta.and_then(|m| m.iteration_item.clone()).or(r.iteration_item.clone());
                let lbl = label::label(&recipe, ln, it.as_deref(), multi_ns);
                (r.id.0.clone(), lbl)
            })
            .collect();

        let failure_block = failures::render(
            results,
            &|id| labels.get(id).cloned().unwrap_or_else(|| id.to_string()),
            &self.style,
        );
        if !failure_block.is_empty() {
            print!("{failure_block}");
        }

        // Tally from the authoritative TestResults
        let mut tally = summary::Tally::default();
        for r in results {
            match r.outcome {
                TestOutcome::Passed => tally.passed += 1,
                TestOutcome::Failed => tally.failed += 1,
                TestOutcome::Blocked => tally.blocked += 1,
                TestOutcome::TimedOut => tally.timed_out += 1,
            }
            if r.from_cache {
                tally.cached += 1;
            }
        }

        let summary_line = summary::render(&tally, self.started.elapsed(), &self.style);
        println!();
        println!("{summary_line}");
    }

    fn label_for(&self, test_id: &str) -> String {
        let multi_ns = self.multi_ns;
        match self.label_meta.get(test_id) {
            Some(meta) => label::label(
                &meta.recipe,
                meta.line,
                meta.iteration_item.as_deref(),
                multi_ns,
            ),
            None => test_id.to_string(),
        }
    }
}

fn recipe_of(id: &TestId) -> String {
    let s = &id.0;
    s.split(':').next().unwrap_or("").to_string()
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

struct Summary {
    passed: usize,
    failed: usize,
    blocked: usize,
    timed_out: usize,
    cached: usize,
    wall_secs: f64,
}

fn compute_summary(results: &[TestResult]) -> Summary {
    let mut s = Summary {
        passed: 0,
        failed: 0,
        blocked: 0,
        timed_out: 0,
        cached: 0,
        wall_secs: 0.0,
    };
    for r in results {
        match r.outcome {
            TestOutcome::Passed => s.passed += 1,
            TestOutcome::Failed => s.failed += 1,
            TestOutcome::Blocked => s.blocked += 1,
            TestOutcome::TimedOut => s.timed_out += 1,
        }
        if r.from_cache {
            s.cached += 1;
        }
        s.wall_secs += r.duration.as_secs_f64();
    }
    s
}

fn outcome_str(o: TestOutcome) -> &'static str {
    match o {
        TestOutcome::Passed => "passed",
        TestOutcome::Failed => "failed",
        TestOutcome::Blocked => "blocked",
        TestOutcome::TimedOut => "timed_out",
    }
}

// ---------------------------------------------------------------------------
// §6.3 JSON sidecar (always written)
// ---------------------------------------------------------------------------

/// Write the JSON test report.
///
/// The output path is resolved as (in order of precedence):
/// 1. `report_json_path` argument, if `Some`
/// 2. `<project_root>/.cook/test-report.json`
///
/// Schema version 1 per runner spec §6.3.
pub fn write_json_sidecar(
    project_root: &std::path::Path,
    report_json_path: Option<&std::path::Path>,
    results: &[TestResult],
) -> std::io::Result<()> {
    use serde_json::json;
    use crate::iso8601::now_iso8601;

    let path = report_json_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| project_root.join(".cook/test-report.json"));

    let summary = compute_summary(results);
    let total_duration: f64 = results.iter().map(|r| r.duration.as_secs_f64()).sum();
    let saved_by_cache: f64 = results.iter()
        .filter(|r| r.from_cache)
        .map(|r| r.duration.as_secs_f64())
        .sum();

    let payload = json!({
        "schema_version": 1,
        "cook_version": env!("CARGO_PKG_VERSION"),
        "ran_at": now_iso8601(),
        "duration_secs": total_duration,
        "wall_clock_secs": summary.wall_secs,
        "saved_by_cache_secs": saved_by_cache,
        "summary": {
            "passed": summary.passed,
            "failed": summary.failed,
            "blocked": summary.blocked,
            "timed_out": summary.timed_out,
            "cached": summary.cached,
            "total": results.len(),
        },
        "tests": results.iter().map(|r| json!({
            "id": r.id.0,
            "namespace": r.namespace,
            "recipe": r.recipe,
            "name": r.name,
            "suite": r.suite,
            "iteration_item": r.iteration_item,
            "outcome": outcome_str(r.outcome),
            "duration_secs": r.duration.as_secs_f64(),
            "from_cache": r.from_cache,
            "should_fail": r.should_fail,
            "timed_out": r.timed_out,
            "stdout": r.stdout,
            "stderr": r.stderr,
            "fingerprint": r.fingerprint,
            "line": r.line,
            "exit_code": r.exit_code,
            "blocked_by": r.blocked_by,
        })).collect::<Vec<_>>(),
    });

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&payload)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&path, &bytes)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// §6.4 JUnit XML sidecar (opt-in via --report-junit PATH)
// ---------------------------------------------------------------------------

/// Write a JUnit-compatible XML report to `path`.
///
/// Grouping: one `<testsuite>` per recipe (derived from the test ID prefix
/// before the first `:`). Outcomes map as:
/// - `Passed`   → self-closing `<testcase/>`
/// - `Failed`   → `<testcase><failure .../></testcase>`
/// - `TimedOut` → `<testcase><failure message="timed out" .../></testcase>`
/// - `Blocked`  → `<testcase><skipped .../></testcase>`
pub fn write_junit_sidecar(
    path: &std::path::Path,
    results: &[TestResult],
) -> std::io::Result<()> {
    let mut by_recipe: BTreeMap<String, Vec<&TestResult>> = BTreeMap::new();
    for r in results {
        by_recipe.entry(recipe_of(&r.id)).or_default().push(r);
    }

    let summary = compute_summary(results);
    let total_failures = summary.failed + summary.timed_out;

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuites name=\"cook\" tests=\"{}\" failures=\"{}\" errors=\"0\" time=\"{:.3}\">\n",
        results.len(),
        total_failures,
        summary.wall_secs,
    ));

    for (recipe, tests) in &by_recipe {
        let recipe_failures = tests.iter()
            .filter(|r| matches!(r.outcome, TestOutcome::Failed | TestOutcome::TimedOut))
            .count();
        let recipe_time: f64 = tests.iter().map(|r| r.duration.as_secs_f64()).sum();
        out.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" time=\"{:.3}\">\n",
            xml_escape_attr(recipe),
            tests.len(),
            recipe_failures,
            recipe_time,
        ));

        for r in tests {
            let test_name = if r.name.is_empty() { "(unnamed)" } else { &r.name };
            out.push_str(&format!(
                "    <testcase name=\"{}\" classname=\"{}\" time=\"{:.3}\"",
                xml_escape_attr(test_name),
                xml_escape_attr(recipe),
                r.duration.as_secs_f64(),
            ));
            match r.outcome {
                TestOutcome::Passed => {
                    out.push_str("/>\n");
                }
                TestOutcome::Failed => {
                    out.push_str(">\n");
                    out.push_str("      <failure message=\"test failed\">");
                    out.push_str("<![CDATA[\n");
                    out.push_str(&cdata_safe(&r.stdout));
                    out.push_str("\n");
                    out.push_str(&cdata_safe(&r.stderr));
                    out.push_str("\n]]>");
                    out.push_str("</failure>\n");
                    out.push_str("    </testcase>\n");
                }
                TestOutcome::TimedOut => {
                    out.push_str(">\n");
                    out.push_str("      <failure message=\"timed out\">");
                    out.push_str("<![CDATA[\n");
                    out.push_str(&cdata_safe(&r.stdout));
                    out.push_str("\n");
                    out.push_str(&cdata_safe(&r.stderr));
                    out.push_str("\n]]>");
                    out.push_str("</failure>\n");
                    out.push_str("    </testcase>\n");
                }
                TestOutcome::Blocked => {
                    out.push_str(">\n");
                    let cause = r.blocked_by.as_deref().unwrap_or("upstream cook step");
                    out.push_str(&format!(
                        "      <skipped message=\"blocked by upstream cook failure: {}\"/>\n",
                        xml_escape_attr(cause),
                    ));
                    out.push_str("    </testcase>\n");
                }
            }
        }
        out.push_str("  </testsuite>\n");
    }
    out.push_str("</testsuites>\n");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Escape characters that are not valid in XML attribute values.
fn xml_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Make a string safe for inclusion inside a `<![CDATA[ ... ]]>` section.
///
/// The sequence `]]>` would prematurely close the CDATA section; we split it
/// into two adjacent CDATA sections: `]]]]><![CDATA[>`.
fn cdata_safe(s: &str) -> String {
    s.replace("]]>", "]]]]><![CDATA[>")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/test_reporter_tests.rs"]
mod tests;
