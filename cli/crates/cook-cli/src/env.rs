//! Environment resolution: layered variable loading.
//!
//! Layer order (later wins):
//!   1. System env
//!   2. Cookfile bare vars
//!   3. .env file (dotenvy)
//!   4. CLI --set flags
//!
//! Per-config env mutation is handled later by `config ... end` Lua blocks at runtime.

use std::collections::HashMap;
use std::path::Path;

use crate::error::CookError;

/// Load variables from a `.env` file in `cookfile_dir`, if present.
pub fn load_env(cookfile_dir: &Path) -> HashMap<String, String> {
    let env_path = cookfile_dir.join(".env");
    match dotenvy::from_path_iter(&env_path) {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => HashMap::new(),
    }
}

/// Merge all environment layers into a single map.
pub fn resolve_env(
    cookfile: &cook_lang::ast::Cookfile,
    selected_config: Option<&str>,
    dotenv_vars: HashMap<String, String>,
    cli_sets: &[String],
) -> Result<HashMap<String, String>, CookError> {
    // selected_config no longer overlays env vars; it flows to the runtime
    // for `config NAME ... end` Lua-block dispatch. Kept here to avoid churn
    // in call sites.
    let _ = selected_config;

    // Layer 1: system env
    let mut env: HashMap<String, String> = std::env::vars().collect();

    // Layer 2: Cookfile bare vars
    for (k, v) in &cookfile.vars {
        env.insert(k.clone(), v.clone());
    }

    // Layer 3: .env file
    for (k, v) in dotenv_vars {
        env.insert(k, v);
    }

    // Layer 4: CLI --set (split on first '=')
    for set_arg in cli_sets {
        if let Some(eq_pos) = set_arg.find('=') {
            let key = set_arg[..eq_pos].to_string();
            let value = set_arg[eq_pos + 1..].to_string();
            env.insert(key, value);
        } else {
            return Err(CookError::Other(format!(
                "--set value must be KEY=VALUE, got: {}", set_arg
            )));
        }
    }

    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_env_from_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".env"), "FOO=bar\nBAZ=qux\n").unwrap();
        let env = load_env(dir.path());
        assert_eq!(env.get("FOO").unwrap(), "bar");
        assert_eq!(env.get("BAZ").unwrap(), "qux");
    }

    #[test]
    fn test_missing_env_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let env = load_env(dir.path());
        assert!(env.is_empty());
    }

    #[test]
    fn test_comments_and_blank_lines() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(".env"),
            "# This is a comment\n\nKEY=value\n\n# Another comment\nKEY2=value2\n",
        )
        .unwrap();
        let env = load_env(dir.path());
        assert_eq!(env.len(), 2);
        assert_eq!(env.get("KEY").unwrap(), "value");
        assert_eq!(env.get("KEY2").unwrap(), "value2");
    }

    #[test]
    fn test_quoted_values() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(".env"),
            "SINGLE='hello world'\nDOUBLE=\"hello world\"\n",
        )
        .unwrap();
        let env = load_env(dir.path());
        assert_eq!(env.get("SINGLE").unwrap(), "hello world");
        assert_eq!(env.get("DOUBLE").unwrap(), "hello world");
    }
}
