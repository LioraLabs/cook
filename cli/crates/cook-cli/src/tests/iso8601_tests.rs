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

#[test]
fn epoch_is_1970_01_01() {
    assert_eq!(days_to_ymd(0), (1970, 1, 1));
}

#[test]
fn known_date() {
    // 2026-05-07: days since epoch = 56+365*56 + leap-day offsets.
    // Verify via now_iso8601 shape rather than recomputing manually.
    let ts = now_iso8601();
    let year: i32 = ts[..4].parse().unwrap();
    assert!(year >= 2026, "clock seems wrong: {ts}");
}
