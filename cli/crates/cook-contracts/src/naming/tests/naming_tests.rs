use super::{internal_module_tag, is_internal_recipe};

#[test]
fn plain_recipe_names_are_not_internal() {
    assert!(!is_internal_recipe("build"));
    assert!(!is_internal_recipe("apps.web.build"));
    assert!(!is_internal_recipe(""));
    assert_eq!(internal_module_tag("build"), None);
}

#[test]
fn bare_internal_names_are_recognised() {
    assert!(is_internal_recipe("__cc_config_header__build_dhewm3_config_h"));
    assert_eq!(
        internal_module_tag("__cc_config_header__build_dhewm3_config_h"),
        Some("cc")
    );
}

/// COOK-411: the disagreement. cook-cli tested the last segment and hid this
/// from completion; cook-progress tested the whole string and rendered it raw.
#[test]
fn a_qualified_internal_name_is_internal_too() {
    assert!(is_internal_recipe("game.__cc_config_header__x"));
    assert_eq!(internal_module_tag("game.__cc_config_header__x"), Some("cc"));

    assert!(is_internal_recipe("apps.web.__pnpm_install"));
    assert_eq!(internal_module_tag("apps.web.__pnpm_install"), Some("pnpm"));
}

#[test]
fn a_dunder_with_nothing_after_it_has_no_tag() {
    assert!(is_internal_recipe("__"));
    assert_eq!(internal_module_tag("__"), None);
    assert!(is_internal_recipe("ns.__"));
    assert_eq!(internal_module_tag("ns.__"), None);
}

/// A single leading underscore is a user's choice of name, not the tooling
/// convention.
#[test]
fn one_underscore_is_not_the_convention() {
    assert!(!is_internal_recipe("_private"));
    assert_eq!(internal_module_tag("_private"), None);
}
