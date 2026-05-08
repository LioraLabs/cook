//! Terminal test reporter — live event accumulation + final summary block.
//!
//! Per docs/superpowers/specs/2026-05-07-test-runner-design.md §6.5.

pub mod failures;
pub mod label;
pub mod live;
pub mod style;
pub mod summary;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io::IsTerminal;
use cook_engine::{EngineEvent, TestId, TestOutcome, TestResult};
use crate::cli::Cli;

pub struct Reporter {
    started: std::time::Instant,
    verbose: bool,
    style: style::Style,
    namespaces_seen: BTreeSet<String>,
    label_meta: BTreeMap<String, LabelMeta>,
    header_printed: bool,
}

#[derive(Clone)]
struct LabelMeta {
    recipe: String,
    name: String,
    line: u32,
    iteration_item: Option<String>,
}

impl Reporter {
    pub fn new(cli: &Cli) -> Self {
        let no_color_env = std::env::var("NO_COLOR").ok();
        let is_tty = std::io::stdout().is_terminal();
        let colored = style::resolve_color_choice(
            cli.color.as_str(),
            no_color_env.as_deref(),
            is_tty,
        );
        Self {
            started: std::time::Instant::now(),
            verbose: cli.verbose,
            style: style::Style::new(colored),
            namespaces_seen: BTreeSet::new(),
            label_meta: BTreeMap::new(),
            header_printed: false,
        }
    }

