//! Pure unit tests for `cook cache gc`: rendering (dry-run, applied, empty
//! plan) and argument validation (neither-flag usage error, bad --max-size /
//! --older-than literals). No filesystem, no `LocalBackend` — every `EvictPlan`
//! / `EvictOutcome` here is a hand-built fixture.

use std::path::{Path, PathBuf};

use cook_engine::cook_cache::backend::{EvictCandidate, EvictOutcome, EvictPlan};
use tempfile::tempdir;

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
    assert!(msg.contains("10GB"), "message was: {msg}");
    assert!(msg.contains("30d"), "message was: {msg}");
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

// ---------------------------------------------------------------------------
// enumerate_store / sweep — the reusable enumerate-plan-apply seam
// ---------------------------------------------------------------------------

/// Write a real blob at the same `{2 hex}/{62 hex}` layout `LocalBackend`
/// uses, so a dry-run / victim-free test can assert against an actual file
/// on disk rather than only the returned `Sweep`.
fn write_blob(store: &Path, key: [u8; 32], size: u64) {
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    let dir = store.join(&hex[..2]);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(&hex[2..]), vec![b'x'; size as usize]).unwrap();
}

fn blob_path(store: &Path, key: [u8; 32]) -> PathBuf {
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    store.join(&hex[..2]).join(&hex[2..])
}

#[test]
fn enumerate_store_on_nonexistent_path_returns_none_and_creates_nothing() {
    let tmp = tempdir().unwrap();
    let store = tmp.path().join("does-not-exist");

    let result = enumerate_store(&store).unwrap();

    assert!(result.is_none());
    assert!(
        !store.exists(),
        "enumerate_store must never create the store as a side effect of checking it"
    );
}

#[test]
fn enumerate_store_on_empty_existing_directory_returns_some_empty_vec() {
    let tmp = tempdir().unwrap();
    let store = tmp.path().join("cas");
    std::fs::create_dir_all(&store).unwrap();

    let result = enumerate_store(&store).unwrap();

    assert_eq!(result, Some(Vec::new()));
}

#[test]
fn sweep_dry_run_returns_none_outcome_and_deletes_nothing_on_disk() {
    let tmp = tempdir().unwrap();
    let store = tmp.path().join("cas");
    std::fs::create_dir_all(&store).unwrap();
    let candidate = dummy_candidate(1);
    write_blob(&store, candidate.key, 1);

    // `max_size: Some(0)` means the size pass WOULD evict the only
    // candidate if this were a real sweep — proving it's `dry_run`, not an
    // empty plan, that suppresses the delete.
    let policy = EvictPolicy::manual(Some(0), None);
    let result = sweep(&store, &[candidate.clone()], &policy, 0, true).unwrap();

    assert_eq!(result.plan.victims.len(), 1, "the policy must have chosen a victim");
    assert!(result.outcome.is_none());
    assert!(
        blob_path(&store, candidate.key).exists(),
        "a dry run must not delete anything on disk"
    );
}

#[test]
fn sweep_with_victim_free_plan_returns_none_outcome_without_calling_apply_eviction() {
    let tmp = tempdir().unwrap();
    // Never created: if `sweep` constructed a `LocalBackend` (which
    // `apply_eviction` requires) it would `create_dir_all` this path as a
    // side effect, so its continued absence is proof `apply_eviction` was
    // never reached.
    let store = tmp.path().join("does-not-exist");
    let candidate = dummy_candidate(1);

    // Neither pass runs (both policy knobs are `None`), so the plan is
    // empty regardless of the candidate.
    let policy = EvictPolicy::manual(None, None);
    let result = sweep(&store, &[candidate], &policy, 0, false).unwrap();

    assert!(result.plan.victims.is_empty());
    assert!(result.outcome.is_none());
    assert!(
        !store.exists(),
        "a victim-free plan must never construct a backend / touch the store"
    );
}
