//! Pure unit tests for `cook cache du`'s aggregation (`summarize`) and
//! rendering (`render`) — no filesystem, no `LocalBackend`. Hand-built
//! `Vec<EvictCandidate>` fixtures exercise every shape the report can take.

use super::*;

fn candidate(kind: Option<&str>, ns: &str, size: u64, last_access: u64) -> EvictCandidate {
    EvictCandidate {
        key: [0u8; 32],
        size,
        last_access,
        kind: kind.map(str::to_string),
        recipe_namespace: ns.to_string(),
    }
}

// ---------------------------------------------------------------------------
// summarize: kind grouping
// ---------------------------------------------------------------------------

#[test]
fn kind_grouping_folds_none_into_file_and_keeps_distinct_kinds_separate() {
    let candidates = vec![
        candidate(None, "ns", 100, 1),
        candidate(None, "ns", 50, 2),
        candidate(Some("probe"), "ns", 10, 3),
    ];
    let report = summarize(candidates);
    assert_eq!(report.by_kind.len(), 2);
    let file_row = report
        .by_kind
        .iter()
        .find(|(label, _, _)| label == "file")
        .expect("None kind must fold into a 'file' row");
    assert_eq!(file_row.1, 150);
    assert_eq!(file_row.2, 2);
    let probe_row = report
        .by_kind
        .iter()
        .find(|(label, _, _)| label == "probe")
        .expect("distinct kind must get its own row");
    assert_eq!(probe_row.1, 10);
    assert_eq!(probe_row.2, 1);
}

#[test]
fn kind_rows_sorted_by_bytes_descending() {
    let candidates = vec![
        candidate(Some("small"), "ns", 10, 1),
        candidate(Some("big"), "ns", 1000, 2),
        candidate(Some("mid"), "ns", 100, 3),
    ];
    let report = summarize(candidates);
    let labels: Vec<&str> = report.by_kind.iter().map(|(l, _, _)| l.as_str()).collect();
    assert_eq!(labels, vec!["big", "mid", "small"]);
}

// ---------------------------------------------------------------------------
// summarize: namespace grouping
// ---------------------------------------------------------------------------

#[test]
fn namespace_grouping_uses_stored_string_verbatim_and_sorts_descending() {
    let candidates = vec![
        candidate(None, "/Cookfile::build", 500, 1),
        candidate(None, "apps.web::test", 2000, 2),
    ];
    let report = summarize(candidates);
    let labels: Vec<&str> = report
        .by_namespace
        .iter()
        .map(|(l, _, _)| l.as_str())
        .collect();
    // Byte-descending: apps.web::test (2000) before /Cookfile::build (500).
    assert_eq!(labels, vec!["apps.web::test", "/Cookfile::build"]);
    // The COOK-333 empty project_id prefix bug is out of scope: the string
    // must appear exactly as stored, no parsing or repair.
    assert!(report
        .by_namespace
        .iter()
        .any(|(l, _, _)| l == "/Cookfile::build"));
}

#[test]
fn empty_namespace_kept_verbatim_in_summarize_render_maps_it_to_no_sidecar() {
    let candidates = vec![candidate(None, "", 100, 1)];
    let report = summarize(candidates);
    assert_eq!(report.by_namespace.len(), 1);
    assert_eq!(report.by_namespace[0].0, "");

    let rendered = render(&report, Path::new("/tmp/store"), None);
    assert!(rendered.contains("<no sidecar>"));
}

#[test]
fn more_than_20_namespaces_renders_top_20_plus_accurate_more_count() {
    let mut candidates = Vec::new();
    for i in 0..23 {
        // Distinct byte counts so descending order is unambiguous and stable.
        candidates.push(candidate(None, &format!("ns{i:02}"), 1000 - i as u64, 1));
    }
    let report = summarize(candidates);
    assert_eq!(report.by_namespace.len(), 23);

    let rendered = render(&report, Path::new("/tmp/store"), None);
    // Top 20 (ns00..ns19, the 20 largest) must appear.
    for i in 0..20 {
        assert!(
            rendered.contains(&format!("ns{i:02}")),
            "expected ns{i:02} in top 20:\n{rendered}"
        );
    }
    // The 3 smallest must be omitted from named rows.
    for i in 20..23 {
        assert!(
            !rendered.contains(&format!("ns{i:02}")),
            "ns{i:02} should have been collapsed into the overflow count:\n{rendered}"
        );
    }
    assert!(
        rendered.contains("… and 3 more"),
        "expected an accurate overflow count:\n{rendered}"
    );
}

