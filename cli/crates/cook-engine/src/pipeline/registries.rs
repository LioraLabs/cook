//! Build per-prefix `RegistryEntry` maps for `run::run`.
//!
//! `run` requires one `RegistryEntry` per namespace prefix (the empty string
//! for single-Cookfile builds; one per import for workspace builds). This
//! module assembles those maps from the resolved env layering and the
//! `Workspace` struct.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::RegistryEntry;

use super::env::{load_env, resolve_env};
use super::error::PipelineError;
use super::recipe_info::find_full_prefix;
use super::workspace::Workspace;

/// Build a single-Cookfile registry map (empty-string prefix).
///
/// `env_vars` is the fully-layered environment (system → .env → overrides);
/// callers typically obtain it from `super::env::resolve_env`.
pub fn build_single_registries(
    cookfile_dir: &Path,
    env_vars: HashMap<String, String>,
    lua_source: String,
    selected_config: Option<&str>,
) -> BTreeMap<String, RegistryEntry> {
    let registry = cook_register::Registry::new(cookfile_dir.to_path_buf(), env_vars)
        .with_selected_config(selected_config.map(|s| s.to_string()));
    let mut registries = BTreeMap::new();
    registries.insert(
        String::new(),
        RegistryEntry {
            registry,
            lua_source,
            alias_dirs: BTreeMap::new(),
        },
    );
    registries
}

/// Build workspace registries: one for root (empty prefix), one per import.
///
/// `env_overrides` is the slice of caller-supplied `KEY=VALUE` overrides
/// (typically the CLI `--set` flags). The function performs the full env
/// layering for both the root and each import internally — imported
/// Cookfiles see system env + overrides only (no `.env` file), matching the
/// previous cook-cli behavior exactly.
pub fn build_workspace_registries(
    workspace: &Workspace,
    config: Option<&str>,
    env_overrides: &[String],
) -> Result<BTreeMap<String, RegistryEntry>, PipelineError> {
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(config, dotenv_vars, env_overrides)?;

    // One shared terminal-outputs map for the entire workspace invocation.
    // All Registries write to and read from the same map, keyed by
    // fully-qualified recipe name (e.g. "lib.lib_build" or "build").
    let shared_outputs: cook_register::SharedTerminalOutputs =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));

    let mut registries: BTreeMap<String, RegistryEntry> = BTreeMap::new();

    let root_alias_dirs = workspace.alias_dirs_for(&workspace.root.dir);
    let root_alias_qp = workspace.alias_qualified_prefixes_for(&workspace.root.dir);
    // Root has empty prefix (already the default; explicit for clarity).
    let root_registry = cook_register::Registry::new(workspace.root.dir.clone(), root_env)
        .with_selected_config(config.map(|s| s.to_string()))
        .with_shared_terminal_outputs(shared_outputs.clone())
        .with_qualified_prefix(String::new())
        .with_alias_dirs(root_alias_dirs.clone())
        .with_alias_qualified_prefixes(root_alias_qp);
    registries.insert(
        String::new(),
        RegistryEntry {
            registry: root_registry,
            lua_source: workspace.root.lua_source.clone(),
            alias_dirs: root_alias_dirs,
        },
    );

    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        let import_env = resolve_env(config, HashMap::new(), env_overrides)?;
        let alias_dirs = workspace.alias_dirs_for(&loaded.dir);
        let alias_qp = workspace.alias_qualified_prefixes_for(&loaded.dir);
        let registry = cook_register::Registry::new(loaded.dir.clone(), import_env)
            .with_selected_config(config.map(|s| s.to_string()))
            .with_shared_terminal_outputs(shared_outputs.clone())
            .with_qualified_prefix(prefix.clone())
            .with_alias_dirs(alias_dirs.clone())
            .with_alias_qualified_prefixes(alias_qp);
        registries.insert(
            prefix,
            RegistryEntry {
                registry,
                lua_source: loaded.lua_source.clone(),
                alias_dirs,
            },
        );
    }

    Ok(registries)
}
