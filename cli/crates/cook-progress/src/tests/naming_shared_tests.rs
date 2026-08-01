use super::{probe_group_detail, recipe_did_no_real_work};

#[test]
fn no_real_work_when_everything_was_cached() {
    assert!(recipe_did_no_real_work(3, 0, 3));
    assert!(recipe_did_no_real_work(2, 1, 3));
    assert!(recipe_did_no_real_work(0, 3, 3));
}

#[test]
fn real_work_when_anything_actually_ran() {
    assert!(!recipe_did_no_real_work(2, 0, 3));
    assert!(!recipe_did_no_real_work(0, 0, 1));
}

/// The `>=` rather than `==` is deliberate: a probe can be counted both as a
/// cached node and as a probe that ran, so the sum can exceed the total.
#[test]
fn an_over_count_still_reads_as_no_real_work() {
    assert!(recipe_did_no_real_work(3, 3, 3));
}

#[test]
fn probe_detail_agrees_on_singular_and_plural() {
    assert_eq!(probe_group_detail(1, 0), "(1 probe)");
    assert_eq!(probe_group_detail(2, 0), "(2 probes)");
    assert_eq!(probe_group_detail(0, 1), "(1 probe, 1 cached)");
    assert_eq!(probe_group_detail(3, 1), "(4 probes, 1 cached)");
}
