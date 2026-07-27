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
