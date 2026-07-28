//! The Lua *render* of a `$<IDENT>` placeholder. The parse lives in
//! `cook_contracts::sigil` and is re-exported here so this crate's many
//! `sigil::scan(...)` callers are unchanged.
//!
//! CS-0074 kept the parse and the render together on the reasoning that the
//! ident to `cook.probes.get(...)` mapping is the placeholder's own desugaring
//! and both consumers emitted Lua. That was right while it was true. CS-0188
//! deleted the register-phase rewrite, so `cook-register` no longer emits Lua
//! for a probe reference, and the execute-phase worker now needs the parse with
//! no render at all — it holds a live VM and reads the value directly rather
//! than building source to evaluate. Two phases needing the same parse and only
//! one needing the render is the boundary the old comment said nobody needed.
//!
//! What is left here is the render, in the crate whose job is emitting Lua.

pub use cook_contracts::sigil::{probe_ref, scan, PlaceholderSpan, ProbeRef, Seg};

use crate::lua_string::escape_lua_string;

/// The ready-to-emit Lua read for a probe reference:
/// `cook.probes.get("key").field[1]`.
///
/// A free function rather than a method because [`ProbeRef`] is a
/// `cook-contracts` type now, and rendering Lua is not something that crate may
/// know how to do.
pub fn lua_access(r: &ProbeRef) -> String {
    let mut access = format!("cook.probes.get(\"{}\")", escape_lua_string(r.key()));
    for seg in r.path() {
        match seg {
            Seg::Field(name) => {
                access.push('.');
                access.push_str(name);
            }
            Seg::Index(idx) => {
                access.push('[');
                access.push_str(idx);
                access.push(']');
            }
        }
    }
    access
}

#[cfg(test)]
#[path = "tests/sigil_tests.rs"]
mod tests;
