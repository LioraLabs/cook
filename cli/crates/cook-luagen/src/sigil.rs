//! Strict `$<IDENT>` placeholder scanner per CS-0033 §3.1.
//!
//! A Cook placeholder in shell text matches exactly:
//!   $<IDENT>
//! where IDENT is one of:
//!   bare_ident       := ALPHA (ALPHA | DIGIT | "_" | ".")*
//!   out_indexed      := "out_" DIGIT+
//!   out_indexed_acc  := "out_" DIGIT+ "." accessor
//!   ACC              := "stem" | "name" | "ext" | "dir"
//!   ALPHA            := "a"…"z" | "A"…"Z" | "_"
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

    // Subsequent characters: ALPHA | DIGIT | _ | .
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
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
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

    #[test]
    fn rejects_ident_with_colon() {
        assert!(scan("$<HOME:-default>").is_empty());
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