    pub fn on_event(&mut self, evt: EngineEvent) {
        match evt {
            EngineEvent::TestStarted { id, recipe, name, line } => {
                if !self.header_printed {
                    println!("{}", self.style.bold("running tests"));
                    self.header_printed = true;
                }
                if id.0.contains('.') {
                    if let Some(ns) = id.0.split('.').next() {
                        self.namespaces_seen.insert(ns.to_string());
                    }
                }
                self.label_meta.insert(id.0.clone(), LabelMeta {
                    recipe: recipe.clone(),
                    name: name.clone(),
                    line,
                    iteration_item: None,
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
        let multi_ns = self.namespaces_seen.len() > 1;
        // Pre-build labels keyed by TestId.0 so the failure renderer doesn't
        // need to reach into self.
        let labels: BTreeMap<String, String> = results.iter()
            .map(|r| {
                let meta = self.label_meta.get(&r.id.0);
                let recipe = meta.map(|m| m.recipe.clone()).unwrap_or_else(|| r.recipe.clone());
                let nm = meta.map(|m| m.name.clone()).unwrap_or_else(|| r.name.clone());
                let ln = meta.map(|m| m.line).unwrap_or(r.line);
                let it = meta.and_then(|m| m.iteration_item.clone()).or(r.iteration_item.clone());
                let lbl = label::label(&recipe, &nm, ln, it.as_deref(), multi_ns);
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
        let multi_ns = self.namespaces_seen.len() > 1;
        match self.label_meta.get(test_id) {
            Some(meta) => label::label(
                &meta.recipe,
                &meta.name,
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
mod tests {
    use super::*;
    use cook_engine::{TestId, TestOutcome, TestResult};
    use std::time::Duration;
    use tempfile::tempdir;

    fn mk(id: &str, outcome: TestOutcome) -> TestResult {
        TestResult {
            id: TestId(id.to_string()),
            namespace: String::new(),
            recipe: id.split(':').next().unwrap_or("").to_string(),
            name: id.split(':').nth(1).unwrap_or("").to_string(),
            suite: String::new(),
            iteration_item: None,
            outcome,
            duration: Duration::from_millis(100),
            from_cache: false,
            stdout: "stdout-line".into(),
            stderr: "stderr-line".into(),
            fingerprint: None,
            blocked_by: None,
            should_fail: false,
            timed_out: false,
            line: 0,
            exit_code: None,
        }
    }

    // ---------------------------------------------------------------------------
    // JSON sidecar
    // ---------------------------------------------------------------------------

    #[test]
    fn json_sidecar_schema_is_v1() {
        let tmp = tempdir().unwrap();
        let results = vec![mk("r:a", TestOutcome::Passed), mk("r:b", TestOutcome::Failed)];
        write_json_sidecar(tmp.path(), None, &results).unwrap();
        let bytes = std::fs::read(tmp.path().join(".cook/test-report.json")).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["summary"]["total"], 2);
        assert_eq!(v["summary"]["passed"], 1);
        assert_eq!(v["summary"]["failed"], 1);
        assert_eq!(v["tests"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn json_sidecar_custom_path() {
        let tmp = tempdir().unwrap();
        let custom = tmp.path().join("out/report.json");
        let results = vec![mk("r:a", TestOutcome::Passed)];
        write_json_sidecar(tmp.path(), Some(&custom), &results).unwrap();
        assert!(custom.exists());
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&custom).unwrap()).unwrap();
        assert_eq!(v["schema_version"], 1);
    }

    #[test]
    fn json_sidecar_outcome_strings() {
        let tmp = tempdir().unwrap();
        let results = vec![
            mk("r:a", TestOutcome::Passed),
            mk("r:b", TestOutcome::Failed),
            mk("r:c", TestOutcome::Blocked),
            mk("r:d", TestOutcome::TimedOut),
        ];
        write_json_sidecar(tmp.path(), None, &results).unwrap();
        let bytes = std::fs::read(tmp.path().join(".cook/test-report.json")).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let tests = v["tests"].as_array().unwrap();
        assert_eq!(tests[0]["outcome"], "passed");
        assert_eq!(tests[1]["outcome"], "failed");
        assert_eq!(tests[2]["outcome"], "blocked");
        assert_eq!(tests[3]["outcome"], "timed_out");
    }

    #[test]
    fn json_sidecar_has_ran_at_timestamp() {
        let tmp = tempdir().unwrap();
        let results = vec![mk("r:a", TestOutcome::Passed)];
        write_json_sidecar(tmp.path(), None, &results).unwrap();
        let bytes = std::fs::read(tmp.path().join(".cook/test-report.json")).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let ran_at = v["ran_at"].as_str().unwrap();
        assert_eq!(ran_at.len(), 20);
        assert!(ran_at.ends_with('Z'));
    }

    // ---------------------------------------------------------------------------
    // JUnit XML sidecar
    // ---------------------------------------------------------------------------

    #[test]
    fn junit_xml_is_well_formed() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("junit.xml");
        let results = vec![
            mk("r:passing", TestOutcome::Passed),
            mk("r:failing", TestOutcome::Failed),
            mk("r:blocked", TestOutcome::Blocked),
            mk("r:timed", TestOutcome::TimedOut),
        ];
        write_junit_sidecar(&path, &results).unwrap();
        let xml = std::fs::read_to_string(&path).unwrap();
        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("<testsuites"));
        assert!(xml.contains("<testcase name=\"passing\""));
        assert!(xml.contains("<failure"));
        assert!(xml.contains("<skipped message=\"blocked"));
        // Well-formed: every open tag has a matching close tag
        let opens = xml.matches("<testsuite ").count();
        let closes = xml.matches("</testsuite>").count();
        assert_eq!(opens, closes);
    }

    #[test]
    fn junit_cdata_safe_handles_close_marker() {
        // A test stdout containing "]]>" must not break the CDATA section.
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("junit.xml");
        let mut r = mk("r:tricky", TestOutcome::Failed);
        r.stdout = "before ]]> after".to_string();
        write_junit_sidecar(&path, &[r]).unwrap();
        let xml = std::fs::read_to_string(&path).unwrap();
        // The literal "]]>" inside CDATA would close it prematurely; we expect
        // the safe replacement so the raw sequence doesn't appear verbatim.
        assert!(!xml.contains("before ]]> after"),
            "unsafe CDATA sequence survived into XML:\n{xml}");
    }

    #[test]
    fn junit_groups_by_recipe() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("junit.xml");
        let results = vec![
            mk("recipe_a:test1", TestOutcome::Passed),
            mk("recipe_a:test2", TestOutcome::Failed),
            mk("recipe_b:test1", TestOutcome::Passed),
        ];
        write_junit_sidecar(&path, &results).unwrap();
        let xml = std::fs::read_to_string(&path).unwrap();
        // Two recipe suites
        assert_eq!(xml.matches("<testsuite ").count(), 2);
        assert_eq!(xml.matches("</testsuite>").count(), 2);
        // recipe_a suite has both tests, recipe_b has one
        assert!(xml.contains("name=\"recipe_a\""));
        assert!(xml.contains("name=\"recipe_b\""));
    }

    #[test]
    fn junit_xml_attr_escaping() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("junit.xml");
        let mut r = mk("r:tricky", TestOutcome::Blocked);
        r.blocked_by = Some("upstream \"failure\" & <build>".to_string());
        write_junit_sidecar(&path, &[r]).unwrap();
        let xml = std::fs::read_to_string(&path).unwrap();
        // The escaped forms must appear in the output
        assert!(xml.contains("&amp;"), "& not escaped: {xml}");
        assert!(xml.contains("&quot;"), "\" not escaped: {xml}");
        assert!(xml.contains("&lt;"), "< not escaped: {xml}");
        // The raw & must not appear outside of entity references
        assert!(!xml.contains(" & "), "raw & survived into XML: {xml}");
    }

    // ---------------------------------------------------------------------------
    // Unit helpers
    // ---------------------------------------------------------------------------

    #[test]
    fn cdata_safe_escapes_close_marker() {
        let safe = cdata_safe("hello ]]> world ]]> end");
        assert!(!safe.contains("]]>") || safe.contains("]]]]><![CDATA[>"),
            "close marker was not escaped: {safe}");
        assert!(safe.contains("]]]]><![CDATA[>"));
    }

    #[test]
    fn xml_escape_attr_escapes_specials() {
        let escaped = xml_escape_attr("a & b < c > d \"e\"");
        assert!(escaped.contains("&amp;"));
        assert!(escaped.contains("&lt;"));
        assert!(escaped.contains("&gt;"));
        assert!(escaped.contains("&quot;"));
        assert!(!escaped.contains('&') || escaped.contains("&amp;"));
    }

    // ---------------------------------------------------------------------------
    // Reporter unit tests
    // ---------------------------------------------------------------------------

    #[test]
    fn reporter_label_for_unknown_id_returns_id() {
        use clap::Parser;
        let cli = Cli::parse_from(["cook", "--test"]);
        let r = Reporter::new(&cli);
        assert_eq!(r.label_for("orphan:t"), "orphan:t");
    }
}
