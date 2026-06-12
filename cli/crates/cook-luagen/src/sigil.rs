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
mod tests {
    use super::*;

    fn idents(text: &str) -> Vec<String> {
        scan(text).into_iter().map(|s| s.ident).collect()
    }

    #[test]
    fn matches_simple_ident() {
        assert_eq!(idents("echo $<HOME>"), vec!["HOME"]);
    }

    #[test]
    fn matches_dotted_ident() {
        assert_eq!(idents("$<in.stem>.o"), vec!["in.stem"]);
    }

    #[test]
    fn matches_out_indexed() {
        assert_eq!(idents("cp src $<out_1> $<out_2>"), vec!["out_1", "out_2"]);
    }

    #[test]
    fn matches_out_indexed_accessor() {
        assert_eq!(idents("$<out_1.stem>"), vec!["out_1.stem"]);
    }

    #[test]
    fn matches_multiple_in_one_string() {
        assert_eq!(
            idents("gcc -c $<in> -o $<out>"),
            vec!["in", "out"]
        );
    }

    #[test]
    fn rejects_empty_ident() {
        assert!(scan("$<>").is_empty());
    }

    #[test]
    fn rejects_ident_starting_with_digit() {
        assert!(scan("$<1foo>").is_empty());
    }

    #[test]
    fn rejects_ident_with_space() {
        assert!(scan("$<foo bar>").is_empty());
    }

    #[test]
    fn rejects_ident_with_comma() {
        assert!(scan("$<a,b,c>").is_empty());
    }

    // CS-0074: both `:` and `-` are now valid IDENT-continue characters.
    // `$<HOME:-default>` is now tokenised as a single sigil with
    // ident=`HOME:-default`. In practice, Cook authors do not use shell
    // parameter-expansion syntax inside `$<...>` — probe keys may legitimately
    // contain hyphens (e.g. `$<demo:cc-version.ver>`), so `-` must be admitted.
    #[test]
    fn ident_with_colon_and_dash_is_accepted() {
        let spans = scan("$<HOME:-default>");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].ident, "HOME:-default");
    }

    #[test]
    fn probe_ref_with_hyphen_in_key_is_accepted() {
        assert_eq!(idents("$<demo:cc-version.version>"), vec!["demo:cc-version.version"]);
    }

    // CS-0074: probe-ref tokenization tests
    #[test]
    fn probe_ref_bare_key() {
        assert_eq!(idents("$<cc:zlib>"), vec!["cc:zlib"]);
    }

    #[test]
    fn probe_ref_key_dot_field() {
        assert_eq!(idents("$<cc:zlib.cflags>"), vec!["cc:zlib.cflags"]);
    }

    #[test]
    fn probe_ref_key_field_index() {
        assert_eq!(idents("$<cc:zlib.libs[2]>"), vec!["cc:zlib.libs[2]"]);
    }

    #[test]
    fn probe_ref_does_not_break_bare_ident() {
        assert_eq!(idents("$<in>"), vec!["in"]);
    }

    #[test]
    fn probe_ref_does_not_break_recipe_ident() {
        assert_eq!(idents("$<my_recipe>"), vec!["my_recipe"]);
    }

    #[test]
    fn probe_ref_mixed_with_other_sigils() {
        let result = idents("$<cc:compiler.path> -c $<in> -o $<out>");
        assert_eq!(result, vec!["cc:compiler.path", "in", "out"]);
    }

    #[test]
    fn rejects_unclosed_placeholder() {
        assert!(scan("$<foo").is_empty());
        assert!(scan("$<foo bar baz").is_empty());
    }

    #[test]
    fn does_not_search_forward_for_close() {
        // A `>` appearing later in the string MUST NOT be treated as the close
        // of a malformed `$<...`. Verifies the strict-bail behavior.
        assert!(scan("$<foo bar> baz").is_empty());
    }

    #[test]
    fn literal_dollar_alone_is_not_placeholder() {
        assert!(scan("echo $HOME").is_empty());
        assert!(scan("echo $1").is_empty());
        assert!(scan("price: $5").is_empty());
    }

    #[test]
    fn literal_braces_are_not_placeholders() {
        // The strict rule: only $< triggers the scanner.
        assert!(scan("{a,b,c}").is_empty());
        assert!(scan("${HOME}").is_empty()); // `${` is not `$<`
        assert!(scan("awk '{print $1}'").is_empty());
    }

    #[test]
    fn span_includes_dollar_and_close() {
        let spans = scan("hi $<foo> there");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].range, 3..9);
        assert_eq!(spans[0].ident, "foo");
    }

    // CS-0101: `file:` prefix admits path characters `/` and `*`.
    #[test]
    fn file_ref_literal_path() {
        assert_eq!(idents("--tokens $<file:tokens.css>"), vec!["file:tokens.css"]);
    }

    #[test]
    fn file_ref_with_slash_and_star() {
        assert_eq!(idents("$<file:templates/*.html>"), vec!["file:templates/*.html"]);
        assert_eq!(idents("$<file:voice/narrator.wav>"), vec!["file:voice/narrator.wav"]);
    }

    #[test]
    fn file_ref_empty_path_is_literal() {
        assert!(scan("$<file:>").is_empty());
    }

    #[test]
    fn file_ref_with_space_is_literal() {
        assert!(scan("$<file:a b.css>").is_empty());
    }

    #[test]
    fn file_ref_strict_bail_no_forward_search() {
        // out-of-charset byte (`[`) bails; no forward search for `>`
        assert!(scan("$<file:a[0].css> x").is_empty());
    }

    #[test]
    fn file_ref_multibyte_path_char_strict_bails() {
        // The path charset is ASCII-only; a multibyte char (`ü`) is an
        // out-of-charset byte and MUST strict-bail (sequence stays literal),
        // never split a UTF-8 code point or forward-search for `>`.
        assert!(scan("$<file:fü.css>").is_empty());
    }

    #[test]
    fn file_prefix_only_as_whole_token() {
        // `myfile:x` is NOT a file ref — generic charset still applies
        assert_eq!(idents("$<myfile:x.css>"), vec!["myfile:x.css"]);
    }

    #[test]
    fn dollar_lt_followed_by_dollar_lt() {
        // $<$<x>> — outer $< followed by literal $, then identifier-shaped
        // content fails (first char of IDENT is `$`, which is not ALPHA), so
        // outer is literal. The inner $<x> is a valid placeholder at offset 2.
        let spans = scan("$<$<x>>");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].ident, "x");
        assert_eq!(spans[0].range, 2..6);
    }
}
