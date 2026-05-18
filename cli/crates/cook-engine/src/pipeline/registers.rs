//! Workspace-level register pass: invokes `cook_register::register_cookfile`
//! once per Cookfile (root + each import), merges per-import results into a
//! single [`RegisteredWorkspace`] with qualified names, units, probes, and
//! per-Cookfile env / working-dir / alias-dirs entries.
//!
//! This is the pipeline-layer entry point that replaces today's
//! `build_*_registries` helpers (SHI-222 CS-0077 Phase 5 Task 5.1). The CLI
//! commands in subsequent Phase 5 tasks (`cmd_run`, `cmd_test`, `cmd_dag`)
//! migrate to call one of these two helpers and then hand the resulting
//! `RegisteredWorkspace` to `cook_engine::run::run`.
//!
//! Two entry points:
//!
//! - [`register_single_cookfile`] — for single-Cookfile projects (no imports).
//!   Skips the `Workspace::load` walk; takes the cookfile dir + a Lua source
//!   string directly and produces a `RegisteredWorkspace` with one entry
//!   (root, empty qualified prefix).
//! - [`register_workspace`] — for multi-Cookfile workspaces. Iterates the
//!   root + every import in `Workspace::imports`, calling `register_cookfile`
//!   on each, then merges the per-import results.
//!
//! The merge logic prefixes each registered name, unit key, and probe key
//! with the import's qualified prefix (`""` for root). Per-Cookfile
//! `final_env`, `working_dir`, and `alias_dirs` are recorded under the same
//! prefix key.
//!
//! `cache_ctx` is threaded through to each `register_cookfile` call so that
//! probes registered during the register pass see real machine identity and
//! the env denylist (CS-0074). The CLI lifts the `CacheContext` construction
//! out of `run_inner` in Task 5.3 so the register pass observes the same
//! context the executor will later use.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use cook_register::{register_cookfile, RegisterSessionBuilder, SharedTerminalOutputs};

use super::env::{load_env, parse_cli_overrides, resolve_env};
use super::error::PipelineError;
use super::recipe_info::find_full_prefix;
use super::workspace::Workspace;
use crate::registered_workspace::RegisteredWorkspace;

/// Run the register pass for a single Cookfile (no imports).
///
/// Used by CLI commands that operate against a single Cookfile path with no
/// workspace resolution (legacy single-file paths in `cmd_run`, the
/// conformance harness, embedded library callers).
///
/// `cache_ctx` is `None` in tests and legacy call sites; production CLI
/// paths pass `Some(cache_ctx)` so probes registered during the pass observe
/// real machine identity.
pub fn register_single_cookfile(
    cookfile_dir: &Path,
    env_vars: HashMap<String, String>,
    env_overrides: &[String],
    lua_source: String,
    selected_config: Option<&str>,
    cache_ctx: Option<Arc<cook_cache::cache_ctx::CacheContext>>,
) -> Result<RegisteredWorkspace, PipelineError> {
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let builder = RegisterSessionBuilder::new(cookfile_dir.to_path_buf(), env_vars)
        .with_cli_overrides(cli_overrides)
        .with_selected_config(selected_config.map(|s| s.to_string()));
    let registered = register_cookfile(builder, &lua_source, cache_ctx)
        .map_err(|e| PipelineError::Other(e.to_string()))?;

    let mut ws = RegisteredWorkspace {
        names: registered.names,
        units_by_recipe: registered.units_by_recipe,
        probes: registered.probes,
        final_env_by_cookfile: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix: BTreeMap::new(),
    };
    ws.final_env_by_cookfile
        .insert(String::new(), registered.final_env);
    ws.working_dir_by_prefix
        .insert(String::new(), cookfile_dir.to_path_buf());
    ws.alias_dirs_by_prefix
        .insert(String::new(), BTreeMap::new());
    Ok(ws)
}

