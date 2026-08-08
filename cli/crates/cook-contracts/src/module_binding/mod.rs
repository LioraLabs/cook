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
/// probes `.lua` paths and nothing else.
///
/// It sits beside [`LOAD_MODULE_FN`] because the decision CS-0204's soundness
/// rests on is the PAIR: these two names are the complete set of doors module
/// code can enter through, so a determinant set observed at both is complete.
/// Naming one and inlining the other would leave that argument half-written
/// down. Today it has a single consumer, the observer in `cook-lua-stdlib`
/// that wraps it — a weaker claim on this crate than `LOAD_MODULE_FN`'s
/// emitter/consumer pair, and recorded here rather than dressed up.
pub const REQUIRE_FN: &str = "require";

/// The suffix that separates a `use` declaration's two forms (App. A.2,
/// CS-0206). An argument ending in it is a PATH; anything else is a NAME.
pub const PATH_TARGET_SUFFIX: &str = ".lua";

/// Which form of `use` — and therefore which resolution rule — a target
/// selects (App. A.2, §12.1).
///
/// The discriminator is the `.lua` suffix rather than "does this fail to lex
/// as a Lua identifier". The two are NOT equivalent in the case that matters:
/// CS-0035 rejects `use my-mod` as a malformed *name*, and a lex-failure
/// discriminator would silently reclassify it as a path missing a suffix,
/// trading a diagnostic that names the real mistake for one that does not. A
/// name admits no `.` at all, so the suffix rule makes the two forms disjoint
/// by construction and nothing is tried and backtracked.
///
/// It is spelled here rather than in `layout` because both ends of the door
/// need it: `cook-lang` decides it at parse time for a `use` declaration, and
/// `cook-lua-stdlib` decides it at run time for a `cook.load_module` argument
/// a body computed. Two answers to that question would put a Cookfile's
/// meaning at odds with the resolver's.
pub fn is_path_target(target: &str) -> bool {
    target.ends_with(PATH_TARGET_SUFFIX)
}

/// The Lua identifier a module name binds (§12.1): each ASCII hyphen becomes an
/// underscore, everything else passes through. The on-disk name is NOT
/// rewritten (Note 12.1.1).
///
/// This rewrite had no reachable input until CS-0206, which is what COOK-436
/// reported: CS-0035 made `cook-lang`'s lexer reject a `use` NAME that is not
/// already a Lua identifier (`check_use_name`), so §12.1's rewrite described a
/// spelling no conforming Cookfile could express. Deleting it was refused in
/// favour of giving it its input. A path form's alias is derived from a file
/// BASENAME ([`derived_alias`]), which passes through no name production and
/// is under no obligation to be an identifier, so `use ./build/my-helpers.lua`
/// binds `my_helpers` — the rule as written, arriving at its first live case.
/// The name-form clause is retained for a future name production that admits
/// hyphens; today it is a no-op there.
pub fn alias_of(module_name: &str) -> String {
    module_name.replace('-', "_")
}

/// The identifier a path-form `use` binds when the author wrote no explicit
/// alias (§12.1, CS-0206): the path's final segment with its `.lua` suffix
/// removed, through [`alias_of`].
///
/// The result is NOT guaranteed to be a Lua identifier — `./build/9lives.lua`
/// derives `9lives` — and this function does not judge it. Validation belongs
/// to the caller that owns the diagnostic: `cook-lang` rejects the declaration
/// and names the explicit-alias form as the remedy. Returning a bad identifier
/// rather than an error keeps this a pure derivation with one job, and keeps
/// the "what is a legal Lua identifier" rule in the one place that already
/// answers it for `use_name`.
pub fn derived_alias(path_target: &str) -> String {
    let stem = path_target
        .rsplit('/')
        .next()
        .unwrap_or(path_target)
        .strip_suffix(PATH_TARGET_SUFFIX)
        .unwrap_or(path_target);
    alias_of(stem)
}

/// What a module is CALLED, independent of what a Cookfile bound it to
/// (§12.7.8, CS-0206).
///
/// A name form's identity is the name. A path form's is the file's basename
/// with `.lua` removed — the disk name, NOT put through [`alias_of`], because
/// this is the module naming itself rather than a Lua local being declared
/// (Note 12.1.1).
///
/// This is a different question from both of its neighbours and the three
/// answers genuinely differ. [`derived_alias`] asks what identifier the
/// CALLER binds, and rewrites hyphens because the answer has to be a Lua
/// local. `layout::module_memo_key` asks which load this IS, and takes the
/// whole target because two files with one basename in two directories are
/// two modules. This asks what the module may call itself: §12.7.8 checks a
/// module-registered chore's namespace prefix against it, and a module that
/// answered `lua/cook_demo.lua` could register nothing at all — every legal
/// prefix would have to contain a `/`, which no chore name may.
pub fn module_identity(target: &str) -> &str {
    if !is_path_target(target) {
        return target;
    }
    let base = target.rsplit('/').next().unwrap_or(target);
    base.strip_suffix(PATH_TARGET_SUFFIX).unwrap_or(base)
}

/// The resolver call, qualified and with the target escaped as a
/// double-quoted Lua literal: `cook.load_module("my-mod")`,
/// `cook.load_module("build/helpers.lua")`.
pub fn load_module_call(target: &str) -> String {
    format!(
        "cook.{}(\"{}\")",
        LOAD_MODULE_FN,
        escape_double_quoted(target)
    )
}

/// The whole binding statement, with no terminator and no trailing newline:
/// `local my_mod = cook.load_module("my-mod")`.
///
/// Both `use` forms compose through here, and both pass the SAME `target` on
/// to the resolver they name (§24.2, CS-0206). A path form does not get a
/// second entry point of its own: a module bound by any route other than
/// [`LOAD_MODULE_FN`] leaves CS-0204's determinant set, and a path-loaded
/// module is exactly the kind of source an author edits most often.
///
/// `alias` is supplied rather than derived because the two forms derive it
/// differently — a name IS its alias after [`alias_of`], a path's is explicit
/// or comes from [`derived_alias`] — and because only the parser can tell the
/// two apart while it still holds the declaration. How the statement is framed
/// — its line terminator, whether a generated-by comment follows it, whether
/// several are `; `-joined onto one line to keep CS-0126's line alignment — is
/// the emitter's business and stays in `cook-luagen`. The statement itself is
/// law.
pub fn binding(alias: &str, target: &str) -> String {
    format!("local {} = {}", alias, load_module_call(target))
}

#[cfg(test)]
#[path = "tests/module_binding_tests.rs"]
mod tests;
