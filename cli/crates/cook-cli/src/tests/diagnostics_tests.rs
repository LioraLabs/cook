use super::*;

#[test]
fn strips_traceback_and_wrappers() {
    let raw = "lua error: runtime error: Cookfile:3: attempt to call a nil value (global 'OUTDIR')\nstack traceback:\n\t[C]: in global 'OUTDIR'\n\tCookfile:3: in function '__cook_run_config_blocks'";
    assert_eq!(
        sanitize_error(raw, false),
        "Cookfile:3: attempt to call a nil value (global 'OUTDIR')"
    );
}

#[test]
fn preserves_recipe_tag() {
    let raw = "[boom] lua error: runtime error: Cookfile:2: kaboom\nstack traceback:\n\t[C]: in ?";
    assert_eq!(sanitize_error(raw, false), "[boom] Cookfile:2: kaboom");
    assert_eq!(
        sanitize_error(raw, false),
        cook_contracts::lua_error::sanitize(raw, false)
    );
}

#[test]
fn backtrace_optin_keeps_traceback() {
    let raw = "lua error: runtime error: Cookfile:3: boom\nstack traceback:\n\tx";
    let s = sanitize_error(raw, true);
    assert!(s.contains("stack traceback:"));
    assert!(s.starts_with("Cookfile:3: boom"));
}

#[test]
fn plain_messages_pass_through() {
    assert_eq!(sanitize_error("recipe not found: zzz", false), "recipe not found: zzz");
}

#[test]
fn extracts_leading_cookfile_location() {
    assert_eq!(
        extract_location("Cookfile:3: attempt to call a nil value"),
        (Some("Cookfile".to_string()), Some(3))
    );
    assert_eq!(
        extract_location("sub/Cookfile:7: kaboom"),
        (Some("sub/Cookfile".to_string()), Some(7))
    );
}

#[test]
fn extracts_parse_error_line_location() {
    assert_eq!(
        extract_location("parse error: line 2: config values are Lua assignments"),
        (Some("Cookfile".to_string()), Some(2))
    );
}

#[test]
fn locationless_messages_return_none() {
    assert_eq!(extract_location("recipe not found: zzz"), (None, None));
}
