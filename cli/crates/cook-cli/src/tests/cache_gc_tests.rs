//! Pure unit tests for `cook cache gc`: rendering (dry-run, applied, empty
//! plan) and argument validation (neither-flag usage error, bad --max-size /
//! --older-than literals). No filesystem, no `LocalBackend` — every `EvictPlan`
//! / `EvictOutcome` here is a hand-built fixture.

use std::path::Path;

use cook_engine::cook_cache::backend::{EvictCandidate, EvictOutcome, EvictPlan};

use super::*;

fn dummy_candidate(n: u8) -> EvictCandidate {
    EvictCandidate {
        key: [n; 32],
        size: 1,
        last_access: 0,
        kind: None,
        recipe_namespace: String::new(),
    }
}

fn plan(
    victim_count: usize,
    freed_bytes: u64,
    total_before: u64,
    total_after: u64,
    count_before: usize,
) -> EvictPlan {
    EvictPlan {
        victims: (0..victim_count).map(|i| dummy_candidate(i as u8)).collect(),
        freed_bytes,
        total_before,
        total_after,
        count_before,
    }
}

// ---------------------------------------------------------------------------
// render_dry_run / render_applied
// ---------------------------------------------------------------------------

#[test]
fn render_dry_run_reports_projection_and_store_transition() {
    let p = plan(3, 4_000_000, 12_000_000, 8_000_000, 17);
    let out = render_dry_run(Path::new("/abs/path"), &p);
    assert_eq!(
        out,
        "Store: /abs/path\n\
         Would free: 3 objects, 4.0 MB\n\
         Store: 12.0 MB -> 8.0 MB (17 objects -> 14)\n"
    );
}

#[test]
fn render_applied_sources_freed_line_from_outcome_not_plan() {
    let p = plan(3, 4_000_000, 12_000_000, 8_000_000, 17);
    // A concurrent sweep already removed one victim: the outcome disagrees
    // with the plan's projection. render_applied must report the OUTCOME's
    // numbers on both the `Freed:` line and the store-transition line, never
    // the plan's `freed_bytes` / `total_after` / victim count.
    let outcome = EvictOutcome {
        objects: 2,
        bytes: 3_000_000,
    };
    let out = render_applied(Path::new("/abs/path"), &p, &outcome);
    assert_eq!(
        out,
        "Store: /abs/path\n\
         Freed: 2 objects, 3.0 MB\n\
         Store: 12.0 MB -> 9.0 MB (17 objects -> 15)\n"
    );
}

#[test]
fn render_dry_run_empty_plan_prints_nothing_to_evict() {
    let p = plan(0, 0, 12_000_000, 12_000_000, 17);
    let out = render_dry_run(Path::new("/abs/path"), &p);
    assert_eq!(out, "Store: /abs/path\nNothing to evict.\n");
}

#[test]
fn render_applied_empty_plan_prints_nothing_to_evict() {
    let p = plan(0, 0, 12_000_000, 12_000_000, 17);
    let out = render_applied(Path::new("/abs/path"), &p, &EvictOutcome::default());
    assert_eq!(out, "Store: /abs/path\nNothing to evict.\n");
}

// ---------------------------------------------------------------------------
// argument validation
// ---------------------------------------------------------------------------

#[test]
fn usage_error_names_both_flags_with_examples() {
    let msg = usage_error().to_string();
    assert!(msg.contains("--max-size"), "message was: {msg}");
    assert!(msg.contains("--older-than"), "message was: {msg}");
}

#[test]
fn bad_max_size_literal_names_the_offending_text() {
    let err = parse_max_size_flag("not-a-size").expect_err("garbage literal must be rejected");
    assert!(err.to_string().contains("not-a-size"));
}

#[test]
fn bad_older_than_literal_names_the_offending_text() {
    let err =
        parse_older_than_flag("not-a-duration").expect_err("garbage literal must be rejected");
    assert!(err.to_string().contains("not-a-duration"));
}

#[test]
fn good_max_size_literal_parses_via_shared_parse_size() {
    assert_eq!(parse_max_size_flag("10GB").unwrap(), 10_000_000_000);
}

#[test]
fn good_older_than_literal_parses_via_humantime() {
    assert_eq!(
        parse_older_than_flag("30d").unwrap(),
        std::time::Duration::from_secs(30 * 86_400)
    );
}
