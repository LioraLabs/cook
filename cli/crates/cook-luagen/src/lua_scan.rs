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

/// Does `ident` occur as a FREE identifier — a read of a name in scope, rather
/// than a field or method of something else — in the code regions of `src`?
///
/// This is the crate's one free-identifier walk. Two callers ask the same
/// question of a Lua body and must get the same answer:
///
/// - [`crate::template`] asks whether a plate/test body reads `input` or
///   `inputs`, which picks the body's iteration mode;
/// - [`crate::use_prelude`] asks whether a body names a `use` alias, which
///   decides whether the CS-0205 binding is prepended.
///
/// Both directions of a wrong answer are defects, and they are asymmetric.
/// A missed reference (false negative) is the defect CS-0205 fixes: the alias
/// is unbound and the body dies on a nil global. A spurious one (false
/// positive) is NOT the harmless extra module load it looks like — the binding
/// evaluates the module's top level and `init()` on a worker VM for a body that
/// never asked, which raises for a module written for register phase, and under
/// CS-0204 it puts that module in the unit's cache key.
///
/// So both are ruled out where they actually arise:
///
/// - a whole token is consumed at once, so `mygreet` and `greet2` cannot
///   partial-match `greet`;
/// - a name immediately behind `.` or `:` is a field or method (`t.greet`),
///   not this scope's `greet` — unless that dot is the second of a `..`
///   concatenation (`"pre"..greet.f()`), where the name IS free. Checking only
///   the single preceding byte gets that case wrong in the false-negative
///   direction.
///
/// `load`ed chunks and `_ENV` lookups are deliberately not considered: neither
/// can see a `local`, so neither can observe the binding.
pub(crate) fn free_identifier_occurs(src: &str, ident: &str) -> bool {
    if ident.is_empty() {
        return false;
    }
    let bytes = src.as_bytes();
    let needle = ident.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match skip_non_code(src, bytes, i) {
            Skip::Ended(next) => i = next.max(i + 1),
            // Past an unterminated literal there is no honest answer; guessing
            // where it ends invents matches.
            Skip::Unterminated => return false,
            Skip::Code => {
                if is_ident_start(bytes[i]) {
                    let start = i;
                    let end = ident_end(bytes, i);
                    if &bytes[start..end] == needle && !is_field_access_at(bytes, start) {
                        return true;
                    }
                    i = end;
                } else {
                    i += 1;
                }
            }
        }
    }
    false
}

/// Is the identifier starting at `start` the field or method half of an access?
///
/// True for `t.name` and `t:name`. False for `x..name`, where the byte before
/// is a dot but is the second half of the concatenation operator.
fn is_field_access_at(bytes: &[u8], start: usize) -> bool {
    if start == 0 {
        return false;
    }
    match bytes[start - 1] {
        b':' => true,
        b'.' => start < 2 || bytes[start - 2] != b'.',
        _ => false,
    }
}

#[cfg(test)]
#[path = "tests/lua_scan_tests.rs"]
mod tests;
