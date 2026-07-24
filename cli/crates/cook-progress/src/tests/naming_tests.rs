use super::*;

#[test]
fn double_underscore_marks_internal() {
    assert!(is_internal_recipe("__cc_config_header__build_dhewm3_config_h"));
    assert!(!is_internal_recipe("idLib"));
    assert!(!is_internal_recipe("_private"));
}

#[test]
fn internal_recipe_displays_module_tag() {
    assert_eq!(display_recipe_name("__cc_config_header__build_dhewm3_config_h"), "cc");
    assert_eq!(display_recipe_name("__pnpm_install__web"), "pnpm");
}

#[test]
fn user_recipe_displays_as_is() {
    assert_eq!(display_recipe_name("idLib"), "idLib");
    assert_eq!(display_recipe_name("web:build"), "web:build");
}

#[test]
fn degenerate_internal_name_falls_back_to_raw() {
    assert_eq!(display_recipe_name("__"), "__");
    assert_eq!(display_recipe_name("___x"), "___x");
}

#[test]
fn probe_module_extracts_tag() {
    assert_eq!(probe_module("probe:cc:compiler:auto"), Some("cc"));
    assert_eq!(probe_module("probe:sys:os"), Some("sys"));
    assert_eq!(probe_module("build/obj/lvm.o"), None);
    assert_eq!(probe_module("probe:"), None);
}
