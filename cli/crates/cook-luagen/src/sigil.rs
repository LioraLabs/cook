//! Strict `$<IDENT>` placeholder scanner per CS-0033 §3.1, CS-0074, and CS-0101.
//!
//! A Cook placeholder in shell text matches exactly:
//!   $<IDENT>
//! where IDENT is one of:
//!   bare_ident       := ALPHA (ALPHA | DIGIT | "_" | "." | ":" | "[" | "]")*
//!   out_indexed      := "out_" DIGIT+
//!   out_indexed_acc  := "out_" DIGIT+ "." accessor
//!   probe_ref        := ALPHA (ALPHA | DIGIT | "_" | ".")* ":" ...
//!   file_ref         := "file:" PATH_CHAR+
//!   ACC              := "stem" | "name" | "ext" | "dir"
//!   ALPHA            := "a"…"z" | "A"…"Z" | "_"
//!   PATH_CHAR        := ALPHA | DIGIT | "_" | "." | "-" | "/" | "*"
//!
//! CS-0074: IDENTs containing a colon (`:`) are probe-value references.
//! The scanner admits `:`, `.`, `[`, `]`, and `-` as IDENT-continue characters
//! so that `$<cc:zlib.cflags[2]>` and `$<demo:cc-version.ver>` tokenise as
//! single spans. The resolver dispatches on the presence of `:` to select
//! between existing register-time semantics and the new probe-cache-read path.
//!
//! CS-0101: The `file:` namespace uses an extended path charset that admits
//! `/` and `*` (for literal paths and glob patterns). At least one path
//! character after the prefix is required; strict-bail applies (no forward
//! search past an out-of-charset byte).  The prefix dispatch occurs before
//! the generic IDENT-continue loop so that `$<file:dir/*.css>` is a single
//! well-formed span. Other `xxx:` namespaces continue to use the generic
//! charset — `$<myfile:x.css>` tokenises via the generic loop.
//!
//! Anything not matching the strict shape is literal shell text. The scanner
//! does not search forward for a `>` past a malformed inner — a `$<foo bar>`
//! is literal, not an unclosed-placeholder error.

use std::ops::Range;

/// One placeholder occurrence in a shell text string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderSpan {
    /// Byte range of the entire placeholder, including `$<` and `>`.
    pub range: Range<usize>,
    /// The IDENT content between `$<` and `>`.
    pub ident: String,
}

/// Scan `text` for all well-formed `$<IDENT>` placeholders.
/// Returns spans in source order. Malformed `$<...` sequences are skipped
/// (treated as literal shell text).
pub fn scan(text: &str) -> Vec<PlaceholderSpan> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'$' && bytes[i + 1] == b'<' {
            if let Some(span) = try_match_placeholder(text, i) {
                let end = span.range.end;
                out.push(span);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// If `text[start..]` begins with a well-formed `$<IDENT>`, return the span.
/// Otherwise None.
fn try_match_placeholder(text: &str, start: usize) -> Option<PlaceholderSpan> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes[start], b'$');
    debug_assert_eq!(bytes[start + 1], b'<');
    let ident_start = start + 2;
    let mut i = ident_start;

    // First IDENT character must be ALPHA (a-z, A-Z, _).
    if i >= bytes.len() || !is_alpha(bytes[i]) {
        return None;
    }
    i += 1;

    // CS-0101: the `file:` namespace admits a path charset (`/`, `*`)
    // that the generic IDENT charset does not. At least one path char
    // is required; strict-bail otherwise (the sequence stays literal).
    const FILE_PREFIX: &str = "file:";
    if text[ident_start..].starts_with(FILE_PREFIX) {
        let path_start = ident_start + FILE_PREFIX.len();
        let mut j = path_start;
        while j < bytes.len() && is_file_path_continue(bytes[j]) {
            j += 1;
        }
        if j == path_start || j >= bytes.len() || bytes[j] != b'>' {
            return None;
        }
        return Some(PlaceholderSpan {
            range: start..j + 1,
            ident: text[ident_start..j].to_string(),
        });
    }

    // Subsequent characters: ALPHA | DIGIT | _ | . | : | [ | ]
    while i < bytes.len() && is_ident_continue(bytes[i]) {
        i += 1;
    }

    // Must be followed immediately by `>`.
    if i >= bytes.len() || bytes[i] != b'>' {
        return None;
    }

    let ident = text[ident_start..i].to_string();
    Some(PlaceholderSpan {
        range: start..i + 1,
        ident,
    })
}

#[inline]
fn is_alpha(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

#[inline]
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':' || b == b'[' || b == b']' || b == b'-'
}

#[inline]
fn is_file_path_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'/' | b'*')
}

#[cfg(test)]
#[path = "tests/sigil_tests.rs"]
mod tests;
