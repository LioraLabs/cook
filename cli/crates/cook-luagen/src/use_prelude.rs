//! The `use` alias binding, spelled once for both phases (§{mods.use},
//! §{lua.cook-load-module}).
//!
//! A `use foo` declaration binds `foo` in the declaring Cookfile's Lua
//! environment. At register phase that is one `local` at the top of the
//! generated chunk. At execute phase every body is its own chunk on a worker
//! VM, so the binding has to travel INSIDE each body's source — which is what
//! this module builds.
//!
//! CS-0205: the binding is prepended for the aliases the body actually names.
//! It binds a Lua `local`, and a local is reachable only by code that names it
//! lexically, so an unnamed alias is unobservable — while binding it anyway
//! would run the module's top level and `init()` on a worker VM (breaking a
//! Cookfile that `use`s a register-oriented module and never calls it at
//! execute phase) and would make that module a CS-0204 cache determinant of
//! every Lua-bodied unit in the Cookfile.
//!
//! The prelude is ONE line with no trailing newline, glued onto the body's
//! first line, so a body's line 1 is still line 1 of the chunk and no emission
//! site has to compensate for the prelude's height (§{exec.diag}).

use cook_lang::ast::UseStatement;
use cook_contracts::module_binding;

use crate::lua_scan::free_identifier_occurs;

/// The Lua identifier a `use <name>` declaration binds (Note 12.1.1).
///
/// Re-exported from `cook_contracts::module_binding` rather than spelled here:
/// the §12.1 rewrite is language law, and COOK-431's path-form `use` will need
/// the same answer from a third crate. (The hyphen case is unreachable from
/// today's `use` surface — see the note on `module_binding::alias_of`.)
pub(crate) use module_binding::alias_of;

/// The execute-phase prelude for `body`: one `local <alias> =
/// cook.load_module("<name>")` statement per `use` the body names, in
/// declaration order, `; `-joined on a single line with a trailing space.
/// Empty when the body names none of them.
pub(crate) fn execute_prelude(uses: &[UseStatement], body: &str) -> String {
    let mut out = String::new();
    let mut bound: Vec<String> = Vec::new();
    for use_stmt in uses {
        let alias = alias_of(&use_stmt.module_name);
        if bound.contains(&alias) || !free_identifier_occurs(body, &alias) {
            continue;
        }
        out.push_str(&module_binding::binding(&use_stmt.module_name));
        out.push_str("; ");
        bound.push(alias);
    }
    out
}

/// `body` with [`execute_prelude`] glued to its front. Byte-identical to
/// `body` when nothing binds.
pub(crate) fn with_execute_prelude(uses: &[UseStatement], body: &str) -> String {
    let prelude = execute_prelude(uses, body);
    if prelude.is_empty() {
        return body.to_string();
    }
    format!("{prelude}{body}")
}

#[cfg(test)]
#[path = "tests/use_prelude_tests.rs"]
mod tests;
