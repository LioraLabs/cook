//! Where a Lua string or comment begins and ends (COOK-403).
//!
//! Lua spells a string four ways — `"…"`, `'…'`, `[[…]]`, `[==[…]==]` — and a
//! comment two — `-- …` to end of line, `--[==[ … ]==]` across them. Anything
//! that walks Lua source looking for a shape must ignore all six, because a
//! brace, a field name or an identifier written inside one is text, not code.
//!
//! That is one lexical fact about one language, and it had two implementations
//! in two crates at different fidelities. `cook-luagen` scans generated and
//! authored Lua for `var.X` reads, `cook.probes.get` keys and free identifiers
//! (COOK-357), and understood all six. `cook-cookfile` scans a module call for
//! the field it is about to splice into, and understood two: it read `[[a}b]]`
//! as a list that closed at the `}`, and spliced the author's new entry into
//! the middle of their string literal (CS-0208).
//!
//! Both answers decide where a byte range ends, both are consumed by code that
//! then edits or lowers those bytes, and disagreement between them is a defect
//! rather than a preference — so the answer lives here, once, and both crates
//! ask it. It is pure and needs no dependency at all.
//!
//! # Scope
//!
//! This says what is *not* code. It does not tokenise Lua, does not know what
//! any construct means, and does not decide anything about Cook's own grammar.
//! Its inverse — choosing a long-bracket level no inner close can match, when
//! *writing* a literal — is a lowering choice and stays in `cook-luagen`'s
//! `wrap_lua_string`, next to [`crate::lua_string`]'s short-literal escaping.

/// What [`skip_non_code`] found at a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// A comment or string ended at this index; resume scanning there.
    Ended(usize),
    /// A comment or string opened and never closed. The source cannot be
    /// scanned past this point; every caller stops with what it has, because
    /// guessing where an unterminated literal ends invents matches.
    Unterminated,
    /// Not the start of a comment or string; the caller's own matching applies.
    Code,
}

/// Classify position `i` in `src`.
///
/// Recognises `--` line comments, `--[[ … ]]` / `--[==[ … ]==]` long comments,
/// `"…"` and `'…'` short strings (with backslash escapes), and `[[ … ]]` /
/// `[==[ … ]==]` long strings.
///
/// `i` is a byte index. Every construct this recognises opens with an ASCII
/// byte, so a caller walking bytes rather than chars cannot land inside a
/// multi-byte character and mistake one for an opener.
pub fn skip_non_code(src: &str, i: usize) -> Skip {
    let bytes = src.as_bytes();
    let b = bytes[i];

    // Line and long comments both open with `--`.
    if opens_comment(bytes, i) {
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
        // A short string that runs to end-of-input consumes the rest, rather
        // than reporting `Unterminated` as the long forms do. That asymmetry
        // is deliberate and it is law, not one caller's legacy: an unclosed
        // long bracket is a span whose end is genuinely unknown, while an
        // unclosed short string is a span Lua itself would end at the next
        // newline, so the only question is how much of the tail to distrust.
        // Both callers want the same answer to that — everything after an open
        // quote is not code — and both then fail to find what they were
        // looking for, which is the honest outcome for a fragment the author
        // has already broken. `min` keeps the index in bounds.
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

/// Does a comment open at `i`?
///
/// Both spellings open with the same two bytes, which is what makes this one
/// fact rather than two. A caller that has to tell a comment from a string —
/// because a comment may be written over but a string may not — asks here
/// rather than re-deriving it beside [`skip_non_code`]'s own answer.
pub fn opens_comment(bytes: &[u8], i: usize) -> bool {
    bytes[i] == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-'
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

pub fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

pub fn is_ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Advance from `start` while identifier-continuation bytes match. Returns
/// `start` unchanged when `start` is not an identifier-start byte.
pub fn ident_end(bytes: &[u8], start: usize) -> usize {
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
