//! The proleptic Gregorian calendar, and the RFC-3339 timestamps Cook writes
//! into `.cook/logs` (COOK-421).
//!
//! A composer and its inverse, which the admission bar names outright.
//! `cook-progress` stamps a build's `started_at` and `ended_at`;
//! `cook-logs` reads them back to show how long the build took;
//! `cook-cli` stamps `ran_at` on its test report and renders a cache blob's
//! mtime as a date. Four surfaces, one calendar.
//!
//! Before this module there were three: `cook-progress` formatted with the
//! `time` crate, `cook-cli` hand-rolled a correct Hinnant civil-from-days,
//! and `cook-logs` hand-rolled a parser that faked the calendar as
//! `(y*365 + m*31 + d)` — so a one-second build spanning 28 February read as
//! 72 hours. The three ends never compared notes because nothing made them.
//!
//! Reading a clock is an effect and stays with the callers. Everything here is
//! a function of its arguments.

/// Days from 1970-01-01 to `(year, month, day)` in the proleptic Gregorian
/// calendar.
///
/// Howard Hinnant's `days_from_civil` (public domain). The exact inverse of
/// [`civil_from_days`], and they are stated together so a change to one is
/// visibly a change to the pair.
///
/// Public although only [`parse_rfc3339_ms`] calls it today. A calendar module
/// that exposes one direction and hides the other is how the next caller ends
/// up writing the missing half somewhere else, which is the whole of what this
/// module was created to undo.
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = if month <= 2 {
        year as i64 - 1
    } else {
        year as i64
    };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // 0..=399
    let m = month as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as u64 - 1; // 0..=365
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // 0..=146096
    era * 146_097 + doe as i64 - 719_468
}

/// `(year, month, day)` for a count of days since 1970-01-01, in the proleptic
/// Gregorian calendar.
///
/// Howard Hinnant's `civil_from_days` (public domain). Moved here from
/// `cook-cli`, which had it right and kept it to itself while `cook-logs`
/// invented a worse one.
pub fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
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

/// Render `epoch_secs` as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// The caller supplies the instant, because reading a clock is an effect.
pub fn format_rfc3339_secs(epoch_secs: u64) -> String {
    let days = (epoch_secs / 86_400) as i64;
    let rem = epoch_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Parse an RFC-3339 UTC timestamp into milliseconds since the Unix epoch, or
/// `None` when it is not one.
///
/// Accepts what Cook writes and what the `time` crate's `Rfc3339` emits for a
/// UTC instant: `YYYY-MM-DDTHH:MM:SS[.fraction]Z`. The fraction is truncated
/// to milliseconds, and a shorter one is padded rather than misread — `.5` is
/// 500ms, not 5.
///
/// Offsets other than `Z` are refused rather than silently read as UTC. Cook
/// only ever writes `Z`, and a timestamp from elsewhere that this function
/// mis-parsed would produce a duration wrong by hours with nothing to show for
/// it.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z').or_else(|| s.strip_suffix('z'))?;
    let (date, time) = s.split_once('T').or_else(|| s.split_once('t'))?;

    let (year, rest) = date.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    let year: i32 = parse_fixed(year, 4)?;
    let month: u32 = parse_fixed(month, 2)?;
    let day: u32 = parse_fixed(day, 2)?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }

    let mut parts = time.splitn(3, ':');
    let hour: i64 = parse_fixed(parts.next()?, 2)?;
    let minute: i64 = parse_fixed(parts.next()?, 2)?;
    let seconds_field = parts.next()?;
    let (second, millis) = match seconds_field.split_once('.') {
        Some((whole, fraction)) => {
            if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let mut ms: i64 = 0;
            for index in 0..3 {
                ms = ms * 10
                    + fraction
                        .as_bytes()
                        .get(index)
                        .map_or(0, |b| i64::from(b - b'0'));
            }
            (parse_fixed::<i64>(whole, 2)?, ms)
        }
        None => (parse_fixed::<i64>(seconds_field, 2)?, 0),
    };
    // 60 admits a leap second, which RFC 3339 permits and which reads as the
    // following instant here rather than being refused outright.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3600 + minute * 60 + second) * 1000 + millis)
}

/// How many days `month` has in `year`.
///
/// The day range has to be checked against the MONTH, not against 31.
/// `days_from_civil` computes a day-of-year and trusts its input, so a
/// `2026-02-31` that only cleared a `1..=31` bound came back as 3 March —
/// three days invented from a corrupt log, which is the small version of the
/// 72-hour error this module exists to kill.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Parse exactly `width` ASCII digits. Rejects signs, spaces and short or long
/// fields, so `2026-5-07` and `2026- 5-07` are refused instead of being read
/// as May.
fn parse_fixed<T: std::str::FromStr>(field: &str, width: usize) -> Option<T> {
    if field.len() != width || !field.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}

#[cfg(test)]
#[path = "tests/timestamp_tests.rs"]
mod tests;
