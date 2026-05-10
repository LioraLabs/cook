//! M3.1 — `cook.toml` `[modules]` and `[registry].indexes` parsing.
//!
//! `[modules]` is a flat TOML table mapping rock names to luarocks version
//! constraints. Cook does not invent constraint grammar — values pass through
//! to luarocks verbatim. Rock names use luarocks's allowed character set
//! (`[A-Za-z][A-Za-z0-9_.\-]*`); cook uses underscore-separated names for
//! its blessed `cook_*` modules so they are valid Lua identifiers and bare
//! TOML keys.
//!
//! `[registry].indexes` is the new Phase 3 array distinct from the legacy
//! Phase 1 `[registry].url` (which `cook pull` still consumes). Empty or
//! missing `indexes` falls through to `ManifestRegistry::default()`, which
//! M3.7 fills in with `["https://rocks.usecook.com", "https://luarocks.org"]`.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ManifestModules {
    pub modules: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestRegistry {
    pub indexes: Vec<String>,
}

impl Default for ManifestRegistry {
    /// Default index list when `[registry].indexes` is missing or empty.
    /// rocks.usecook.com is the cook-blessed index (CS-0062 §7 search-path);
    /// luarocks.org is the public ecosystem fallback.
    fn default() -> Self {
        Self {
            indexes: vec![
                "https://rocks.usecook.com".to_string(),
                "https://luarocks.org".to_string(),
            ],
        }
    }
}

#[derive(Deserialize)]
struct CookToml {
    #[serde(default)]
    registry: Option<RegistryRaw>,
    #[serde(default)]
    modules: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct RegistryRaw {
    // `url` is the Phase 1 legacy field consumed by `cook pull`. We deserialize
    // and discard it here so a cook.toml that has both forms still parses.
    #[allow(dead_code)]
    url: Option<String>,
    #[serde(default)]
    indexes: Option<Vec<String>>,
}

pub fn parse_cook_toml(path: &Path) -> Result<(ManifestModules, ManifestRegistry)> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let parsed: CookToml = toml::from_str(&raw)
        .with_context(|| format!("parse {}", path.display()))?;
    let modules = ManifestModules {
        modules: parsed.modules.unwrap_or_default(),
    };
    let registry = match parsed.registry {
        None => ManifestRegistry::default(),
        Some(r) => {
            let indexes = r.indexes.unwrap_or_default();
            if indexes.is_empty() {
                ManifestRegistry::default()
            } else {
                ManifestRegistry { indexes }
            }
        }
    };
    Ok((modules, registry))
}

#[cfg(test)]
mod tests {
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
    fn legacy_url_present_alongside_indexes() {
        // Phase 3 leaves `[registry].url` untouched; Phase 4 collapses these.
        // `cook modules` ignores `url` (only `cook pull` consumes it).
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
}
