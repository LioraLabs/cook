use super::*;

/// `cook menu` renders a required positional as its bare name.
#[test]
fn required_renders_as_bare_name() {
    let meta = ChoreParamMeta::Required { name: "caller".to_string() };
        assert_eq!(meta.display_token(), "caller");
}

/// A defaulted-string positional renders `name="default"` with the
/// default Debug-quoted (matches shell-quoting expectations closely
/// enough for a display token, without claiming to be re-parseable).
#[test]
fn defaulted_string_renders_name_equals_debug_quoted_default() {
    let meta = ChoreParamMeta::DefaultedString {
        name: "who".to_string(),
        default: "world".to_string(),
    };
    assert_eq!(meta.display_token(), "who=\"world\"");
}

/// A Lua-expression default can't be rendered as a literal (it's a
/// closure evaluated at bind time), so the token is a placeholder.
#[test]
fn defaulted_lua_renders_placeholder() {
    let meta = ChoreParamMeta::DefaultedLua {
        name: "version".to_string(),
        default_key_name: "__cook_chore_default:demo:version:0".to_string(),
    };
    assert_eq!(meta.display_token(), "version=<lua>");
}

/// A one-or-more variadic renders with a bare `...` suffix (no
/// brackets — at least one argv is required).
#[test]
fn variadic_plus_renders_with_trailing_ellipsis() {
    let meta = ChoreParamMeta::VariadicPlus { name: "rest".to_string() };
    assert_eq!(meta.display_token(), "rest...");
}

/// A zero-or-more variadic renders bracketed to signal optionality.
#[test]
fn variadic_star_renders_bracketed() {
    let meta = ChoreParamMeta::VariadicStar { name: "rest".to_string() };
    assert_eq!(meta.display_token(), "[rest...]");
}
