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

#[test]
fn json_sidecar_includes_line_and_exit_code() {
    let tmp = tempdir().unwrap();
    let mut r = mk("recipe:t", TestOutcome::Failed);
    r.line = 17;
    r.exit_code = Some(2);
    write_json_sidecar(tmp.path(), None, &[r]).unwrap();
    let bytes = std::fs::read(tmp.path().join(".cook/test-report.json")).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let row = &v["tests"][0];
    assert_eq!(row["line"], 17);
    assert_eq!(row["exit_code"], 2);
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
    let globals = crate::cli::Globals::default();
    let r = Reporter::new(&globals);
    assert_eq!(r.label_for("orphan:t"), "orphan:t");
}

#[test]
fn seed_multi_namespace_from_run_plan() {
    let globals = crate::cli::Globals::default();
    let mut r = Reporter::new(&globals);
    r.seed_run_namespaces(&["menugen.check", "api.tests", "web.smoke"]);
    assert!(r.multi_ns);
}

#[test]
fn seed_single_namespace_strips() {
    let globals = crate::cli::Globals::default();
    let mut r = Reporter::new(&globals);
    r.seed_run_namespaces(&["web.smoke", "web.e2e"]);
    assert!(!r.multi_ns);
}

#[test]
fn seed_root_counts_as_a_namespace() {
    // A root recipe (no dot) mixed with an imported one is a
    // multi-namespace run; stripping would collide "check" with
    // "menugen.check".
    let globals = crate::cli::Globals::default();
    let mut r = Reporter::new(&globals);
    r.seed_run_namespaces(&["check", "menugen.check"]);
    assert!(r.multi_ns);
}
