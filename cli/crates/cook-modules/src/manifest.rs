//! `cook.toml` `[modules]` and `[registry].indexes` parsing.
//!
//! `[modules]` is a flat TOML table mapping rock names to luarocks version
//! constraints. Cook does not invent constraint grammar — values pass through
//! to luarocks verbatim. Rock names use luarocks's allowed character set
//! (`[A-Za-z][A-Za-z0-9_.\-]*`); cook uses underscore-separated names for
//! its blessed `cook_*` modules so they are valid Lua identifiers and bare
//! TOML keys.
//!
//! `[registry].indexes` lists the rocks indexes in resolution order. Empty
//! or missing `indexes` falls through to `ManifestRegistry::default()`,
//! which is `["https://rocks.usecook.com", "https://luarocks.org"]`.
//! Unknown keys inside `[registry]` (e.g. a historical `url` field from
//! pre-Phase-3 cook.toml files) are silently ignored for forward-
//! compatibility.

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
#[path = "tests/manifest_tests.rs"]
mod tests;
