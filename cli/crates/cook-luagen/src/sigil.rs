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
//! CS-0195 deleted the last render, `lua_access`: no emission site builds a
//! `cook.probes.get(...)` chain any more — every probe-ref position renders
//! through one substitution helper keyed by the IDENT, backed by the CS-0192
//! law in `cook_contracts::sigil::subst`. What is left here is the re-export.

pub use cook_contracts::sigil::{probe_ref, scan, PlaceholderSpan, ProbeRef, Seg};

#[cfg(test)]
#[path = "tests/sigil_tests.rs"]
mod tests;
