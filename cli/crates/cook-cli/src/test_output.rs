//! Test result types, JUnit XML output, JSON output, and terminal summary.

use std::collections::BTreeMap;
use std::path::Path;

use crate::color::{ColorConfig, Symbols};

// ---------------------------------------------------------------------------
// Test result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TestStatus {
    Pass,
    Fail,
    Error,
    Skip,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TestCaseResult {
    pub name: String,
    pub suite: String,
    pub status: TestStatus,
    pub time: f64,
    pub output: String,
    pub message: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TestSuiteResult {
    pub name: String,
    pub cases: Vec<TestCaseResult>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TestResults {
    pub suites: BTreeMap<String, TestSuiteResult>,
}

#[allow(dead_code)]
impl TestResults {
    pub fn new() -> Self {
        TestResults {
            suites: BTreeMap::new(),
        }
    }

    pub fn add_case(&mut self, suite_name: &str, case: TestCaseResult) {
        let suite = self
            .suites
            .entry(suite_name.to_string())
            .or_insert_with(|| TestSuiteResult {
                name: suite_name.to_string(),
                cases: Vec::new(),
            });
        suite.cases.push(case);
    }

    pub fn total_tests(&self) -> usize {
        self.suites.values().map(|s| s.cases.len()).sum()
    }

    pub fn total_passed(&self) -> usize {
        self.suites
            .values()
            .flat_map(|s| &s.cases)
            .filter(|c| c.status == TestStatus::Pass)
            .count()
    }

    pub fn total_failed(&self) -> usize {
        self.suites
            .values()
            .flat_map(|s| &s.cases)
            .filter(|c| c.status == TestStatus::Fail)
            .count()
    }

    pub fn total_errors(&self) -> usize {
        self.suites
            .values()
            .flat_map(|s| &s.cases)
            .filter(|c| c.status == TestStatus::Error)
            .count()
    }

    pub fn total_skipped(&self) -> usize {
        self.suites
            .values()
            .flat_map(|s| &s.cases)
            .filter(|c| c.status == TestStatus::Skip)
            .count()
    }

    pub fn total_time(&self) -> f64 {
        self.suites
            .values()
            .flat_map(|s| &s.cases)
            .map(|c| c.time)
            .sum()
    }

    pub fn has_failures(&self) -> bool {
        self.total_failed() > 0 || self.total_errors() > 0
    }
}

impl Default for TestResults {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// XML escaping
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// JUnit XML output
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub fn to_junit_xml(results: &TestResults) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!(
        "<testsuites tests=\"{}\" failures=\"{}\" errors=\"{}\" time=\"{:.3}\">\n",
        results.total_tests(),
        results.total_failed(),
        results.total_errors(),
        results.total_time(),
    ));

    for suite in results.suites.values() {
        let tests = suite.cases.len();
        let failures = suite.cases.iter().filter(|c| c.status == TestStatus::Fail).count();
        let errors = suite.cases.iter().filter(|c| c.status == TestStatus::Error).count();
        let time: f64 = suite.cases.iter().map(|c| c.time).sum();

        xml.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"{}\" time=\"{:.3}\">\n",
            xml_escape(&suite.name), tests, failures, errors, time,
        ));

        for case in &suite.cases {
            xml.push_str(&format!(
                "    <testcase name=\"{}\" classname=\"{}\" time=\"{:.3}\"",
                xml_escape(&case.name),
                xml_escape(&case.suite),
                case.time,
            ));

            match case.status {
                TestStatus::Pass => {
                    xml.push_str("/>\n");
                }
                TestStatus::Fail => {
                    xml.push_str(">\n");
                    xml.push_str(&format!(
                        "      <failure message=\"{}\" type=\"TestFailure\">{}</failure>\n",
                        xml_escape(&case.message),
                        xml_escape(&case.output),
                    ));
                    xml.push_str("    </testcase>\n");
                }
                TestStatus::Error => {
                    xml.push_str(">\n");
                    xml.push_str(&format!(
                        "      <error message=\"{}\" type=\"TestError\">{}</error>\n",
                        xml_escape(&case.message),
                        xml_escape(&case.output),
                    ));
                    xml.push_str("    </testcase>\n");
                }
                TestStatus::Skip => {
                    xml.push_str(">\n");
                    xml.push_str(&format!(
                        "      <skipped message=\"{}\"/>\n",
                        xml_escape(&case.message),
                    ));
                    xml.push_str("    </testcase>\n");
                }
            }
        }

        xml.push_str("  </testsuite>\n");
    }

    xml.push_str("</testsuites>\n");
    xml
}

// ---------------------------------------------------------------------------
// JSON output
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[allow(dead_code)]
pub fn to_json(results: &TestResults) -> String {
    let mut json = String::new();
    json.push_str("{\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    json.push_str(&format!("  \"timestamp\":{},\n", ts));

    json.push_str("  \"suites\":[\n");
    let suite_count = results.suites.len();
    for (i, suite) in results.suites.values().enumerate() {
        json.push_str("    {\n");
        json.push_str(&format!("      \"name\":\"{}\",\n", json_escape(&suite.name)));
        json.push_str(&format!("      \"tests\":{},\n", suite.cases.len()));
        let failures = suite.cases.iter().filter(|c| c.status == TestStatus::Fail).count();
        let errors = suite.cases.iter().filter(|c| c.status == TestStatus::Error).count();
        let time: f64 = suite.cases.iter().map(|c| c.time).sum();
        json.push_str(&format!("      \"failures\":{},\n", failures));
        json.push_str(&format!("      \"errors\":{},\n", errors));
        json.push_str(&format!("      \"time\":{:.3},\n", time));
        json.push_str("      \"cases\":[\n");

        for (j, case) in suite.cases.iter().enumerate() {
            let status_str = match case.status {
                TestStatus::Pass => "pass",
                TestStatus::Fail => "fail",
                TestStatus::Skip => "skip",
                TestStatus::Error => "error",
            };
            json.push_str(&format!(
                "        {{\"name\":\"{}\",\"status\":\"{}\",\"time\":{:.3}}}",
                json_escape(&case.name), status_str, case.time,
            ));
            if j + 1 < suite.cases.len() {
                json.push(',');
            }
            json.push('\n');
        }

        json.push_str("      ]\n");
        json.push_str("    }");
        if i + 1 < suite_count {
            json.push(',');
        }
        json.push('\n');
    }
    json.push_str("  ],\n");

    json.push_str(&format!(
        "  \"summary\":{{\"tests\":{},\"passed\":{},\"failed\":{},\"errors\":{},\"skipped\":{},\"time\":{:.3}}}\n",
        results.total_tests(),
        results.total_passed(),
        results.total_failed(),
        results.total_errors(),
        results.total_skipped(),
        results.total_time(),
    ));

    json.push_str("}\n");
    json
}

