use super::*;

#[test]
fn hash_fields_serialize_as_lowercase_hex_strings() {
    let entry = StepEntry {
        inputs: vec![FileRecord {
            path: "src/main.c".into(),
            mtime: 1700000000123,
            hash: 0x1234567890abcdef,
        }],
        outputs: vec![],
        command_hash: 0x0102030405060708,
        env_contribution: 0,
        seal_contribution: 0,
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

/// COOK-360: one version for every record store, owned by cook-contracts.
#[test]
fn cache_version_is_the_shared_record_schema_version() {
    assert_eq!(CACHE_VERSION, cook_contracts::cache::record::RECORD_SCHEMA_VERSION);
}

#[test]
fn seal_contribution_round_trips_as_hex() {
    let entry = StepEntry {
        inputs: vec![],
        outputs: vec![],
        command_hash: 0x0102030405060708,
        env_contribution: 0,
        seal_contribution: 0xAABBCCDDEEFF0011,
    };
    let s = toml::to_string(&entry).expect("toml serialize");
    assert!(s.contains(r#"seal_contribution = "aabbccddeeff0011""#), "got: {s}");
    let back: StepEntry = toml::from_str(&s).expect("toml deserialize");
    assert_eq!(entry, back);
}
