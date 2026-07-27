use super::sanitize;

#[test]
fn strips_traceback_and_lua_wrappers_in_order() {
    let raw = "lua error: runtime error: Cookfile:3: boom\nstack traceback:\n\tx";
    assert_eq!(sanitize(raw, false), "Cookfile:3: boom");
}

#[test]
fn traceback_can_be_preserved_while_wrappers_are_stripped() {
    let raw = "lua error: runtime error: Cookfile:3: boom\nstack traceback:\n\tx";
    assert_eq!(
        sanitize(raw, true),
        "Cookfile:3: boom\nstack traceback:\n\tx"
    );
}

#[test]
fn recipe_tag_is_preserved_while_wrappers_after_it_are_stripped() {
    let raw = "[recipe] lua error: runtime error: Cookfile:2: kaboom\nstack traceback:\n\t[C]";
    assert_eq!(sanitize(raw, false), "[recipe] Cookfile:2: kaboom");
}

#[test]
fn unrelated_messages_are_unchanged() {
    assert_eq!(
        sanitize("recipe not found: zzz", false),
        "recipe not found: zzz"
    );
}
