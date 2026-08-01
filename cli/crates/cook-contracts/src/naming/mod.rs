//! What a `__`-prefixed registered name means (COOK-411).
//!
//! Module tooling mints recipes the user never wrote:
//! `__cc_config_header__build_dhewm3_config_h` is `cook_cc` generating a
//! config header, not something anyone typed. Two surfaces have to agree on
//! which names those are: shell completion hides them from candidates, and
//! progress renders them as their module tag rather than the raw identifier.
//!
//! They did not agree. `cook-cli`'s completion tested the last dotted segment,
//! `cook-progress` tested the whole string, so a workspace-qualified
//! `game.__cc_config_header__x` was hidden from completion and rendered raw by
//! progress in the same session. Neither crate knew the other answered the
//! question.
//!
//! The segment test is the correct one: a name acquires its namespace prefix
//! from composition (§11), and composing a Cookfile into a workspace does not
//! make its internal recipes user-facing.
//!
//! Pure `&str` in, no IO.

/// True when `name` marks a recipe minted by module tooling rather than
/// declared by a user.
///
/// Tests the last dotted segment, so both a bare `__cc_x` and a qualified
/// `game.__cc_x` are recognised.
pub fn is_internal_recipe(name: &str) -> bool {
    last_segment(name).starts_with("__")
}

/// The module tag an internal recipe belongs to: `__cc_config_header__x` is
/// `cc`, and so is `game.__cc_config_header__x`.
///
/// `None` when the name is not internal, or is `__` with nothing after it.
pub fn internal_module_tag(name: &str) -> Option<&str> {
    let rest = last_segment(name).strip_prefix("__")?;
    let module = rest.split('_').next().unwrap_or("");
    (!module.is_empty()).then_some(module)
}

/// The part after the final `.`, or the whole string when there is no `.`.
fn last_segment(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

#[cfg(test)]
#[path = "tests/naming_tests.rs"]
mod tests;
