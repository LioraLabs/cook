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

/// True when a recipe did no work worth reporting: everything was cached
/// except, at most, the toolchain probes, which have already printed their own
/// group line.
///
/// COOK-413: `event_writer.rs` and `plain.rs` each spelled
/// `cached + probes_ran >= total`. Both renderers must make the same
/// suppression decision or a warm build is noisy in one output mode and quiet
/// in the other, for the same events.
pub fn recipe_did_no_real_work(cached: usize, probes_ran: usize, total: usize) -> bool {
    cached + probes_ran >= total
}

/// The parenthesised detail on a probe group line: `(3 probes, 1 cached)`,
/// `(1 probe)`.
///
/// COOK-413: the singular/plural choice and the `, N cached` suffix were
/// spelled in both renderers. The surrounding line differs between them (one
/// is verb-prefixed, one is a padded table row) and stays separate; this is
/// the part that must read identically.
pub fn probe_group_detail(ran: usize, cached: usize) -> String {
    let noun = if ran + cached == 1 { "probe" } else { "probes" };
    if cached > 0 {
        format!("({} {noun}, {cached} cached)", ran + cached)
    } else {
        format!("({ran} {noun})")
    }
}

#[cfg(test)]
#[path = "tests/naming_shared_tests.rs"]
mod naming_shared_tests;
