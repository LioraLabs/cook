//! What `use foo` binds, and which door it binds through (§12.1, §24.2).
//!
//! A `use` declaration is answered by two crates that never call each other.
//! `cook-luagen` COMPOSES the binding — `local foo = cook.load_module("foo")` —
//! into the register-phase chunk and into every execute-phase body that names
//! the alias. `cook-lua-stdlib` INSTALLS the function that binding calls, and
//! wraps `require`, the second door module code can enter through. Emitter and
//! consumer of one literal, in different crates: exactly the shape this crate's
//! admission bar names.
//!
//! Drift here is not a broken build. CS-0204 keys a unit on the module source it
//! actually loaded, and it observes that at these two doors. A binding composed
//! against a name the loader no longer installs stops going through the observed
//! door, so the module quietly leaves the unit's determinant set and a machine
//! serves a shared-store answer produced under different module code. That is
//! the worst failure class Cook has, and it would present as a rename.
//!
//! Deliberately NOT here: the loader itself, the observer, and the `package.path`
//! composition. Those are mlua-bearing or effectful; their home is
//! `cook-lua-stdlib` and `layout`. This module holds only the pure decisions —
//! the alias a name derives, the door names, and the text of the binding.

use crate::lua_string::escape_double_quoted;

/// The field `cook.load_module` is installed under on the `cook` table.
///
/// Spelled without the `cook.` prefix for the same reason
/// [`crate::registration::REGISTER_SURFACE_NAME`] is: the installer sets a
/// field, and only the emitter writes the qualified call.
pub const LOAD_MODULE_FN: &str = "load_module";

/// Lua's own `require` — the module system's other door.
///
/// A multi-file rock reaches its own submodules through it, and a native
/// `.so` can be reached NO other way, since [`crate::layout::module_candidates`]
/// probes four `.lua` paths and nothing else.
///
/// It sits beside [`LOAD_MODULE_FN`] because the decision CS-0204's soundness
/// rests on is the PAIR: these two names are the complete set of doors module
/// code can enter through, so a determinant set observed at both is complete.
/// Naming one and inlining the other would leave that argument half-written
/// down. Today it has a single consumer, the observer in `cook-lua-stdlib`
/// that wraps it — a weaker claim on this crate than `LOAD_MODULE_FN`'s
/// emitter/consumer pair, and recorded here rather than dressed up.
pub const REQUIRE_FN: &str = "require";

/// The Lua identifier a module name binds (§12.1): each ASCII hyphen becomes an
/// underscore, everything else `BARE_IDENTIFIER` admits passes through. The
/// on-disk name is NOT rewritten (Note 12.1.1).
///
/// Note that today's `use` surface cannot reach the hyphen case: CS-0035 made
/// `cook-lang`'s lexer reject a `use` name that is not already a Lua identifier
/// (`check_use_name`), so §12.1's rewrite and Note 12.1.1's `use my-mod`
/// example describe something no conforming Cookfile can currently express —
/// a Standard/implementation divergence that predates this module and is not
/// this module's to settle (COOK-436). The rule is spelled here anyway because
/// it is the Standard's, because it is the derivation COOK-431's path-form
/// `use` needs for a basename, and because a name reached from a path has not
/// been through `check_use_name`.
pub fn alias_of(module_name: &str) -> String {
    module_name.replace('-', "_")
}

/// The resolver call, qualified and with the module name escaped as a
/// double-quoted Lua literal: `cook.load_module("my-mod")`.
pub fn load_module_call(module_name: &str) -> String {
    format!(
        "cook.{}(\"{}\")",
        LOAD_MODULE_FN,
        escape_double_quoted(module_name)
    )
}

/// The whole binding statement, with no terminator and no trailing newline:
/// `local my_mod = cook.load_module("my-mod")`.
///
/// How the statement is framed — its line terminator, whether a generated-by
/// comment follows it, whether several are `; `-joined onto one line to keep
/// CS-0126's line alignment — is the emitter's business and stays in
/// `cook-luagen`. The statement itself is law.
pub fn binding(module_name: &str) -> String {
    format!(
        "local {} = {}",
        alias_of(module_name),
        load_module_call(module_name)
    )
}

#[cfg(test)]
#[path = "tests/module_binding_tests.rs"]
mod tests;
