//! Strict `$<IDENT>` placeholder scanner per CS-0033 §3.1, CS-0074, and CS-0101.
//!
//! A Cook placeholder in shell text matches exactly:
//!   $<IDENT>
//! where IDENT is one of:
//!   bare_ident       := ALPHA (ALPHA | DIGIT | "_" | "." | ":" | "[" | "]")*
//!   out_indexed      := "out_" DIGIT+
//!   out_indexed_acc  := "out_" DIGIT+ "." accessor
//!   probe_ref        := ALPHA (ALPHA | DIGIT | "_" | ".")* ":" ...
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
//!
//! Anything not matching the strict shape is literal shell text. The scanner
//! does not search forward for a `>` past a malformed inner — a `$<foo bar>`
//! is literal, not an unclosed-placeholder error.

use std::ops::Range;

use crate::lua_string::escape_lua_string;

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

// ─── CS-0074: the probe-value reference grammar ─────────────────────────────
//
// Lives beside the scanner rather than in `resolver`, and not in
// cook-contracts, because the ident->`cook.probes.get(...)` mapping is the
// placeholder's own desugaring: CS-0074 and §22.5.7 define the two together,
// and both consumers (this crate's resolver, cook-register's `cook.add_unit`
// capture) emit Lua. Splitting the parse from the render would cost an extra
// module hop for one 40-line concept without buying a boundary anyone needs.

/// One path segment of a probe-value reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    /// `.field`
    Field(String),
    /// `[i]` — the index text verbatim. §22.5.7 defines `[i]` as a one-based
    /// array element; anything else lowers as written and fails at execute
    /// time against the real value, which is where the type is known.
    Index(String),
}

/// A parsed probe-value reference: the `key` a `$<key:field[i]>` sigil names,
/// plus the access path applied to that key's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRef {
    key: String,
    path: Vec<Seg>,
}

impl ProbeRef {
    /// The probe key: everything up to the first `.` or `[` that follows the
    /// `:` discriminator. A dot BEFORE the colon belongs to the key
    /// (`demo:cc-version.ver` keys on `demo:cc-version`).
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The access path applied to the key's value, in source order.
    pub fn path(&self) -> &[Seg] {
        &self.path
    }

    /// The ready-to-emit Lua read: `cook.probes.get("key").field[1]`.
    pub fn lua_access(&self) -> String {
        let mut access = format!("cook.probes.get(\"{}\")", escape_lua_string(&self.key));
        for seg in &self.path {
            match seg {
                Seg::Field(name) => {
                    access.push('.');
                    access.push_str(name);
                }
                Seg::Index(idx) => {
                    access.push('[');
                    access.push_str(idx);
                    access.push(']');
                }
            }
        }
        access
    }
}

/// Parse a probe-shaped IDENT, or `None` when `ident` is not one.
///
/// Probe-shaped means: contains a `:`. CS-0187 removed the one namespace that
/// was dispatched ahead of the colon discriminator, so a colon now means a
/// probe reference and nothing else.
pub fn probe_ref(ident: &str) -> Option<ProbeRef> {
    let colon = ident.find(':')?;

    // The key ends at the first `.` or `[` that appears AFTER the colon.
    let after_colon = &ident[colon + 1..];
    let path_start = after_colon
        .find(|c: char| c == '.' || c == '[')
        .map(|p| colon + 1 + p)
        .unwrap_or(ident.len());

    let mut path = Vec::new();
    let mut chars = ident[path_start..].chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '.' => {
                chars.next();
                let mut name = String::new();
                while let Some(&nc) = chars.peek() {
                    if nc.is_alphanumeric() || nc == '_' {
                        name.push(nc);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if !name.is_empty() {
                    path.push(Seg::Field(name));
                }
            }
            '[' => {
                chars.next();
                let mut idx = String::new();
                while let Some(&nc) = chars.peek() {
                    if nc == ']' {
                        chars.next();
                        break;
                    }
                    idx.push(nc);
                    chars.next();
                }
                path.push(Seg::Index(idx));
            }
            _ => {
                chars.next();
            }
        }
    }

    Some(ProbeRef {
        key: ident[..path_start].to_string(),
        path,
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

#[cfg(test)]
#[path = "tests/sigil_tests.rs"]
mod tests;
