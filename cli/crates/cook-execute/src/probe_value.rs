//! Lua↔JSON walkers for the execute-phase probe dispatch (§22.5.5, CS-0102).
//!
//! The walkers live in `cook_lua_stdlib::json_codec` — the single home for
//! both phases (COOK-388). This module survives as the crate-local import
//! path; the register pre-pass and the worker pool serialize probe values
//! into the same store and the same `seal_contribution` fingerprint, so the
//! one implementation is what guarantees they agree byte-for-byte.
//!
//! `encode_canonical_json` and `decode_json` come from
//! `cook_contracts::probe_value` and are shared across all crates.

pub use cook_lua_stdlib::json_codec::{json_to_lua, lua_to_json};
