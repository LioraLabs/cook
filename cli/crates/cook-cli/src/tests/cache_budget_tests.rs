//! Unit tests for the end-of-run CAS store-budget check (COOK-235).
//!
//! Pure functions only: the two renderers and the largest-contributor fold.
//! Nothing here touches a filesystem, loads a config, or reads a clock —
//! `check_budget_after_run` is the I/O shell around these, and the automatic
//! sweep it drives DELETES FILES, so it is deliberately left to the
//! end-to-end proof against the real binary rather than exercised here.
//!
//! Every assertion below is a whole-string `assert_eq!` rather than a
//! `contains` check, because a subprocess test greps this output verbatim: a
//! silent change to spacing or to the `cook: ` prefix would break that test
//! somewhere far away from here, so it should break here first.

use cook_engine::cook_cache::backend::{EvictCandidate, EvictOutcome};

use super::*;

/// A candidate carrying only the two fields the fold reads (`size`,
/// `recipe_namespace`); `key` / `last_access` / `kind` are eviction-planning
/// inputs and play no part in attribution.
fn candidate(key: u8, namespace: &str, size: u64) -> EvictCandidate {
    EvictCandidate {
        key: [key; 32],
        size,
        last_access: 0,
        kind: None,
        recipe_namespace: namespace.to_string(),
    }
}

// ---------------------------------------------------------------------------
// render_budget_warning
// ---------------------------------------------------------------------------

/// The whole warning block: both sizes, the percentage, the verbatim literal
/// inside a paste-able remediation command, the reclaim delta, and the
/// largest contributor with its own share-of-store percentage.
///
/// Note `20 GB` renders as `20.0 GB`: every size goes through
/// `cache_du::human_size`, so the budget is on exactly the same scale as the
/// total rather than being echoed back as the user typed it. The *command*
/// still echoes the literal verbatim — that is the one place the raw string
/// matters, because the user has to be able to paste it.
#[test]
fn budget_warning_reports_sizes_percentage_command_and_largest() {
    let out = render_budget_warning(
        23_100_000_000,
        20_000_000_000,
        "20GB",
        Some(("/Cookfile::duckdb", 11_000_000_000)),
    );
    assert_eq!(
        out,
        "cook: warning: cache store is 23.1 GB, over the 20.0 GB budget (116%)\n\
         cook:          run `cook cache gc --max-size 20GB` to reclaim 3.1 GB\n\
         cook:          largest: /Cookfile::duckdb 11.0 GB (48%)\n"
    );
}

/// The printed command and the printed number must agree. `cook cache gc
/// --max-size <literal>` sweeps at `low_water = 1.0` — to exactly the budget
/// — so the reclaim figure is `total - budget`, NOT the `total - 0.8 *
/// budget` the automatic sweep would free. Promising the low-water figure
/// next to the exact-budget command would send the user off to run something
/// that reclaims less than the line just told them it would.
#[test]
fn budget_warning_reclaim_matches_what_the_printed_command_would_free() {
    let total = 30_000_000_000u64;
    let budget = 20_000_000_000u64;
    let out = render_budget_warning(total, budget, "20GB", None);

    // total - budget = 10 GB: what `--max-size 20GB` actually frees.
    assert!(
        out.contains("run `cook cache gc --max-size 20GB` to reclaim 10.0 GB"),
        "expected the exact-budget reclaim figure, got:\n{out}"
    );
    // total - 0.8 * budget = 14 GB: the automatic sweep's figure, which must
    // NOT appear next to a command that does not free that much.
    assert!(
        !out.contains("14.0 GB"),
        "the low-water reclaim figure must not be printed with the exact-budget command:\n{out}"
    );
}