// ---------------------------------------------------------------------------
// Terminal summary
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub fn format_terminal_summary(results: &TestResults, color: &ColorConfig) -> String {
    let sym = Symbols::new();
    let mut out = String::new();

    out.push_str(&color.bold("Test Results"));
    out.push('\n');

    for suite in results.suites.values() {
        let passed = suite.cases.iter().filter(|c| c.status == TestStatus::Pass).count();
        let failed = suite
            .cases
            .iter()
            .filter(|c| c.status == TestStatus::Fail || c.status == TestStatus::Error)
            .count();
        let time: f64 = suite.cases.iter().map(|c| c.time).sum();

        let name_part = &suite.name;
        let dots = color.dim("\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}");

        let mut parts = Vec::new();
        if passed > 0 {
            parts.push(color.green(&format!("{} passed", passed)));
        }
        if failed > 0 {
            parts.push(color.red(&format!("{} failed", failed)));
        }
        let counts = parts.join(", ");
        let timing = color.dim(&format!("({:.1}s)", time));

        out.push_str(&format!("{} {} {} {}\n", name_part, dots, counts, timing));
    }

    out.push('\n');

    let total_failed = results.total_failed() + results.total_errors();
    let total_passed = results.total_passed();
    let total_time = results.total_time();

    if total_failed == 0 {
        let checkmark = color.green(sym.success);
        let passed_str = color.green(&format!("{} tests passed", total_passed));
        let timing = color.dim(&format!("({:.1}s)", total_time));
        out.push_str(&format!("{} {} {}\n", checkmark, passed_str, timing));
    } else {
        let cross = color.red(sym.failure);
        let passed_str = color.green(&format!("{} passed", total_passed));
        let failed_str = color.red(&format!("{} failed", total_failed));
        let timing = color.dim(&format!("({:.1}s)", total_time));
        out.push_str(&format!("{} {}, {} {}\n", cross, passed_str, failed_str, timing));
    }

    out
}

// ---------------------------------------------------------------------------
// File output
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub fn write_test_results(results: &TestResults, cook_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(cook_dir)?;

    let xml = to_junit_xml(results);
    std::fs::write(cook_dir.join("test-results.xml"), xml)?;

    let json = to_json(results);
    std::fs::write(cook_dir.join("test-results.json"), json)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorMode;

    fn sample_results() -> TestResults {
        let mut results = TestResults::new();
        results.add_case(
            "suite-a",
            TestCaseResult {
                name: "test_pass".into(),
                suite: "suite-a".into(),
                status: TestStatus::Pass,
                time: 0.1,
                output: String::new(),
                message: String::new(),
            },
        );
        results.add_case(
            "suite-a",
            TestCaseResult {
                name: "test_fail".into(),
                suite: "suite-a".into(),
                status: TestStatus::Fail,
                time: 0.5,
                output: "expected 4 got 5".into(),
                message: "assertion failed".into(),
            },
        );
        results.add_case(
            "suite-b",
            TestCaseResult {
                name: "test_skip".into(),
                suite: "suite-b".into(),
                status: TestStatus::Skip,
                time: 0.0,
                output: String::new(),
                message: "not implemented".into(),
            },
        );
        results
    }

    #[test]
    fn test_junit_xml_output() {
        let results = sample_results();
        let xml = to_junit_xml(&results);
        assert!(xml.contains("<?xml version="));
        assert!(xml.contains("<testsuites"));
        assert!(xml.contains(r#"tests="3""#));
        assert!(xml.contains(r#"failures="1""#));
        assert!(xml.contains(r#"errors="0""#));
        assert!(xml.contains(r#"<testsuite name="suite-a""#));
        assert!(xml.contains(r#"<testsuite name="suite-b""#));
        assert!(xml.contains(r#"<testcase name="test_pass""#));
        assert!(xml.contains("<failure"));
        assert!(xml.contains("<skipped"));
    }

    #[test]
    fn test_json_output() {
        let results = sample_results();
        let json = to_json(&results);
        assert!(json.contains(r#""suites""#));
        assert!(json.contains(r#""suite-a""#));
        assert!(json.contains(r#""test_pass""#));
        assert!(json.contains(r#""status":"pass""#));
        assert!(json.contains(r#""status":"fail""#));
        assert!(json.contains(r#""summary""#));
    }

    #[test]
    fn test_terminal_summary() {
        let results = sample_results();
        let color = ColorConfig::resolve(ColorMode::Never, false, false);
        let summary = format_terminal_summary(&results, &color);
        assert!(summary.contains("suite-a"));
        assert!(summary.contains("1 passed"));
        assert!(summary.contains("1 failed"));
        assert!(summary.contains("\u{2717}"));
    }
}
