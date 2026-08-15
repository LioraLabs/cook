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

// ---------------------------------------------------------------------------
// The bare-name character class (COOK-421)
// ---------------------------------------------------------------------------

/// True when `c` may START a bare name: App. A's
/// `BARE_IDENTIFIER ::= /[A-Za-z_][A-Za-z0-9_.\-]*/`, and `TOOL_NAME`, which
/// is the same production under a different name.
pub fn is_bare_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// True when `c` may CONTINUE a bare name.
///
/// One production, FIVE spellings before this existed: `cook-lang`'s lexer
/// (which parses `recipe NAME`, `chore NAME` and `config NAME`), `cook-lang`'s
/// top-level `tools` declaration validator (the live `TOOL_NAME` check), `cook-cli`'s argv
/// partitioner (which decides whether `@foo.bar` is a preset selector),
/// `probe_key::is_tool_name`, and a diagnostic heuristic in `cook-plan`.
///
/// The count was four for a while, and the fifth is the interesting one: a
/// review found that `is_tool_name` had no production caller at all, so the
/// first pass consolidated a dead function and left the live `TOOL_NAME`
/// validator forked. Counting the copies you can see is not the same as
/// counting the copies.
///
/// The lexer's and the CLI's are the pair that MUST agree — a preset name is
/// declared in a Cookfile and selected on the command line, so a class one end
/// admits and the other refuses is a preset the user can name and cannot
/// select.
///
/// The `.` is what separates this from a Lua identifier (§12.1's `LUA_IDENT`,
/// which is `[A-Za-z0-9_]` only), and it is deliberate: a recipe name is
/// dotted by workspace composition and an executable name may carry one.
pub fn is_bare_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}

/// True when `s` is a well-formed bare name: a start character followed by any
/// number of continue characters, and not empty.
pub fn is_bare_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_bare_name_start) && chars.all(is_bare_name_char)
}
