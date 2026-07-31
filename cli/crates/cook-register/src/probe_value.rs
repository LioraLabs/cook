//! Lua↔JSON walkers for the register-VM probe paths (§22.5.5, CS-0102).
//!
//! The walkers live in `cook_lua_stdlib::json_codec` — the single home for
//! both phases (COOK-388). This module survives as the crate-local import
//! path; the register pre-pass and the worker pool serialize probe values
//! into the same store and the same `seal_contribution` fingerprint, so the
//! one implementation is what guarantees they agree byte-for-byte.
//!
//! `encode_canonical_json` and `decode_json` are re-exported from
//! `cook_contracts::probe_value` so callers that import this module continue
//! to work without change.

// Re-export the encode/decode helpers from cook-contracts so callers can use
// either path interchangeably.
pub use cook_contracts::probe_value::{decode_json, encode_canonical_json};

pub use cook_lua_stdlib::json_codec::{json_to_lua, lua_to_json};
