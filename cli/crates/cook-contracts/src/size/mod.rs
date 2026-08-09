//! Byte-size literals: one grammar, one vocabulary, two ends (COOK-421).
//!
//! A user types a byte budget in two places — `[cache] max_size` in
//! `.cook/cloud.toml`, read by `cook-cache`, and `cook cache gc --max-size`,
//! read by `cook-cli` — and the two MUST accept exactly the same literals.
//! That is the admission bar's own worked example twice over: a pure function
//! more than one crate must agree on, and a shared string constant with two
//! message-formatting ends.
//!
//! It used to live in `cook-cache` with `cook-cli` reaching it as
//! `cook_engine::cook_cache::parse_size` — a re-export through a re-export,
//! which is the mechanical signal that an item is in the wrong crate.

/// COOK-234. The [`parse_size`] accepted-forms vocabulary, shared verbatim
/// between `CloudConfigError::BadMaxSize`'s message (a bad `[cache] max_size`
/// in `.cook/cloud.toml`) and `cook cache gc --max-size`'s flag diagnostic
/// (`cook-cli`'s `cache_gc.rs`). The two call sites name different things —
/// a config field vs. a CLI flag — so they can't share one `Display`
/// message outright, but the list of accepted units is exactly one fact and
/// must not fork into two strings that can drift when a unit is added.
pub const SIZE_LITERAL_HELP: &str =
    "a decimal number with an optional unit suffix (B, KB, MB, GB, TB, KiB, MiB, GiB, TiB)";

/// COOK-232. Parse a byte-count literal.
///
/// Accepts a decimal number (fractional allowed, truncated toward zero)
/// followed by an optional unit suffix, matched case-insensitively, with
/// optional whitespace between the number and the unit. A bare number is
/// bytes. `KB`/`MB`/`GB`/`TB` are powers of 1000; `KIB`/`MIB`/`GIB`/`TIB`
/// are powers of 1024. A leading sign (`+` or `-`) and anything else that
/// doesn't match this shape return `None` — the caller turns that into a
/// diagnostic naming the original literal, worded with
/// [`SIZE_LITERAL_HELP`]. A budget literal never needs an explicit sign, so
/// a value never gets far enough to be tested as negative (which would
/// otherwise let a signed zero like `"-0"` slip through, since `-0.0 < 0.0`
/// is false).
///
/// A value that doesn't fit in `u64` also returns `None` rather than
/// silently saturating to `u64::MAX` — a typo like `"999999999TB"` must
/// not read as "effectively no budget".
pub fn parse_size(literal: &str) -> Option<u64> {
    let s = literal.trim();
    if s.starts_with('-') || s.starts_with('+') {
        return None;
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut saw_digit = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        saw_digit = true;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            saw_digit = true;
        }
    }
    if !saw_digit {
        return None;
    }
    let (num_part, rest) = s.split_at(i);
    let number: f64 = num_part.parse().ok()?;
    let unit = rest.trim_start();
    let multiplier: f64 = if unit.is_empty() {
        1.0
    } else {
        match unit.to_ascii_uppercase().as_str() {
            "B" => 1.0,
            "KB" => 1_000.0,
            "MB" => 1_000_000.0,
            "GB" => 1_000_000_000.0,
            "TB" => 1_000_000_000_000.0,
            "KIB" => 1024.0,
            "MIB" => 1024.0 * 1024.0,
            "GIB" => 1024.0 * 1024.0 * 1024.0,
            "TIB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
            _ => return None,
        }
    };
    let total = number * multiplier;
    if !total.is_finite() {
        return None;
    }
    // `u64::MAX` isn't exactly representable as `f64` (it rounds up to
    // `2^64`), so this comparison correctly rejects anything that would
    // otherwise saturate via the `as` cast below rather than erroring.
    if total >= u64::MAX as f64 {
        return None;
    }
    Some(total as u64)
}

#[cfg(test)]
#[path = "tests/size_tests.rs"]
mod tests;
