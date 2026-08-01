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
///
/// COOK-411: this used to test the whole string while `cook-cli`'s completion
/// tested the last dotted segment, so a workspace-qualified
/// `game.__cc_config_header__x` was hidden from completion and rendered raw
/// here in the same session. The law lives in `cook-contracts` now.
pub fn is_internal_recipe(name: &str) -> bool {
    cook_contracts::naming::is_internal_recipe(name)
}

/// Friendly display name for a recipe. Internal recipes display as their
/// module tag (`__cc_config_header__x` → `cc`); user recipes display as-is.
///
/// Segment-aware with the predicate above, so a qualified internal name also
/// displays as its tag rather than as the raw minted identifier.
pub fn display_recipe_name(name: &str) -> String {
    cook_contracts::naming::internal_module_tag(name)
        .map(str::to_owned)
        .unwrap_or_else(|| name.to_string())
}

/// If `display` names a probe node (`probe:<module>:<key>`), return the
/// module tag (`cc`); otherwise `None`.
pub fn probe_module(display: &str) -> Option<&str> {
    // COOK-392: the parse half of contracts' probe_label pair.
    let key = cook_contracts::unit::parse_probe_label(display)?;
    let module = key.split(':').next().unwrap_or("");
    if module.is_empty() { None } else { Some(module) }
}

#[cfg(test)]
#[path = "tests/naming_tests.rs"]
mod tests;
