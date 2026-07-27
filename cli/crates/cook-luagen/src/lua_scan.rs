//! Skipping the non-code regions of Lua source.
//!
//! Three scanners in this crate walk a Lua body looking for one shape and must
//! ignore everything written inside strings and comments: `var.X` reads and
//! `cook.probes.get("k")` calls in [`crate::lua_var`], and the free `input` /
//! `inputs` identifiers that pick a plate/test body's iteration mode in
//! [`crate::template`]. Each carried its own copy of the same walk (COOK-357).
//!
//! This is not sigil meaning and does not belong beside the resolver: it is a
//! lexical fact about Lua, owned by the crate that reads Lua source.

/// What [`skip_non_code`] found at a position.
pub(crate) enum Skip {
    /// A comment or string ended at this index; resume scanning there.
    Ended(usize),
    /// A comment or string opened and never closed. The source cannot be
    /// scanned past this point; every caller stops with what it has, because
    /// guessing where an unterminated literal ends invents matches.
    Unterminated,
    /// Not the start of a comment or string; the caller's own matching applies.
    Code,
}

/// Classify position `i` in `src` (whose bytes are `bytes`).
///
/// Recognises `--` line comments, `--[[ … ]]` / `--[==[ … ]==]` long comments,
/// `"…"` and `'…'` short strings (with backslash escapes), and `[[ … ]]` /
/// `[==[ … ]==]` long strings.
pub(crate) fn skip_non_code(src: &str, bytes: &[u8], i: usize) -> Skip {
    let b = bytes[i];

    // Line and long comments both open with `--`.
    if b == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
        if i + 2 < bytes.len() && bytes[i + 2] == b'[' {
            if let (eq_count, Some(after_open)) = count_long_bracket_eqs(&bytes[i + 3..]) {
                let close = format!("]{}]", "=".repeat(eq_count));
                let from = i + 3 + after_open;
                return match src[from..].find(&close) {
                    Some(rel) => Skip::Ended(from + rel + close.len()),
                    None => Skip::Unterminated,
                };
            }
        }
        let mut j = i;
        while j < bytes.len() && bytes[j] != b'\n' {
            j += 1;
        }
        return Skip::Ended(j);
    }

    // Short strings.
    if b == b'"' || b == b'\'' {
        let quote = b;
        let mut j = i + 1;
        while j < bytes.len() && bytes[j] != quote {
            j += if bytes[j] == b'\\' && j + 1 < bytes.len() { 2 } else { 1 };
        }
        // A short string that runs to end-of-input is unterminated, but callers
        // have always treated it as simply consuming the rest, so `Ended` past
        // the end is the historical shape. `min` keeps the index in bounds.
        return Skip::Ended((j + 1).min(bytes.len()));
    }

    // Long strings.
    if b == b'[' {
        if let (eq_count, Some(after_open)) = count_long_bracket_eqs(&bytes[i + 1..]) {
            let close = format!("]{}]", "=".repeat(eq_count));
            let from = i + 1 + after_open;
            return match src[from..].find(&close) {
                Some(rel) => Skip::Ended(from + rel + close.len()),
                None => Skip::Unterminated,
            };
        }
    }

    Skip::Code
}

/// At `bytes[0]` we are past a leading `[`. If the next bytes are `=*[` this is
/// a long-bracket open: returns `(equals count, offset just past the second
/// `[`)`. Otherwise `(0, None)`.
fn count_long_bracket_eqs(bytes: &[u8]) -> (usize, Option<usize>) {
    let mut eq = 0;
    while eq < bytes.len() && bytes[eq] == b'=' {
        eq += 1;
    }
    if eq < bytes.len() && bytes[eq] == b'[' {
        (eq, Some(eq + 1))
    } else {
        (0, None)
    }
}

pub(crate) fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

pub(crate) fn is_ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Advance from `start` while identifier-continuation bytes match. Returns
/// `start` unchanged when `start` is not an identifier-start byte.
pub(crate) fn ident_end(bytes: &[u8], start: usize) -> usize {
    if start >= bytes.len() || !is_ident_start(bytes[start]) {
        return start;
    }
    let mut k = start + 1;
    while k < bytes.len() && is_ident_cont(bytes[k]) {
        k += 1;
    }
    k
}

#[cfg(test)]
#[path = "tests/lua_scan_tests.rs"]
mod tests;
