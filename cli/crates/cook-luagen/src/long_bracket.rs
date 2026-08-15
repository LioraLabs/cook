//! Long-bracket wrapping: `[[ … ]]` at a level no inner close can match.
//!
//! Separate from the short-literal law, which is `cook_contracts::lua_string`
//! and is called there by its own name (COOK-398, COOK-440). The split is the
//! stratum rule: what a `"…"` literal must escape is a rule this crate and
//! `cook-register` must agree on, while choosing a bracket level is a lowering
//! choice with one emitter and no counterpart to disagree with. This module
//! was called `lua_string` until COOK-440, which left it holding only the
//! long-bracket half and a name that collided with the contract it no longer
//! implemented.

/// Pick a long-bracket level high enough to safely wrap `s`.
///
/// A Lua long-bracket wrapper is `[<=>{n}[ ... ]<=>{n}]` for some n >= 0,
/// where `<=>{n}` denotes a run of exactly `n` equals signs. The closing
/// bracket only matches an opener of the same level, so the wrapper is safe
/// iff `s` contains no substring of the form `]<=>{n}]` (an inner closing
/// bracket of the same level).
///
/// Algorithm: scan `s` for every substring matching `]=*]` and record the
/// length of the equals run. The chosen level is `max(run) + 1`, which
/// guarantees no inner close can match. If `s` contains no `]…]` pattern
/// at all we use level 0 (`[[ … ]]`).
///
/// We accept overlapping matches (e.g. `]]]` contains both `]]` at offset 0
/// and `]]` at offset 1, both length-0 runs) by always continuing the scan
/// from the position after the opening `]`, not after the closing one.
fn pick_long_bracket_level(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut max_run: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b']' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] == b'=' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b']' {
                let run = j - i - 1;
                max_run = Some(max_run.map_or(run, |m| m.max(run)));
            }
        }
        i += 1;
    }
    max_run.map_or(0, |m| m + 1)
}

/// Wrap `s` in a Lua long-string literal, picking the smallest safe level.
///
/// The closing bracket sits immediately after `s`, so the scan runs over
/// `s` with a sentinel `]` appended (COOK-489). Without it a content
/// suffix of `]` followed by exactly `level` equals signs pairs with the
/// closer's leading `]` to form a matching close, and the literal ends
/// early: `[ -f x ]` wrapped at level 0 is `[[[ -f x ]]]`, which Lua reads
/// as the string `[ -f x ` plus a stray `]`. The sentinel makes any such
/// suffix a run the level must exceed, so the pairing can never happen.
pub(crate) fn wrap_lua_string(s: &str) -> String {
    let level = pick_long_bracket_level(&format!("{s}]"));
    let eq = "=".repeat(level);
    format!("[{eq}[{s}]{eq}]")
}

/// Wrap `code` in a Lua long-string literal suitable for embedding a Lua
/// chunk (plate/test bodies). Adds the surrounding newlines that Lua eats
/// after `[…[` so the body's first line is preserved verbatim, then picks
/// a long-bracket level high enough to contain any `]=*]` runs in `code`.
///
/// This form needs no closing sentinel (unlike `wrap_lua_string`): the `\n`
/// it writes before the closer separates `code`'s last byte from the
/// closing `]`, so a trailing `]` in `code` cannot pair with it.
pub(crate) fn lua_chunk_literal(code: &str) -> String {
    let level = pick_long_bracket_level(code);
    let eq = "=".repeat(level);
    format!("[{eq}[\n{code}\n]{eq}]")
}

#[cfg(test)]
#[path = "tests/long_bracket_tests.rs"]
mod tests;
