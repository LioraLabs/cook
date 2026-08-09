use super::*;

/// Walk `src` with `skip_non_code`, collecting the byte offsets the scanner
/// considers code. This is the shared contract all three callers rely on.
fn code_offsets(src: &str) -> Vec<usize> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match skip_non_code(src, bytes, i) {
            Skip::Ended(next) => {
                assert!(next > i, "skip must make progress at {i} in {src:?}");
                i = next;
            }
            Skip::Unterminated => break,
            Skip::Code => {
                out.push(i);
                i += 1;
            }
        }
    }
    out
}

fn code_text(src: &str) -> String {
    code_offsets(src)
        .into_iter()
        .map(|i| src.as_bytes()[i] as char)
        .collect()
}

#[test]
fn line_comment_is_not_code() {
    assert_eq!(code_text("a -- x\nb"), "a \nb");
}

#[test]
fn long_comment_is_not_code() {
    assert_eq!(code_text("a--[[ x ]]b"), "ab");
}

#[test]
fn long_comment_with_equals_level_is_not_code() {
    assert_eq!(code_text("a--[==[ ]] x ]==]b"), "ab");
}

#[test]
fn short_strings_are_not_code() {
    assert_eq!(code_text(r#"a"x"b'y'c"#), "abc");
}

#[test]
fn escaped_quote_does_not_end_a_short_string() {
    assert_eq!(code_text(r#"a"x\"y"b"#), "ab");
}

#[test]
fn long_strings_are_not_code() {
    assert_eq!(code_text("a[[ x ]]b"), "ab");
}

#[test]
fn long_string_with_equals_level_is_not_code() {
    assert_eq!(code_text("a[==[ ]] x ]==]b"), "ab");
}

#[test]
fn a_lone_bracket_is_code() {
    // `[` only opens a long string when followed by `=*[`; an ordinary index
    // must stay visible to the callers' matching.
    assert_eq!(code_text("t[k]"), "t[k]");
}

#[test]
fn unterminated_long_comment_stops_the_scan() {
    let src = "a--[[ never closed";
    assert!(matches!(
        skip_non_code(src, src.as_bytes(), 1),
        Skip::Unterminated
    ));
}

#[test]
fn unterminated_long_string_stops_the_scan() {
    let src = "a[[ never closed";
    assert!(matches!(
        skip_non_code(src, src.as_bytes(), 1),
        Skip::Unterminated
    ));
}

#[test]
fn unterminated_short_string_consumes_the_rest() {
    // Historical shape: callers treated a runaway short string as consuming
    // the remainder rather than bailing. Pinned so the shared helper cannot
    // change one caller's behaviour while preserving another's.
    let src = "a\"never closed";
    assert!(matches!(
        skip_non_code(src, src.as_bytes(), 1),
        Skip::Ended(n) if n == src.len()
    ));
}

#[test]
fn ident_end_walks_a_whole_identifier() {
    let src = "foo_bar1 baz";
    assert_eq!(ident_end(src.as_bytes(), 0), 8);
}

#[test]
fn ident_end_returns_start_on_a_non_identifier() {
    let src = "1abc";
    assert_eq!(ident_end(src.as_bytes(), 0), 0);
}

// ---------------------------------------------------------------------------
// identifier_occurs (CS-0205)
// ---------------------------------------------------------------------------

#[test]
fn identifier_occurs_finds_a_bare_reference() {
    assert!(free_identifier_occurs("print(greet.say(\"x\"))", "greet"));
}

#[test]
fn identifier_occurs_is_word_bounded() {
    assert!(!free_identifier_occurs("greeting.say()", "greet"));
    assert!(!free_identifier_occurs("mygreet.say()", "greet"));
    assert!(!free_identifier_occurs("_greet.say()", "greet"));
    assert!(!free_identifier_occurs("greet2.say()", "greet"));
}

#[test]
fn identifier_occurs_ignores_strings_and_comments() {
    assert!(!free_identifier_occurs("print(\"greet\")", "greet"));
    assert!(!free_identifier_occurs("print('greet')", "greet"));
    assert!(!free_identifier_occurs("print([[greet]])", "greet"));
    assert!(!free_identifier_occurs("-- greet\nprint(1)", "greet"));
    assert!(!free_identifier_occurs("--[[ greet ]] print(1)", "greet"));
}

#[test]
fn identifier_occurs_after_a_string_still_matches() {
    // The scanner must resume scanning code AFTER a literal, not stop at it.
    assert!(free_identifier_occurs("print(\"hello\") greet.say()", "greet"));
}

#[test]
fn identifier_occurs_skips_a_field_or_method_access() {
    // `t.greet` and `t:greet()` name a FIELD, never the local. Binding the
    // module for them is not a harmless extra load: it evaluates the module on
    // a worker VM for a body that never asked, which hard-fails for a
    // register-oriented module — the exact scenario CS-0205 gates to avoid.
    assert!(!free_identifier_occurs("t.greet = 1", "greet"));
    assert!(!free_identifier_occurs("t:greet()", "greet"));
    assert!(!free_identifier_occurs("local o = {} o.greet = 1 print(o.greet)", "greet"));
}

#[test]
fn identifier_occurs_is_not_fooled_by_concatenation() {
    // `..` is two dots, and the second one is NOT a field-access dot. Missing
    // this is a false NEGATIVE — the direction that reinstates the defect —
    // so it is pinned separately from the field-access case above.
    assert!(free_identifier_occurs("x .. greet.f()", "greet"));
    assert!(free_identifier_occurs("\"pre\"..greet.f()", "greet"));
    assert!(free_identifier_occurs("a.b..greet.f()", "greet"));
}

#[test]
fn identifier_occurs_stops_at_an_unterminated_literal() {
    // Same halt the other scanners take: past an unterminated literal there is
    // no honest answer, and guessing invents matches.
    assert!(!free_identifier_occurs("print(\"open [[ greet", "greet"));
}

#[test]
fn identifier_occurs_is_false_for_an_empty_body() {
    assert!(!free_identifier_occurs("", "greet"));
}
