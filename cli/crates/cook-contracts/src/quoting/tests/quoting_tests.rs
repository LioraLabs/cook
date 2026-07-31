use super::*;

#[test]
fn tags_round_trip() {
    for ctx in [QCtx::Bare, QCtx::Double, QCtx::Single] {
        assert_eq!(QCtx::from_tag(ctx.tag()), Some(ctx));
    }
}

#[test]
fn unknown_tag_is_none_not_bare() {
    assert_eq!(QCtx::from_tag("triple"), None);
    assert_eq!(QCtx::from_tag(""), None);
    assert_eq!(QCtx::from_tag("BARE"), None);
}

#[test]
fn classifies_bare_double_single() {
    assert_eq!(quote_context("echo "), QCtx::Bare);
    assert_eq!(quote_context(""), QCtx::Bare);
    assert_eq!(quote_context("echo \"prefix "), QCtx::Double);
    assert_eq!(quote_context("echo 'prefix "), QCtx::Single);
    // closed regions return to bare
    assert_eq!(quote_context("echo \"a\" "), QCtx::Bare);
    assert_eq!(quote_context("echo 'a' "), QCtx::Bare);
    // A single quote inside a double-quoted region is literal, not an open.
    assert_eq!(quote_context("echo \"it's "), QCtx::Double);
    // A double quote inside a single-quoted region is literal.
    assert_eq!(quote_context("echo 'say \"hi "), QCtx::Single);
}

#[test]
fn backslash_is_inert_inside_single_quotes() {
    // POSIX: '\' inside single quotes is a literal backslash; the next
    // quote still closes the region.
    assert_eq!(quote_context(r#"echo '\'"#), QCtx::Bare);
    assert_eq!(quote_context("echo 'a\\"), QCtx::Single);
    // ...but escapes a double quote outside single quotes, so an escaped
    // double-quote does not open a double-quoted region.
    assert_eq!(quote_context("echo \\\" "), QCtx::Bare);
}

#[test]
fn quotes_per_context() {
    assert_eq!(quote_for_ctx("a b", QCtx::Bare), "'a b'");
    assert_eq!(quote_for_ctx("it's", QCtx::Bare), r#"'it'\''s'"#);
    assert_eq!(quote_for_ctx(r#"a"b$c"#, QCtx::Double), r#"a\"b\$c"#);
    assert_eq!(quote_for_ctx("raw $HOME", QCtx::Single), "raw $HOME");
}
