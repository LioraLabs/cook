//! The one clock read behind Cook's `ran_at` stamps.
//!
//! The calendar and the RFC-3339 rendering are law with several ends and live
//! in `cook_contracts::timestamp` (COOK-421). What is left here is the effect:
//! asking the operating system what time it is.

/// Return the current wall-clock time as a UTC timestamp string in the form
/// `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    cook_contracts::timestamp::format_rfc3339_secs(secs)
}

#[cfg(test)]
#[path = "tests/iso8601_tests.rs"]
mod tests;