/// Run the register pass once per Cookfile in `workspace` (root + every
/// import in `Workspace::imports`) and merge the per-import results.
///
/// Names, unit keys, and probe keys are qualified with the import's prefix
/// (`""` for root). Per-Cookfile `final_env`, `working_dir`, and `alias_dirs`
/// are recorded under that same prefix key in the returned
/// [`RegisteredWorkspace`].
///
/// A single [`SharedTerminalOutputs`] is threaded through every per-Cookfile
/// builder so cross-Cookfile `cook.dep_output("alias.recipe")` lookups
/// resolve through the same backing storage. Each builder also receives the
/// canonical qualified prefix for the importer's local aliases via
/// `with_alias_qualified_prefixes`, so diamond-import targets resolve to
/// their one canonical storage key regardless of which chain reached them.
///
/// `cache_ctx` is cloned into each per-Cookfile `register_cookfile` call.
/// Task 5.3 lifts the cache_ctx construction out of `run_inner` so the
/// register pass and the executor observe the same context.
pub fn register_workspace(
    workspace: &Workspace,
    config: Option<&str>,
    env_overrides: &[String],
    cache_ctx: Option<Arc<cook_cache::cache_ctx::CacheContext>>,
) -> Result<RegisteredWorkspace, PipelineError> {
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(config, dotenv_vars, env_overrides)?;
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let shared_outputs: SharedTerminalOutputs =
        Arc::new(std::sync::Mutex::new(BTreeMap::new()));

    let mut ws = RegisteredWorkspace {
        names: Vec::new(),
        units_by_recipe: BTreeMap::new(),
        probes: BTreeMap::new(),
        final_env_by_cookfile: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix: BTreeMap::new(),
    };

    // Root Cookfile: empty qualified prefix, populated alias maps for the
    // root's direct imports.
    let root_alias_dirs = workspace.alias_dirs_for(&workspace.root.dir);
    let root_alias_qp = workspace.alias_qualified_prefixes_for(&workspace.root.dir);
    let root_builder = RegisterSessionBuilder::new(workspace.root.dir.clone(), root_env)
        .with_cli_overrides(cli_overrides.clone())
        .with_selected_config(config.map(|s| s.to_string()))
        .with_shared_terminal_outputs(shared_outputs.clone())
        .with_qualified_prefix(String::new())
        .with_alias_dirs(root_alias_dirs.clone())
        .with_alias_qualified_prefixes(root_alias_qp);
    let root_registered = register_cookfile(
        root_builder,
        &workspace.root.lua_source,
        cache_ctx.clone(),
    )
    .map_err(|e| PipelineError::Other(e.to_string()))?;
    merge_into(&mut ws, "", root_registered);
    ws.working_dir_by_prefix
        .insert(String::new(), workspace.root.dir.clone());
    ws.alias_dirs_by_prefix
        .insert(String::new(), root_alias_dirs);

    // Imports: each one gets its canonical workspace qualified prefix
    // (computed from the namespace map) and its own alias map for any
    // nested imports it declares.
    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        // Imports do not inherit the root's .env layering — each
        // sub-Cookfile gets its own env baseline. `resolve_env` here
        // still applies the system env + CLI overrides; .env files in
        // the import dir are intentionally not loaded by this helper
        // (parity with the pre-Task-5.1 path).
        let import_env = resolve_env(config, HashMap::new(), env_overrides)?;
        let alias_dirs = workspace.alias_dirs_for(&loaded.dir);
        let alias_qp = workspace.alias_qualified_prefixes_for(&loaded.dir);
        let builder = RegisterSessionBuilder::new(loaded.dir.clone(), import_env)
            .with_cli_overrides(cli_overrides.clone())
            .with_selected_config(config.map(|s| s.to_string()))
            .with_shared_terminal_outputs(shared_outputs.clone())
            .with_qualified_prefix(prefix.clone())
            .with_alias_dirs(alias_dirs.clone())
            .with_alias_qualified_prefixes(alias_qp);
        let import_registered = register_cookfile(
            builder,
            &loaded.lua_source,
            cache_ctx.clone(),
        )
        .map_err(|e| PipelineError::Other(e.to_string()))?;
        merge_into(&mut ws, &prefix, import_registered);
        ws.working_dir_by_prefix
            .insert(prefix.clone(), loaded.dir.clone());
        ws.alias_dirs_by_prefix.insert(prefix.clone(), alias_dirs);
    }

    Ok(ws)
}

