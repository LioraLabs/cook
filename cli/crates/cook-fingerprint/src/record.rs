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
/// (`<recipe>.toml`) with u64 hash fields as lowercase hex strings.
pub const CACHE_VERSION: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepEntry {
    pub inputs: Vec<FileRecord>,
    pub outputs: Vec<FileRecord>,
    #[serde(with = "hex_u64")]
    pub command_hash: u64,
    #[serde(with = "hex_u64")]
    pub env_contribution: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileRecord {
    pub path: String,
    pub mtime: u64,
    #[serde(with = "hex_u64")]
    pub hash: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_fields_serialize_as_lowercase_hex_strings() {
        let entry = StepEntry {
            inputs: vec![FileRecord {
                path: "src/main.c".to_string(),
                mtime: 1700000000123,
                hash: 0x1234567890abcdef,
            }],
            outputs: vec![],
            command_hash: 0x0102030405060708,
            env_contribution: 0,
        };
        let s = toml::to_string(&entry).expect("toml serialize");
        assert!(s.contains(r#"command_hash = "0102030405060708""#), "got: {s}");
        assert!(s.contains(r#"env_contribution = "0000000000000000""#), "got: {s}");
        assert!(s.contains(r#"hash = "1234567890abcdef""#), "got: {s}");
        // mtime is a timestamp, not a hash — it stays a TOML integer.
        assert!(s.contains("mtime = 1700000000123"), "got: {s}");
        let back: StepEntry = toml::from_str(&s).expect("toml deserialize");
        assert_eq!(entry, back);
    }

    #[test]
    fn hex_deserialize_rejects_non_hex() {
        let bad = r#"
inputs = []
outputs = []
command_hash = "not-hex"
env_contribution = "00"
"#;
        assert!(toml::from_str::<StepEntry>(bad).is_err());
    }

    #[test]
    fn hex_deserialize_rejects_17_digit_overflow() {
        // 17 hex digits exceed u64::MAX — from_str_radix returns Err.
        let bad = r#"
inputs = []
outputs = []
command_hash = "10000000000000000"
env_contribution = "00"
"#;
        assert!(toml::from_str::<StepEntry>(bad).is_err());
    }

    #[test]
    fn hex_deserialize_rejects_empty_string() {
        let bad = r#"
inputs = []
outputs = []
command_hash = ""
env_contribution = "00"
"#;
        assert!(toml::from_str::<StepEntry>(bad).is_err());
    }

    #[test]
    fn hex_deserialize_accepts_uppercase() {
        // Postel leniency: uppercase hex in reader is fine even though the
        // writer always emits lowercase.
        let src = r#"
inputs = []
outputs = []
command_hash = "DEADBEEFCAFE0001"
env_contribution = "00"
"#;
        let entry: StepEntry = toml::from_str(src).expect("uppercase hex should parse");
        assert_eq!(entry.command_hash, 0xDEADBEEFCAFE0001u64);
    }
}