/// `recipe_namespace` is `""` whenever no sidecar was readable, and the
/// measured store has an empty `project_id` in the overwhelming majority of
/// its sidecars — so this is the common case, not an edge case. An empty
/// label would render as a bare size with nothing in front of it.
#[test]
fn budget_warning_renders_an_empty_namespace_as_unattributed() {
    let out = render_budget_warning(3_000_000_000, 2_000_000_000, "2GB", Some(("", 1_500_000_000)));
    assert!(
        out.contains("largest: (unattributed) 1.5 GB (50%)"),
        "expected the (unattributed) label, got:\n{out}"
    );
}

/// No candidates means nothing to attribute; the `largest:` line is dropped
/// entirely rather than printed with a placeholder.
#[test]
fn budget_warning_omits_the_largest_line_without_candidates() {
    let out = render_budget_warning(3_000_000_000, 2_000_000_000, "2GB", None);
    assert_eq!(
        out,
        "cook: warning: cache store is 3.0 GB, over the 2.0 GB budget (150%)\n\
         cook:          run `cook cache gc --max-size 2GB` to reclaim 1.0 GB\n"
    );
    assert!(!out.contains("largest"), "no candidates, no largest line:\n{out}");
}

// ---------------------------------------------------------------------------
// render_auto_gc_report
// ---------------------------------------------------------------------------

/// The one-liner a silent multi-gigabyte deletion would otherwise not have:
/// what the store was, what the budget is, what was freed (bytes and object
/// count), and where the store landed.
#[test]
fn auto_gc_report_names_before_budget_freed_and_after() {
    let outcome = EvictOutcome {
        objects: 312,
        bytes: 4_200_000_000,
    };
    let out = render_auto_gc_report(23_100_000_000, 20_000_000_000, &outcome);
    assert_eq!(
        out,
        "cook: cache store was 23.1 GB, over the 20.0 GB budget — \
         swept 4.2 GB (312 objects), now 18.9 GB\n"
    );
}

/// The `now` figure is derived from what the sweep ACTUALLY removed
/// (`outcome.bytes`), not from the plan's projection — same discipline
/// `cache_gc::render_applied` follows. A sweep that removed nothing reports
/// the store unchanged rather than pretending it hit the low-water mark.
#[test]
fn auto_gc_report_derives_after_from_the_outcome() {
    let out = render_auto_gc_report(
        10_000_000_000,
        8_000_000_000,
        &EvictOutcome {
            objects: 0,
            bytes: 0,
        },
    );
    assert!(
        out.contains("swept 0 B (0 objects), now 10.0 GB"),
        "an empty outcome must leave the after-figure equal to the before-figure:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// largest_namespace
// ---------------------------------------------------------------------------

/// Attribution is a fold over the already-enumerated candidates: sizes sum
/// per namespace, and the biggest sum wins — not the single biggest object.
#[test]
fn largest_namespace_sums_per_namespace_rather_than_picking_one_object() {
    let candidates = vec![
        candidate(1, "/Cookfile::duckdb", 3),
        candidate(2, "/Cookfile::duckdb", 4),
        // A single object larger than either duckdb object, but a smaller
        // total: the fold must prefer duckdb's 7.
        candidate(3, "/Cookfile::app", 6),
    ];
    assert_eq!(
        largest_namespace(&candidates),
        Some(("/Cookfile::duckdb", 7))
    );
}

/// Ties break on the namespace name ascending, so the warning text is
/// reproducible across enumeration orders (`LocalBackend::enumerate` walks
/// the store in whatever order the filesystem hands back).
#[test]
fn largest_namespace_breaks_ties_deterministically() {
    let ascending = vec![candidate(1, "aaa", 5), candidate(2, "zzz", 5)];
    let descending = vec![candidate(2, "zzz", 5), candidate(1, "aaa", 5)];
    assert_eq!(largest_namespace(&ascending), Some(("aaa", 5)));
    assert_eq!(largest_namespace(&descending), Some(("aaa", 5)));
}

#[test]
fn largest_namespace_is_none_for_no_candidates() {
    assert_eq!(largest_namespace(&[]), None);
}
