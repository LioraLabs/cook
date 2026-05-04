//! Environment resolution: layered variable loading.
//!
//! Layer order (later wins):
//!   1. System env
//!   2. .env file (dotenvy)
//!   3. Caller-supplied `KEY=VALUE` overrides (e.g. CLI `--set` flags)
//!
//! Cookfile-defined variables live inside `config ... end` Lua blocks
//! and are applied at runtime, not as part of this static layering.

use std::collections::HashMap;
use std::path::Path;

use super::error::PipelineError;

/// Load variables from a `.env` file in `cookfile_dir`, if present.
pub fn load_env(cookfile_dir: &Path) -> HashMap<String, String> {
    let env_path = cookfile_dir.join(".env");
    match dotenvy::from_path_iter(&env_path) {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => HashMap::new(),
    }
}

/// Merge all environment layers into a single map.
///
/// `selected_config` is accepted but unused: it no longer overlays env
/// vars; it flows to the runtime for `config NAME ... end` Lua-block
/// dispatch. Kept here so call sites don't churn.
pub fn resolve_env(
    selected_config: Option<&str>,
    dotenv_vars: HashMap<String, String>,
    overrides: &[String],
) -> Result<HashMap<String, String>, PipelineError> {
    let _ = selected_config;

    // Layer 1: system env
    let mut env: HashMap<String, String> = std::env::vars().collect();

    // Layer 2: .env file
    for (k, v) in dotenv_vars {
        env.insert(k, v);
    }

    // Layer 3: caller-supplied KEY=VALUE overrides (split on first '=')
    for set_arg in overrides {
        if let Some(eq_pos) = set_arg.find('=') {
            let key = set_arg[..eq_pos].to_string();
            let value = set_arg[eq_pos + 1..].to_string();
            env.insert(key, value);
        } else {
            return Err(PipelineError::InvalidSet(set_arg.clone()));
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

    #[test]
    fn test_resolve_env_invalid_set() {
        let result = resolve_env(None, HashMap::new(), &["NOT_A_PAIR".to_string()]);
        assert!(matches!(result, Err(PipelineError::InvalidSet(_))));
    }
}
