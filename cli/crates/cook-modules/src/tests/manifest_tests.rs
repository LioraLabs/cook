use super::*;
use std::io::Write;

fn write_cook_toml(contents: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().expect("tempfile");
    f.write_all(contents.as_bytes()).expect("write");
    f
}

#[test]
fn empty_file_yields_empty_manifest_and_default_registry() {
    let f = write_cook_toml("");
    let (m, r) = parse_cook_toml(f.path()).expect("parse");
    assert!(m.modules.is_empty());
    assert_eq!(r, ManifestRegistry::default());
}

#[test]
fn modules_only() {
    let f = write_cook_toml(
        r#"
[modules]
cook_smoke  = "*"
"lua-cjson" = "2.1.*"
argparse    = ">=0.7"
"#,
    );
    let (m, r) = parse_cook_toml(f.path()).expect("parse");
    assert_eq!(m.modules.get("cook_smoke").map(String::as_str), Some("*"));
    assert_eq!(m.modules.get("lua-cjson").map(String::as_str), Some("2.1.*"));
    assert_eq!(m.modules.get("argparse").map(String::as_str), Some(">=0.7"));
    assert_eq!(m.modules.len(), 3);
    assert_eq!(r, ManifestRegistry::default());
}

#[test]
fn registry_indexes_array() {
    let f = write_cook_toml(
        r#"
[registry]
indexes = ["https://rocks.usecook.com", "https://luarocks.org"]
"#,
    );
    let (_m, r) = parse_cook_toml(f.path()).expect("parse");
    assert_eq!(
        r.indexes,
        vec![
            "https://rocks.usecook.com".to_string(),
            "https://luarocks.org".to_string(),
        ]
    );
}

#[test]
fn empty_indexes_falls_through_to_default() {
    let f = write_cook_toml(
        r#"
[registry]
indexes = []
"#,
    );
    let (_m, r) = parse_cook_toml(f.path()).expect("parse");
    assert_eq!(r, ManifestRegistry::default());
}

#[test]
fn unknown_registry_keys_are_silently_ignored() {
    // Forward-compat with old cook.toml files that still carry the
    // historical [registry].url field from pre-Phase-3 cook.toml.
    let f = write_cook_toml(
        r#"
[registry]
url = "https://example.test/legacy"
indexes = ["https://rocks.usecook.com"]
"#,
    );
    let (_m, r) = parse_cook_toml(f.path()).expect("parse");
    assert_eq!(r.indexes, vec!["https://rocks.usecook.com".to_string()]);
}

#[test]
fn malformed_toml_errors() {
    let f = write_cook_toml("[modules\n");
    let err = parse_cook_toml(f.path()).expect_err("must fail");
    assert!(format!("{:#}", err).contains("parse"));
}

#[test]
fn non_string_constraint_rejected() {
    // `cook_smoke = 1` would deserialize as integer; we want strings only.
    let f = write_cook_toml(
        r#"
[modules]
cook_smoke = 1
"#,
    );
    let err = parse_cook_toml(f.path()).expect_err("must fail");
    let msg = format!("{:#}", err);
    assert!(msg.contains("parse"), "expected parse error, got: {msg}");
}

#[test]
fn constraint_round_trip_byte_identical() {
    // Whatever the user wrote ends up byte-identical in the BTreeMap.
    let f = write_cook_toml(
        r#"
[modules]
cook_smoke = ">= 1.0, < 2.0"
"#,
    );
    let (m, _r) = parse_cook_toml(f.path()).expect("parse");
    assert_eq!(
        m.modules.get("cook_smoke").map(String::as_str),
        Some(">= 1.0, < 2.0")
    );
}

#[test]
fn default_registry_has_documented_indexes() {
    let r = ManifestRegistry::default();
    assert_eq!(
        r.indexes,
        vec![
            "https://rocks.usecook.com".to_string(),
            "https://luarocks.org".to_string(),
        ]
    );
}
