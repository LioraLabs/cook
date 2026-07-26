use super::*;

fn candidate(key_byte: u8, size: u64, last_access: u64, kind: Option<&str>) -> EvictCandidate {
    let mut key = [0u8; 32];
    key[0] = key_byte;
    EvictCandidate {
        key,
        size,
        last_access,
        kind: kind.map(|s| s.to_string()),
        recipe_namespace: String::new(),
    }
}

fn none_policy() -> EvictPolicy {
    EvictPolicy {
        max_size: None,
        older_than: None,
        low_water: 1.0,
    }
}

#[test]
fn empty_candidates_plan_frees_nothing() {
    let plan = plan_eviction(&[], &none_policy(), 1_000);
    assert_eq!(
        plan,
        EvictPlan {
            victims: vec![],
            freed_bytes: 0,
            total_before: 0,
            total_after: 0,
            count_before: 0,
        }
    );
}

#[test]
fn size_sweep_evicts_the_lru_tail_of_files_only() {
    // A huge exempt object plus three small files; target is set so only
    // the two oldest files need to go to get under budget.
    let exempt = candidate(0x01, 500, 1, Some("probe_value"));
    let f1 = candidate(0x02, 20, 5, None);
    let f2 = candidate(0x03, 20, 15, None);
    let f3 = candidate(0x04, 20, 25, None);
    let candidates = vec![exempt.clone(), f1.clone(), f2.clone(), f3.clone()];

    let policy = EvictPolicy::manual(Some(530), None); // target = 530
    let plan = plan_eviction(&candidates, &policy, 1_000);

    assert_eq!(plan.victims, vec![f1.clone(), f2.clone()]);
    assert!(!plan.victims.contains(&exempt), "exempt kind must never be size-swept");
    assert_eq!(plan.freed_bytes, 40);
    assert_eq!(plan.total_before, 560);
    assert_eq!(plan.total_after, 520);
    assert_eq!(plan.count_before, 4);
}

#[test]
fn size_sweep_stops_at_the_target_not_past_it() {
    let f1 = candidate(0x01, 10, 1, None);
    let f2 = candidate(0x02, 10, 2, None);
    let f3 = candidate(0x03, 10, 3, None);
    let candidates = vec![f1.clone(), f2.clone(), f3.clone()];

    let policy = EvictPolicy::manual(Some(25), None); // target = 25
    let plan = plan_eviction(&candidates, &policy, 1_000);

    // total = 30; evicting f1 alone brings it to 20 (<=25), so the sweep
    // must stop there rather than also taking f2.
    assert_eq!(plan.victims, vec![f1]);
    assert_eq!(plan.total_after, 20);
    assert_eq!(plan.freed_bytes, 10);
}

#[test]
fn exempt_bytes_over_budget_evict_everything_eligible_and_stop() {
    let exempt = candidate(0x01, 1_000, 1, Some("probe_value"));
    let f1 = candidate(0x02, 10, 5, None);
    let candidates = vec![exempt.clone(), f1.clone()];

    let policy = EvictPolicy::manual(Some(5), None); // target = 5, unreachable
    let plan = plan_eviction(&candidates, &policy, 1_000);

    // Everything eligible (f1) is evicted; exempt survives; no panic, and
    // total_after honestly still exceeds max_size.
    assert_eq!(plan.victims, vec![f1]);
    assert!(!plan.victims.contains(&exempt));
    assert_eq!(plan.total_after, 1_000);
    assert!(plan.total_after > policy.max_size.unwrap());
}

#[test]
fn age_sweep_evicts_every_kind_including_exempt_ones() {
    // now=1000, older_than=100s -> cutoff=900.
    let old_exempt = candidate(0x01, 100, 500, Some("probe_value"));
    let young_file = candidate(0x02, 50, 950, None);
    let candidates = vec![old_exempt.clone(), young_file.clone()];

    let policy = EvictPolicy {
        max_size: None,
        older_than: Some(std::time::Duration::from_secs(100)),
        low_water: 1.0,
    };
    let plan = plan_eviction(&candidates, &policy, 1_000);

    assert_eq!(plan.victims, vec![old_exempt]);
    assert_eq!(plan.total_after, 50);
}

#[test]
fn age_cutoff_is_strict_so_an_exactly_at_cutoff_object_survives() {
    // now=1000, older_than=100s -> cutoff=900.
    let at_cutoff = candidate(0x01, 10, 900, None);
    let just_before_cutoff = candidate(0x02, 10, 899, None);
    let candidates = vec![at_cutoff.clone(), just_before_cutoff.clone()];

    let policy = EvictPolicy {
        max_size: None,
        older_than: Some(std::time::Duration::from_secs(100)),
        low_water: 1.0,
    };
    let plan = plan_eviction(&candidates, &policy, 1_000);

    assert!(!plan.victims.contains(&at_cutoff), "exactly-at-cutoff must survive");
    assert!(plan.victims.contains(&just_before_cutoff));
}

