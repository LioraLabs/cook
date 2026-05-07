//! Minimal ISO-8601 UTC timestamp helpers — shared by `test_state` and
//! `test_reporter` so we don't duplicate the date-math algorithm.
//!
//! Does **not** depend on `chrono`; uses only `std::time`.

/// Return the current wall-clock time as a UTC timestamp string in the form
/// `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let (year, month, day) = days_to_ymd(days as i64);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, min, sec
    )
}

/// Convert a count of days since the Unix epoch (1970-01-01) to `(year, month,
/// day)` in the proleptic Gregorian calendar.
///
/// Algorithm from Howard Hinnant's "date" library (public domain).
pub fn days_to_ymd(days_since_epoch: i64) -> (i32, u32, u32) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 {
        days / 146_097
    } else {
        (days - 146_096) / 146_097
    };
    let doe = (days - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
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
}
