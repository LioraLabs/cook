//! Persistent fingerprint state types.
//!
//! `FileRecord` and `StepEntry` describe the recorded fingerprint of inputs,
//! outputs, command, context, and env for a single step. The `CACHE_VERSION`
//! constant tags every persisted RecipeCache so a schema change is rejected
//! on load (see `cook-cache::store`).

use serde::{Deserialize, Serialize};

/// Serde adapter: u64 <-> zero-padded lowercase hex string.
///
/// Used on hash/fingerprint fields of the persisted recipe index (COOK-92):
/// TOML integers are i64, so a u64 hash with the high bit set cannot
/// round-trip as a TOML integer. Hex strings are also what humans expect to
/// see when reading the index. Writers emit exactly 16 lowercase hex digits;
/// the reader accepts any width that `u64::from_str_radix(s, 16)` can parse —
/// including uppercase hex (`"DEADBEEF"`) and a leading `+` sign, per
/// `from_str_radix` semantics (Postel leniency). Strings longer than 16 hex
/// digits that overflow `u64` are rejected.
pub mod hex_u64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u64, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&format!("{v:016x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<u64, D::Error> {
        let s = String::deserialize(de)?;
        u64::from_str_radix(&s, 16).map_err(serde::de::Error::custom)
    }
}

/// Fingerprint schema version. Bump on any breaking change to `StepEntry` /
/// `FileRecord` / cache key composition. v4 (COOK-92): recipe index is TOML
/// (`<recipe>.toml`) with u64 hash fields as lowercase hex strings. v5
/// (COOK-161): the single content-addressed key folds the unit's effective
/// seal set's probe values via `StepEntry.seal_contribution` (CS-0107). v6
/// (COOK-180): per-output `mode` / symlink / empty-dir fidelity recorded in
/// `ArtifactMeta`; a discovered-inputs manifest (keyed by the declared-only
/// key) enables cold fetch-by-key sharing for depfile (cc) units.
pub const CACHE_VERSION: u32 = 6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepEntry {
    pub inputs: Vec<FileRecord>,
    pub outputs: Vec<FileRecord>,
    #[serde(with = "hex_u64")]
    pub command_hash: u64,
    #[serde(with = "hex_u64")]
    pub env_contribution: u64,
    /// COOK-161 / CS-0107: xxh3_64 of the unit's *effective seal set* rendered
    /// as sorted `key\0<canonical-json-bytes>` records joined by `\n`. Zero for
    /// a unit with an empty seal set (the unannotated / `local` / `pinned`
    /// case), keeping non-sealed entries byte-stable apart from the version bump.
    #[serde(default, with = "hex_u64")]
    pub seal_contribution: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileRecord {
    pub path: String,
    pub mtime: u64,
    #[serde(with = "hex_u64")]
    pub hash: u64,
}

#[cfg(test)]
#[path = "tests/record_tests.rs"]
mod tests;