#[test]
fn zero_last_access_is_never_an_age_victim_but_is_lru_first() {
    // Age pass: a zero-last_access object must survive even though every
    // other timestamp in existence would look "old" against any cutoff.
    let unknown_mtime = candidate(0x01, 10, 0, None);
    let age_policy = EvictPolicy {
        max_size: None,
        older_than: Some(std::time::Duration::from_secs(1)),
        low_water: 1.0,
    };
    let age_plan = plan_eviction(&[unknown_mtime.clone()], &age_policy, 1_000_000);
    assert!(age_plan.victims.is_empty(), "last_access == 0 must never be an age victim");

    // Size pass: the same object sorts first (evicted before objects with a
    // known, nonzero last_access) because 0 is the smallest u64.
    let known_newer = candidate(0x02, 10, 50, None);
    let size_policy = EvictPolicy::manual(Some(10), None); // target=10, evict exactly one
    let size_plan = plan_eviction(&[known_newer.clone(), unknown_mtime.clone()], &size_policy, 1_000);
    assert_eq!(size_plan.victims, vec![unknown_mtime]);
}

#[test]
fn both_knobs_together_never_double_count_a_victim() {
    // `old_and_big` qualifies for BOTH the age pass and (if it survived) the
    // size pass; it must appear in `victims` exactly once.
    let old_and_big = candidate(0x01, 50, 1, None);
    let candidates = vec![old_and_big.clone()];

    let policy = EvictPolicy {
        max_size: Some(1), // size pass alone would also want to evict it
        older_than: Some(std::time::Duration::from_secs(1)), // now=1000 -> cutoff=999
        low_water: 1.0,
    };
    let plan = plan_eviction(&candidates, &policy, 1_000);

    assert_eq!(plan.victims, vec![old_and_big]);
    assert_eq!(plan.freed_bytes, 50);
}

#[test]
fn both_knobs_together_the_size_pass_still_evicts_after_the_age_pass() {
    // `old` is caught by the age pass alone; `young` survives the age pass
    // but the size pass must still take it to reach `max_size`. Both must
    // appear in `victims` exactly once, and `old` must NOT be re-evicted by
    // the size pass just because it is still non-exempt and un-filtered.
    let old = candidate(0x01, 50, 1, None);
    let young = candidate(0x02, 100, 999, None);
    let candidates = vec![old.clone(), young.clone()];

    // now=1000, older_than=500s -> cutoff=500: catches `old` (last_access=1)
    // but not `young` (last_access=999).
    let policy = EvictPolicy {
        max_size: Some(60), // target=60 (low_water=1.0)
        older_than: Some(std::time::Duration::from_secs(500)),
        low_water: 1.0,
    };
    let plan = plan_eviction(&candidates, &policy, 1_000);

    // Age pass takes `old` (freed 50, running_total 150-50=100). Size pass
    // then still must evict `young` (100 > target 60) to reach budget.
    assert_eq!(plan.victims.len(), 2, "both old and young must be evicted, each exactly once");
    assert_eq!(plan.victims.iter().filter(|c| **c == old).count(), 1, "old must not be double-counted");
    assert_eq!(plan.victims.iter().filter(|c| **c == young).count(), 1, "young must be evicted by the size pass");
    assert_eq!(plan.freed_bytes, 150, "freed_bytes must be 50+100, not 50+50 from a double-counted old");
    assert_eq!(plan.total_after, 0);
}

#[test]
fn age_pass_already_under_budget_means_the_size_pass_evicts_nothing() {
    // now=1000, older_than=50s -> cutoff=950.
    let old = candidate(0x01, 800, 1, None); // age victim
    let young = candidate(0x02, 50, 960, None); // survives age pass

    let candidates = vec![old.clone(), young.clone()];
    let policy = EvictPolicy {
        max_size: Some(100), // target=100; total_before=850, but after age
        // pass frees 800 the running total is 50, already <= 100.
        older_than: Some(std::time::Duration::from_secs(50)),
        low_water: 1.0,
    };
    let plan = plan_eviction(&candidates, &policy, 1_000);

    // A buggy implementation that restarts the size-pass budget from
    // `total_before` (850 > 100) would additionally evict `young`. The
    // correct running total (850 - 800 = 50) is already under target, so
    // `young` must survive.
    assert_eq!(plan.victims, vec![old]);
    assert_eq!(plan.total_after, 50);
}

#[test]
fn low_water_below_one_sweeps_past_max_size() {
    let f1 = candidate(0x01, 40, 1, None);
    let f2 = candidate(0x02, 40, 2, None);
    let f3 = candidate(0x03, 40, 3, None);
    let candidates = vec![f1.clone(), f2.clone(), f3.clone()];

    let policy = EvictPolicy {
        max_size: Some(100),
        older_than: None,
        low_water: DEFAULT_LOW_WATER, // 0.8 -> target = 80
    };
    let plan = plan_eviction(&candidates, &policy, 1_000);

    assert_eq!(plan.victims, vec![f1]);
    assert_eq!(plan.total_after, 80);
    assert!(plan.total_after < policy.max_size.unwrap(), "must sweep below max_size, not just to it");
}

