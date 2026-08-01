use super::escape_double_quoted;

#[test]
fn passes_ordinary_text_through() {
    assert_eq!(escape_double_quoted("cc -c main.c"), "cc -c main.c");
    assert_eq!(escape_double_quoted(""), "");
}

#[test]
fn escapes_the_two_literal_breakers() {
    assert_eq!(escape_double_quoted(r"a\b"), r"a\\b");
    assert_eq!(escape_double_quoted(r#"say "hi""#), r#"say \"hi\""#);
}

#[test]
fn escapes_the_characters_lua_forbids_raw() {
    assert_eq!(escape_double_quoted("a\nb"), "a\\nb");
    assert_eq!(escape_double_quoted("a\rb"), "a\\rb");
    assert_eq!(escape_double_quoted("a\r\nb"), "a\\r\\nb");
}

/// COOK-398: the drift that motivated this module. cook-luagen escaped `\`,
/// `"` and `\n` only, so a carriage return reached the generated source raw
/// and Lua rejected the chunk.
#[test]
fn carriage_return_never_reaches_output_raw() {
    let out = escape_double_quoted("build\r\nlink");
    assert!(!out.contains('\r'), "raw CR survived: {out:?}");
    assert!(!out.contains('\n'), "raw LF survived: {out:?}");
}

/// Lua's decimal escape consumes up to three digits, so `\0` followed by a
/// digit is a different character. cook-register's hand-rolled escaper emitted
/// `\0`; three digits is the only positionally safe form.
#[test]
fn numeric_escapes_are_always_three_digits() {
    assert_eq!(escape_double_quoted("\u{0}"), "\\000");
    assert_eq!(escape_double_quoted("\u{0}5"), "\\0005");
    assert_eq!(escape_double_quoted("\u{1}"), "\\001");
    assert_eq!(escape_double_quoted("\u{7f}"), "\\127");
}

#[test]
fn escapes_remaining_control_characters() {
    assert_eq!(escape_double_quoted("a\tb"), "a\\tb");
    assert_eq!(escape_double_quoted("\u{1b}[0m"), "\\027[0m");
}

#[test]
fn output_carries_no_raw_control_bytes_or_bare_quotes() {
    let nasty: String = (0u32..0x20).filter_map(char::from_u32).collect();
    let out = escape_double_quoted(&format!("{nasty}\"\\\u{7f}"));
    assert!(
        !out.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7f),
        "control byte survived: {out:?}"
    );
    // Every `"` is preceded by a backslash.
    for (i, c) in out.char_indices() {
        if c == '"' {
            assert_eq!(&out[i - 1..i], "\\", "bare quote at {i} in {out:?}");
        }
    }
}

#[test]
fn multibyte_text_is_untouched() {
    assert_eq!(escape_double_quoted("caf\u{e9} \u{1f600}"), "caf\u{e9} \u{1f600}");
}
