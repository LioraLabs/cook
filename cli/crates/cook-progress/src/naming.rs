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
#[path = "tests/naming_tests.rs"]
mod tests;