#[test]
fn plan_is_deterministic_under_shuffled_input() {
    // Includes a tie on last_access (0x02 and 0x03 both at 10) to exercise
    // the key-ascending tiebreak.
    let candidates = vec![
        candidate(0x05, 100, 1, None),
        candidate(0x02, 20, 10, None),
        candidate(0x03, 20, 10, None),
        candidate(0x01, 30, 3, Some("probe_value")),
        candidate(0x04, 15, 7, Some("dir")),
    ];
    let mut shuffled = candidates.clone();
    shuffled.reverse();
    shuffled.swap(0, 2);

    let policy = EvictPolicy {
        max_size: Some(50),
        older_than: Some(std::time::Duration::from_secs(5)),
        low_water: 1.0,
    };

    let plan_a = plan_eviction(&candidates, &policy, 1_000);
    let plan_b = plan_eviction(&shuffled, &policy, 1_000);

    assert_eq!(plan_a, plan_b);
    // Sanity: the plan actually did something, so this isn't a vacuous
    // "two empty plans are equal" pass.
    assert!(!plan_a.victims.is_empty());
}

#[test]
fn none_kind_is_treated_as_an_evictable_file() {
    let legacy_file = candidate(0x01, 50, 1, None);
    let policy = EvictPolicy::manual(Some(1), None); // target=1, forces eviction
    let plan = plan_eviction(&[legacy_file.clone()], &policy, 1_000);

    assert_eq!(plan.victims, vec![legacy_file]);
}

#[test]
fn is_size_sweep_exempt_does_not_recognise_an_unknown_kind_string() {
    assert!(!is_size_sweep_exempt(Some("hardlink")));
}

#[test]
fn is_size_sweep_exempt_is_case_sensitive() {
    // "probe_value" is exempt; a case-variant of it must not be.
    assert!(is_size_sweep_exempt(Some("probe_value")));
    assert!(!is_size_sweep_exempt(Some("Probe_Value")));
}

#[test]
fn auto_sets_max_size_none_older_than_and_default_low_water() {
    let policy = EvictPolicy::auto(1_000);
    assert_eq!(policy.max_size, Some(1_000));
    assert_eq!(policy.older_than, None);
    assert_eq!(policy.low_water, DEFAULT_LOW_WATER);
}

#[test]
fn auto_sweep_plans_down_to_low_water_and_no_further() {
    // Above-budget store of plain files (kind: None); auto's low_water=0.8
    // must sweep down to <= 32, but not past what's necessary (total_after
    // must not be driven to zero).
    let f1 = candidate(0x01, 10, 1, None);
    let f2 = candidate(0x02, 10, 2, None);
    let f3 = candidate(0x03, 10, 3, None);
    let f4 = candidate(0x04, 10, 4, None);
    let candidates = vec![f1.clone(), f2.clone(), f3.clone(), f4.clone()];

    let policy = EvictPolicy::auto(40); // target = 32
    let plan = plan_eviction(&candidates, &policy, 1_000);

    assert_eq!(plan.victims, vec![f1]);
    assert!(plan.total_after <= 32, "must sweep to at most 0.8 * max_size");
    assert!(plan.total_after > 0, "must not overshoot past what's necessary");
}

#[test]
fn auto_sweep_over_all_exempt_candidates_evicts_nothing() {
    // Entirely exempt-kind candidates, far above budget: auto must not evict
    // any of them, and must not panic computing a negative/unreachable target.
    let e1 = candidate(0x01, 500, 1, Some("probe_value"));
    let e2 = candidate(0x02, 500, 2, Some("dir"));
    let candidates = vec![e1.clone(), e2.clone()];

    let policy = EvictPolicy::auto(10); // target = 8, unreachable via exempt kinds
    let plan = plan_eviction(&candidates, &policy, 1_000);

    assert!(plan.victims.is_empty());
    assert_eq!(plan.total_after, 1_000);
}

#[test]
fn neither_knob_set_frees_nothing_even_with_real_candidates_present() {
    // `empty_candidates_plan_frees_nothing` only exercises this branch over
    // an empty list, where a no-op implementation would also pass. Pin it
    // over a non-empty candidate list too.
    let candidates = vec![
        candidate(0x01, 100, 1, None),
        candidate(0x02, 200, 2, Some("probe_value")),
        candidate(0x03, 300, 3, None),
    ];
    let plan = plan_eviction(&candidates, &none_policy(), 1_000);

    assert!(plan.victims.is_empty());
    assert_eq!(plan.freed_bytes, 0);
    assert_eq!(plan.total_after, plan.total_before);
    assert_eq!(plan.total_before, 600);
    assert_eq!(plan.count_before, 3);
}