/// Run the cheap [`cook_register::list_names`] path for a single Cookfile
/// (no imports) and return the registered names with their kinds.
///
/// This is the listing-surface counterpart to [`register_single_cookfile`]:
/// it loads the Cookfile, runs only register-phase Lua (no recipe bodies,
/// no probe queries), and returns just the names + kinds — enough for
/// `cook list` / `cook menu` to enumerate the full surface, including
/// Lua-registered recipes (e.g. `cook_cc.bin`).
pub fn list_single_cookfile_names(
    cookfile_dir: &Path,
    env_vars: HashMap<String, String>,
    env_overrides: &[String],
    lua_source: String,
    selected_config: Option<&str>,
) -> Result<Vec<(String, cook_register::RecipeKind)>, PipelineError> {
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let builder = RegisterSessionBuilder::new(cookfile_dir.to_path_buf(), env_vars)
        .with_cli_overrides(cli_overrides)
        .with_selected_config(selected_config.map(|s| s.to_string()));
    let names = cook_register::list_names(builder, &lua_source)
        .map_err(|e| PipelineError::Other(e.to_string()))?;
    Ok(names.into_iter().map(|n| (n.name, n.kind)).collect())
}

/// Run [`cook_register::list_names`] for every Cookfile in `workspace`
/// (root + every import) and return the qualified name set with kinds.
///
/// Workspace-level counterpart to [`register_workspace`]: each import's
/// names are prefixed with its qualified workspace prefix. Like
/// [`list_single_cookfile_names`], this avoids invoking any recipe body
/// and avoids firing probe queries — it's the cheap path used by
/// `cook list` / `cook menu`.
pub fn list_workspace_names(
    workspace: &Workspace,
    config: Option<&str>,
    env_overrides: &[String],
) -> Result<Vec<(String, cook_register::RecipeKind)>, PipelineError> {
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(config, dotenv_vars, env_overrides)?;
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let mut out: Vec<(String, cook_register::RecipeKind)> = Vec::new();

    let root_builder = RegisterSessionBuilder::new(workspace.root.dir.clone(), root_env)
        .with_cli_overrides(cli_overrides.clone())
        .with_selected_config(config.map(|s| s.to_string()));
    let root_names = cook_register::list_names(root_builder, &workspace.root.lua_source)
        .map_err(|e| PipelineError::Other(e.to_string()))?;
    for n in root_names {
        out.push((n.name, n.kind));
    }

    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        // Imports do not inherit the root's .env layering — mirror the
        // `register_workspace` policy: each sub-Cookfile starts from a
        // fresh env baseline; system env + CLI overrides still apply.
        let import_env = resolve_env(config, HashMap::new(), env_overrides)?;
        let builder = RegisterSessionBuilder::new(loaded.dir.clone(), import_env)
            .with_cli_overrides(cli_overrides.clone())
            .with_selected_config(config.map(|s| s.to_string()))
            .with_qualified_prefix(prefix.clone());
        let names = cook_register::list_names(builder, &loaded.lua_source)
            .map_err(|e| PipelineError::Other(e.to_string()))?;
        for n in names {
            out.push((format!("{prefix}.{}", n.name), n.kind));
        }
    }

    Ok(out)
}

/// Merge a per-Cookfile [`cook_register::RegisteredCookfile`] into the
/// workspace-level [`RegisteredWorkspace`], qualifying every recipe name,
/// unit key, and probe key with `prefix` (empty for the root).
fn merge_into(
    ws: &mut RegisteredWorkspace,
    prefix: &str,
    rc: cook_register::RegisteredCookfile,
) {
    let qualify = |name: &str| {
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        }
    };
    for n in rc.names {
        let mut qn = n.clone();
        qn.name = qualify(&n.name);
        ws.names.push(qn);
    }
    for (name, units) in rc.units_by_recipe {
        ws.units_by_recipe.insert(qualify(&name), units);
    }
    for (key, probe) in rc.probes {
        ws.probes.insert(
            if prefix.is_empty() {
                key
            } else {
                format!("{prefix}.{key}")
            },
            probe,
        );
    }
    ws.final_env_by_cookfile
        .insert(prefix.to_string(), rc.final_env);
}
