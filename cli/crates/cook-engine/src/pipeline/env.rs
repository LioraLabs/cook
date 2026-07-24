//! Environment resolution: layered variable loading.
//!
//! Layer order (later wins):
//!   1. System env
//!   2. .env file (dotenvy)
//!   3. Caller-supplied `KEY=VALUE` overrides (e.g. CLI `--set` flags)
//!
//! Cookfile-defined variables live inside `config ... end` Lua blocks
//! and are applied at runtime. Layer (3) is reapplied to `cook.env` after
//! the config block runs, so explicit CLI overrides win over config-block
//! defaults regardless of how the block was authored. See
//! `parse_cli_overrides` for the helper that exposes layer (3) separately.

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
    for (k, v) in parse_cli_overrides(overrides)? {
        env.insert(k, v);
    }

    Ok(env)
}

/// Parse `KEY=VALUE` override strings (typically the CLI `--set` flags) into
/// a map. This is layer (3) of [`resolve_env`] in isolation; the engine needs
/// it as a separate input so it can re-apply CLI overrides on top of any
/// values a `config` block writes to `cook.env`.
pub fn parse_cli_overrides(
    overrides: &[String],
) -> Result<HashMap<String, String>, PipelineError> {
    let mut map = HashMap::new();
    for set_arg in overrides {
        if let Some(eq_pos) = set_arg.find('=') {
            let key = set_arg[..eq_pos].to_string();
            let value = set_arg[eq_pos + 1..].to_string();
            map.insert(key, value);
        } else {
            return Err(PipelineError::InvalidSet(set_arg.clone()));
        }
    }
    Ok(map)
}

#[cfg(test)]
#[path = "tests/env_tests.rs"]
mod tests;
