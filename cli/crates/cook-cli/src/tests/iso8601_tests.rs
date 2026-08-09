use super::*;

#[test]
fn now_iso8601_looks_like_utc_timestamp() {
    let ts = now_iso8601();
    // Basic shape: YYYY-MM-DDTHH:MM:SSZ
    assert_eq!(ts.len(), 20, "unexpected length: {ts}");
    assert!(ts.ends_with('Z'), "must end with Z: {ts}");
    assert_eq!(&ts[4..5], "-");
    assert_eq!(&ts[7..8], "-");
    assert_eq!(&ts[10..11], "T");
}

/// The calendar itself is pinned in `cook_contracts::timestamp`; what is
/// testable here is only that this function reads a real clock and hands it to
/// the shared formatter.
#[test]
fn the_stamp_is_a_real_instant_from_a_real_clock() {
    let ts = now_iso8601();
    let year: i32 = ts[..4].parse().unwrap();
    assert!(year >= 2026, "clock seems wrong: {ts}");
    assert_eq!(
        cook_contracts::timestamp::parse_rfc3339_ms(&ts).is_some(),
        true,
        "the stamp must be one the shared parser reads back: {ts}"
    );
}
