use super::*;

// ---------------------------------------------------------------------------
// The calendar
// ---------------------------------------------------------------------------

/// Fixed points computed from an independent calendar (Python's `datetime`),
/// not read off a passing run: the epoch and the day before it, both sides of
/// the leap day 2000 had, both sides of the one 1900 did NOT have because of
/// the century rule, and the February 2026 boundary the defect below turned
/// into three days.
///
/// My first draft of this test had the two 1900 entries off by one, and the
/// implementation caught it. That is the right way round, and it is why the
/// values come from elsewhere.
#[test]
fn the_calendar_agrees_with_the_calendar() {
    for (days, ymd) in [
        (0i64, (1970, 1, 1)),
        (-1, (1969, 12, 31)),
        (-719_162, (1, 1, 1)),
        (11_016, (2000, 2, 29)),
        (11_017, (2000, 3, 1)),
        (-25_509, (1900, 2, 28)),
        (-25_508, (1900, 3, 1)),
        (20_512, (2026, 2, 28)),
        (20_513, (2026, 3, 1)),
    ] {
        assert_eq!(civil_from_days(days), ymd, "civil_from_days({days})");
        assert_eq!(
            days_from_civil(ymd.0, ymd.1, ymd.2),
            days,
            "days_from_civil{ymd:?}"
        );
    }
}

/// The two are inverses across a range wide enough to cross leap days, century
/// rules and the epoch. Stated as a round trip because a matched pair of
/// mistakes in the two directions is the way this family of code goes wrong.
#[test]
fn the_two_directions_are_inverses() {
    for days in (-800_000..800_000).step_by(97) {
        let (y, m, d) = civil_from_days(days);
        assert_eq!(days_from_civil(y, m, d), days, "round trip broke at {days}");
    }
}

// ---------------------------------------------------------------------------
// The RFC-3339 parse
// ---------------------------------------------------------------------------

/// The defect this module exists to end: `cook-logs` faked the calendar as
/// `(y*365 + m*31 + d)`, so a one-second build spanning 28 February read as
/// 72 hours.
#[test]
fn a_second_across_a_short_month_is_a_second() {
    let start = parse_rfc3339_ms("2026-02-28T23:59:59Z").expect("parse");
    let end = parse_rfc3339_ms("2026-03-01T00:00:00Z").expect("parse");
    assert_eq!(end - start, 1_000);
}

#[test]
fn the_epoch_is_zero_and_a_second_is_a_thousand() {
    assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01Z"), Some(1_000));
    assert_eq!(parse_rfc3339_ms("1970-01-02T00:00:00Z"), Some(86_400_000));
}

/// A fraction is truncated to milliseconds, and a SHORT one is padded rather
/// than misread. `.5` is half a second; reading it as five milliseconds is the
/// mistake a naive `parse` makes.
#[test]
fn a_fraction_is_read_as_a_fraction_however_many_digits_it_has() {
    let base = parse_rfc3339_ms("2026-05-07T10:00:00Z").expect("parse");
    assert_eq!(parse_rfc3339_ms("2026-05-07T10:00:00.5Z"), Some(base + 500));
    assert_eq!(parse_rfc3339_ms("2026-05-07T10:00:00.05Z"), Some(base + 50));
    assert_eq!(parse_rfc3339_ms("2026-05-07T10:00:00.007Z"), Some(base + 7));
    // Beyond milliseconds is dropped, not rounded.
    assert_eq!(
        parse_rfc3339_ms("2026-05-07T10:00:00.123999Z"),
        Some(base + 123)
    );
}

/// Refused rather than guessed. Each of these would otherwise produce a
/// plausible wrong instant, which is worse than no instant at all: the caller
/// renders "(unknown duration)" for `None` and a confident lie for the rest.
#[test]
fn a_timestamp_outside_the_accepted_shape_is_refused() {
    for bad in [
        "",
        "2026-05-07T10:00:00",       // no zone
        "2026-05-07T10:00:00+02:00", // an offset Cook never writes
        "2026-05-07 10:00:00Z",      // space instead of T
        "2026-5-07T10:00:00Z",       // unpadded month
        "2026-05-07T10:00Z",         // no seconds
        "2026-13-07T10:00:00Z",      // month 13
        "2026-05-32T10:00:00Z",      // day 32
        "2026-05-07T24:00:00Z",      // hour 24
        "2026-05-07T10:60:00Z",      // minute 60
        "2026-05-07T10:00:00.Z",     // empty fraction
        "2026-05-07T10:00:00.abcZ",  // non-digit fraction
        "not-a-timestamp",
    ] {
        assert_eq!(parse_rfc3339_ms(bad), None, "{bad:?} must be refused");
    }
}

/// RFC 3339 permits second 60. Reading it as the following instant is a
/// deliberate choice over refusing the timestamp outright.
#[test]
fn a_leap_second_parses_as_the_instant_after_the_minute() {
    assert_eq!(
        parse_rfc3339_ms("2016-12-31T23:59:60Z"),
        parse_rfc3339_ms("2017-01-01T00:00:00Z")
    );
}

// ---------------------------------------------------------------------------
// Composer and inverse
// ---------------------------------------------------------------------------

/// The pair this module was pulled together for. Every timestamp Cook writes
/// must be one Cook can read back, or a duration silently becomes
/// "(unknown duration)" — or worse, a wrong number.
#[test]
fn everything_the_formatter_writes_the_parser_reads_back() {
    for secs in [
        0u64,
        1,
        86_399,
        86_400,
        951_782_400,
        1_772_323_199,
        4_102_444_800,
    ] {
        let rendered = format_rfc3339_secs(secs);
        assert_eq!(
            parse_rfc3339_ms(&rendered),
            Some(secs as i64 * 1000),
            "{rendered} did not round-trip"
        );
    }
}

#[test]
fn the_formatter_pads_every_field() {
    assert_eq!(format_rfc3339_secs(0), "1970-01-01T00:00:00Z");
    assert_eq!(
        format_rfc3339_secs(951_782_400 + 3661),
        "2000-02-29T01:01:01Z"
    );
}

/// A day past the end of its month is refused, not rolled forward.
///
/// `days_from_civil` computes a day-of-year and trusts its input, so a
/// `1..=31` bound let `2026-02-31` through as 3 March. The rejection list
/// above pins `2026-05-32` — one past the boundary that WAS checked — which is
/// exactly why the gap survived writing that test.
#[test]
fn a_day_past_the_end_of_its_month_is_refused_not_rolled_forward() {
    for bad in [
        "2026-02-29T00:00:00Z", // 2026 is not a leap year
        "2026-02-31T00:00:00Z",
        "2026-04-31T00:00:00Z",
        "2026-06-31T00:00:00Z",
        "2026-09-31T00:00:00Z",
        "2026-11-31T00:00:00Z",
        "1900-02-29T00:00:00Z", // century rule: 1900 was not a leap year
    ] {
        assert_eq!(parse_rfc3339_ms(bad), None, "{bad:?} is not a date");
    }
    // ...and the real ends of those months still parse.
    for good in [
        "2026-02-28T00:00:00Z",
        "2024-02-29T00:00:00Z", // a leap year
        "2000-02-29T00:00:00Z", // divisible by 400
        "2026-04-30T00:00:00Z",
        "2026-01-31T00:00:00Z",
    ] {
        assert!(parse_rfc3339_ms(good).is_some(), "{good:?} is a date");
    }
}
