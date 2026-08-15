//! Strict `$<IDENT>` placeholder scanner per CS-0033 §3.1, CS-0074, and CS-0101.
//!
//! A Cook placeholder in shell text matches exactly:
//!   $<IDENT>
//! where IDENT is one of:
//!   bare_ident       := ALPHA (ALPHA | DIGIT | "_" | "." | ":" | "[" | "]")*
//!   out_indexed      := "out_" DIGIT+
//!   out_indexed_acc  := "out_" DIGIT+ "." accessor
//!   probe_ref        := PROBE_KEY ( "." field | "[" index "]" )*
//!   ACC              := "stem" | "name" | "ext" | "dir"
//!   ALPHA            := "a"…"z" | "A"…"Z" | "_"
//!   PATH_CHAR        := ALPHA | DIGIT | "_" | "." | "-" | "/" | "*"
//!
//! CS-0074 admitted `:`, `.`, `[`, `]`, and `-` as IDENT-continue characters so
//! that `$<cc:zlib.cflags[2]>` and `$<demo:cc-version.ver>` tokenise as single
//! spans. That much is unchanged.
//!
//! **CS-0240: a probe reference is recognised by NAME, not by punctuation.**
//! CS-0074 dispatched a sigil to the probe path when — and only when — its
//! IDENT contained a colon, which left the sigil as the one bare-name position
//! in the language resolving lexically rather than by lookup. Every other one
//! (`seal` operands, `gather` sources, `probes = {…}`, `cook.probes.get`)
//! already resolved a colon-free key by name, and the native `probe` DSL mints
//! colon-free keys, so `probe keyed_obs` declared a probe that `$<keyed_obs>`
//! could not reach. [`probe_ref`] now asks its caller whether the base name is
//! a declared probe key. A colon-carrying base still answers yes with no
//! lookup: no builtin, recipe or variable name can contain one, so it stays
//! self-identifying, which is what keeps module keys working through callers
//! that hold no keyset of their own.
//!
//!
//! Anything not matching the strict shape is literal shell text. The scanner
//! does not search forward for a `>` past a malformed inner — a `$<foo bar>`
//! is literal, not an unclosed-placeholder error.
//!
//! **Why this lives in `cook-contracts` (CS-0188).** It was deliberately kept
//! in `cook-luagen` while "both consumers emit Lua" was true: the ident to
//! `cook.probes.get(...)` mapping is the placeholder's own desugaring, and
//! splitting the parse from the render bought no boundary. CS-0188 removed the
//! premise. `cook-register` no longer rewrites a probe-referencing command into
//! Lua, and the execute-phase worker now needs the PARSE with no render at all,
//! so the two are no longer one concept. The render stays in `cook-luagen`,
//! which is the crate that emits Lua; what moved here is what both phases must
//! agree on, which is what this crate is for.

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
    /// The probe key: everything up to the first `.` or `[` at or after any
    /// `:`. A dot BEFORE the colon belongs to the key
    /// (`demo:cc-version.ver` keys on `demo:cc-version`); with no colon the
    /// whole leading run is the key (`keyed_obs.field` keys on `keyed_obs`).
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The access path applied to the key's value, in source order.
    pub fn path(&self) -> &[Seg] {
        &self.path
    }

}

/// The base name an IDENT keys on, and the byte offset where its access path
/// begins.
///
/// The base ends at the first `.` or `[` at or after any `:`. A dot BEFORE the
/// colon belongs to the base (`demo:cc-version.ver` keys on
/// `demo:cc-version`), which is the CS-0074 rule generalised: with no colon
/// present the scan simply starts at offset 0, so `keyed_obs.field` keys on
/// `keyed_obs`. `PROBE_SEG` admits `-` but not `.` (see `probe_key`), so the
/// first dot is always the start of member access and never part of the key.
fn split_base(ident: &str) -> (&str, usize) {
    let from = ident.find(':').map(|c| c + 1).unwrap_or(0);
    let path_start = ident[from..]
        .find(|c: char| c == '.' || c == '[')
        .map(|p| from + p)
        .unwrap_or(ident.len());
    (&ident[..path_start], path_start)
}

/// The membership predicate for [`probe_ref`] at a call site that holds no
/// probe keyset — colon-carrying bases only, which is the CS-0074 behaviour.
///
/// Named rather than spelled `|_| false` at each site so the two callers that
/// deliberately stay lexical say so, and a reader can find them.
pub fn colon_keys_only(_name: &str) -> bool {
    false
}

/// Parse a probe-value reference out of an IDENT, or `None` when `ident` is
/// not one.
///
/// `declares` answers "is this a declared probe key" for the caller's scope.
/// A base containing `:` is a probe reference regardless of what `declares`
/// says: CS-0187 removed the one namespace dispatched ahead of the colon, so a
/// colon means a probe key and nothing else, and a caller that cannot see a
/// keyset (the raw `cook.add_unit` capture, whose pass has not finished
/// registering probes) keeps working by passing [`colon_keys_only`].
///
/// A colon-free base is a probe reference exactly when `declares` says so
/// (CS-0240). Ordering against the rest of §10.2's cascade is the caller's;
/// this function only answers what the token names in the probe namespace.
pub fn probe_ref(ident: &str, declares: impl Fn(&str) -> bool) -> Option<ProbeRef> {
    let (base, path_start) = split_base(ident);
    if !base.contains(':') && !declares(base) {
        return None;
    }

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


pub mod subst;

#[cfg(test)]
#[path = "tests/sigil_tests.rs"]
mod tests;
