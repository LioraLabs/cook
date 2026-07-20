//! Display-naming rules shared by the renderers.
//!
//! Two conventions feed progress output:
//!
//! - **Internal recipes** follow the double-underscore convention:
//!   `__<module>_...` marks a recipe minted by module tooling rather than
//!   declared by the user (`__cc_config_header__build_dhewm3_config_h` is
//!   internal cc tooling). Progress output shows the module tag, never the
//!   raw minted identifier, and suppresses the recipe's queued/summary rows.
//! - **Probe nodes** are named `probe:<module>:<key>` and carry no declared
//!   outputs. Renderers group a recipe's probes into a single
//!   `Resolved <module> toolchain` line instead of one row per probe.

/// True when the recipe name marks an internal tooling recipe
/// (double-underscore convention).
pub fn is_internal_recipe(name: &str) -> bool {
    name.starts_with("__")
}

/// Friendly display name for a recipe. Internal recipes display as their
/// module tag (`__cc_config_header__x` → `cc`); user recipes display as-is.
pub fn display_recipe_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("__") {
        let module = rest.split('_').next().unwrap_or("");
        if !module.is_empty() {
            return module.to_string();
        }
    }
    name.to_string()
}

/// If `display` names a probe node (`probe:<module>:<key>`), return the
/// module tag (`cc`); otherwise `None`.
pub fn probe_module(display: &str) -> Option<&str> {
    let key = display.strip_prefix("probe:")?;
    let module = key.split(':').next().unwrap_or("");
    if module.is_empty() { None } else { Some(module) }
}

#[cfg(test)]
mod tests {
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
}
