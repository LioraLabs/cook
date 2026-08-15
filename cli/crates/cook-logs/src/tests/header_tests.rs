use super::duration_str;

/// The header's duration is `ended_at - started_at` over the RFC-3339
/// timestamps `cook-progress` wrote into `.cook/logs`. Two builds a second
/// apart must read as a second however the calendar falls between them.
#[test]
fn a_build_spanning_a_month_boundary_lasts_as_long_as_it_lasted() {
    assert_eq!(
        duration_str("2026-02-28T23:59:59Z", Some("2026-03-01T00:00:00Z")),
        "1.0s",
        "one second across a short month is still one second"
    );
}

#[test]
fn an_ordinary_same_day_build_is_measured_correctly() {
    assert_eq!(duration_str("2026-05-07T10:00:00Z", Some("2026-05-07T10:00:02Z")), "2.0s");
}

#[test]
fn a_build_with_no_end_is_still_running() {
    assert_eq!(duration_str("2026-05-07T10:00:00Z", None), "(running…)");
}