// ---------------------------------------------------------------------------
// summarize: oldest / newest
// ---------------------------------------------------------------------------

#[test]
fn oldest_and_newest_pick_true_min_and_max() {
    let candidates = vec![
        candidate(None, "ns", 10, 500),
        candidate(None, "ns", 10, 100),
        candidate(None, "ns", 10, 999),
    ];
    let report = summarize(candidates);
    assert_eq!(report.oldest, Some(100));
    assert_eq!(report.newest, Some(999));
}

#[test]
fn empty_candidates_have_no_oldest_or_newest() {
    let report = summarize(Vec::new());
    assert_eq!(report.oldest, None);
    assert_eq!(report.newest, None);
}

// ---------------------------------------------------------------------------
// render: zero-store shape
// ---------------------------------------------------------------------------

#[test]
fn empty_candidate_list_renders_zero_store_shape() {
    let report = summarize(Vec::new());
    let rendered = render(&report, Path::new("/tmp/store"), None);
    assert!(rendered.contains("Total: 0 B (0 bytes) across 0 objects"));
    assert!(!rendered.contains("By kind:"));
    assert!(!rendered.contains("By namespace:"));
    assert!(!rendered.contains("Oldest:"));
    assert!(!rendered.contains("Newest:"));
    // The sidecar-exclusion disclosure would be noise on an empty store.
    assert!(!rendered.contains("sidecar"));
}

#[test]
fn nonempty_report_names_the_store_path_and_discloses_sidecar_exclusion() {
    let report = summarize(vec![candidate(None, "ns", 100, 1)]);
    let rendered = render(&report, Path::new("/tmp/my-store"), None);
    assert!(rendered.contains("/tmp/my-store"));
    assert!(rendered.contains(".meta.json"));
    assert!(rendered.contains(".provenance.json"));
}

// ---------------------------------------------------------------------------
// render: budget line
// ---------------------------------------------------------------------------

#[test]
fn budget_line_absent_without_a_budget() {
    let report = summarize(vec![candidate(None, "ns", 100, 1)]);
    let rendered = render(&report, Path::new("/tmp/store"), None);
    assert!(!rendered.contains("Budget:"));
}

#[test]
fn budget_line_shows_percentage_under_budget() {
    let report = summarize(vec![candidate(None, "ns", 1_200_000, 1)]);
    let rendered = render(&report, Path::new("/tmp/store"), Some(20_000_000_000));
    assert!(rendered.contains("Budget: 1.2 MB of 20.0 GB (0% used)"), "{rendered}");
    assert!(!rendered.contains("OVER BUDGET"));
}

#[test]
fn budget_line_carries_over_budget_suffix_and_overage() {
    let report = summarize(vec![candidate(None, "ns", 1_500_000_000, 1)]);
    let rendered = render(&report, Path::new("/tmp/store"), Some(1_000_000_000));
    assert!(rendered.contains("OVER BUDGET"), "{rendered}");
    assert!(rendered.contains("500.0 MB"), "expected the overage amount:\n{rendered}");
}

// ---------------------------------------------------------------------------
// human_size formatter
// ---------------------------------------------------------------------------

#[test]
fn human_size_zero() {
    assert_eq!(human_size(0), "0 B");
}

#[test]
fn human_size_sub_kb() {
    assert_eq!(human_size(500), "500 B");
}

#[test]
fn human_size_kb() {
    assert_eq!(human_size(1_500), "1.5 kB");
}

#[test]
fn human_size_mb_matches_the_spec_worked_example() {
    assert_eq!(human_size(1_258_291), "1.3 MB");
}

#[test]
fn human_size_multi_gb() {
    assert_eq!(human_size(20_000_000_000), "20.0 GB");
}

#[test]
fn human_size_tb() {
    assert_eq!(human_size(3_000_000_000_000), "3.0 TB");
}
