//! Small pure render laws every surface must agree on (COOK-392).
//!
//! Duration rendering existed six times across four crates and had
//! drifted: the same 61,500ms rendered "1m01s" (cook why), "1m1s"
//! (cook logs header), "1m1s" via a different branch (plain progress),
//! "1m01s" (event writer), "61.5s" (snapshot), and "61500ms" (logs TUI)
//! in one terminal session. One function now decides it.

/// Render a duration for human surfaces. THE law:
///
/// * `< 1s`  — whole milliseconds (`"7ms"`); sub-second precision is what
///   a cache-hit line lives on.
/// * `< 1m`  — one-decimal seconds (`"1.5s"`).
/// * `< 1h`  — minutes and zero-padded seconds (`"1m01s"`).
/// * `>= 1h` — hours, zero-padded minutes and seconds (`"2h05m09s"`).
pub fn duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else if ms < 3_600_000 {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1000)
    } else {
        format!(
            "{}h{:02}m{:02}s",
            ms / 3_600_000,
            (ms % 3_600_000) / 60_000,
            (ms % 60_000) / 1000
        )
    }
}

/// Lowercase hex, the one non-dependency spelling. Crates that already
/// carry the `hex` crate for DECODING keep it (the "no hand-rolled hex"
/// convention, `cook why` C2); this exists so no-dependency crates stop
/// hand-rolling the encode loop per site.
pub fn lower_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
#[path = "tests/render_tests.rs"]
mod tests;
