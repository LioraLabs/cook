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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cook_register::{register_cookfile, RegisterSessionBuilder, SharedMemberOutputs, SharedTerminalOutputs};

use cook_lang::ast::Cookfile;

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
        .map_err(map_register_error)?;

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

/// Order the workspace's Cookfiles for the register pass so that every
/// cross-Cookfile `cook.dep_output("alias.recipe")` sees its producer already
/// registered: **importees before importers, with the root last**.
///
/// A recipe's terminal outputs are written into the shared map only *after* its
/// body runs (cook-register populates them from `last_cook_step_outputs`), and
/// a consumer body reads that same map at register time. So a consumer Cookfile
/// must be registered after every Cookfile it references. This is a post-order
/// DFS over the import DAG (acyclic — §11.5 rejects import cycles): post-order
/// emits a node only after all its importees, and the root (which imports but is
/// imported by no one) emits last. Returns canonical directory paths.
fn cookfile_registration_order(workspace: &Workspace) -> Vec<PathBuf> {
    fn canon(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }

    // Import-DAG adjacency: importer dir -> [importee dir, ...] (deduped,
    // declaration order preserved).
    let mut adj: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for (parent, _alias, target) in &workspace.namespace_map {
        let entry = adj.entry(canon(parent)).or_default();
        let t = canon(target);
        if !entry.contains(&t) {
            entry.push(t);
        }
    }

    fn visit(
        node: PathBuf,
        adj: &BTreeMap<PathBuf, Vec<PathBuf>>,
        visited: &mut BTreeSet<PathBuf>,
        order: &mut Vec<PathBuf>,
    ) {
        if !visited.insert(node.clone()) {
            return;
        }
        if let Some(children) = adj.get(&node) {
            for child in children {
                visit(child.clone(), adj, visited, order);
            }
        }
        order.push(node);
    }

    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    let mut order: Vec<PathBuf> = Vec::new();
    visit(canon(&workspace.root.dir), &adj, &mut visited, &mut order);

    // Safety net: register any import not reachable from root (should not occur,
    // since the workspace is built by walking imports outward from root).
    for path in workspace.imports.keys() {
        let c = canon(path);
        if visited.insert(c.clone()) {
            order.push(c);
        }
    }

    order
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
    let shared_member_outputs: SharedMemberOutputs =
        Arc::new(std::sync::Mutex::new(BTreeMap::new()));

    let mut ws = RegisteredWorkspace {
        names: Vec::new(),
        units_by_recipe: BTreeMap::new(),
        probes: BTreeMap::new(),
        final_env_by_cookfile: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix: BTreeMap::new(),
    };

    // Register Cookfiles importees-first / root-last so every cross-Cookfile
    // `cook.dep_output` call sees its producer's terminal outputs already
    // populated in the shared map (see `cookfile_registration_order`).
    let root_canon = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    for dir in cookfile_registration_order(workspace) {
        if dir == root_canon {
            // Root Cookfile: empty qualified prefix, populated alias maps for
            // the root's direct imports.
            let root_alias_dirs = workspace.alias_dirs_for(&workspace.root.dir);
            let root_alias_qp =
                workspace.alias_qualified_prefixes_for(&workspace.root.dir);
            let root_builder =
                RegisterSessionBuilder::new(workspace.root.dir.clone(), root_env.clone())
                    .with_cli_overrides(cli_overrides.clone())
                    .with_selected_config(config.map(|s| s.to_string()))
                    .with_shared_terminal_outputs(shared_outputs.clone())
                    .with_shared_member_outputs(shared_member_outputs.clone())
                    .with_qualified_prefix(String::new())
                    .with_alias_dirs(root_alias_dirs.clone())
                    .with_alias_qualified_prefixes(root_alias_qp.clone());
            let root_registered = register_cookfile(
                root_builder,
                &workspace.root.lua_source,
                cache_ctx.clone(),
            )
            .map_err(map_register_error)?;
            merge_into(&mut ws, "", &root_alias_qp, root_registered);
            ws.working_dir_by_prefix
                .insert(String::new(), workspace.root.dir.clone());
            ws.alias_dirs_by_prefix
                .insert(String::new(), root_alias_dirs);
        } else if let Some(loaded) = workspace.imports.get(&dir) {
            // Imports: each one gets its canonical workspace qualified prefix
            // (computed from the namespace map) and its own alias map for any
            // nested imports it declares. Imports do not inherit the root's
            // .env layering — each sub-Cookfile gets its own env baseline.
            let prefix = find_full_prefix(workspace, &dir);
            let import_env = resolve_env(config, HashMap::new(), env_overrides)?;
            let alias_dirs = workspace.alias_dirs_for(&loaded.dir);
            let alias_qp = workspace.alias_qualified_prefixes_for(&loaded.dir);
            let builder = RegisterSessionBuilder::new(loaded.dir.clone(), import_env)
                .with_cli_overrides(cli_overrides.clone())
                .with_selected_config(config.map(|s| s.to_string()))
                .with_shared_terminal_outputs(shared_outputs.clone())
                .with_shared_member_outputs(shared_member_outputs.clone())
                .with_qualified_prefix(prefix.clone())
                .with_alias_dirs(alias_dirs.clone())
                .with_alias_qualified_prefixes(alias_qp.clone());
            let import_registered = register_cookfile(
                builder,
                &loaded.lua_source,
                cache_ctx.clone(),
            )
            .map_err(map_register_error)?;
            merge_into(&mut ws, &prefix, &alias_qp, import_registered);
            ws.working_dir_by_prefix
                .insert(prefix.clone(), loaded.dir.clone());
            ws.alias_dirs_by_prefix.insert(prefix.clone(), alias_dirs);
        }
    }

    Ok(ws)
}

/// Variant of [`register_single_cookfile`] that binds argv to the targeted
/// recipe / chore (COOK-36 Task 4).
///
/// `target` is the unqualified recipe or chore name being dispatched.
/// `argv` contains the positional arguments following the chore name on the
/// CLI. For normal recipes, `argv` must be empty; a non-empty `argv` will
/// surface `RegisterError::RecipeWithArgv` at body-invocation time.
#[allow(clippy::too_many_arguments)]
pub fn register_single_cookfile_with_argv(
    cookfile_dir: &Path,
    env_vars: HashMap<String, String>,
    env_overrides: &[String],
    lua_source: String,
    selected_config: Option<&str>,
    target: &str,
    argv: &[String],
    cache_ctx: Option<Arc<cook_cache::cache_ctx::CacheContext>>,
) -> Result<RegisteredWorkspace, PipelineError> {
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let builder = RegisterSessionBuilder::new(cookfile_dir.to_path_buf(), env_vars)
        .with_cli_overrides(cli_overrides)
        .with_selected_config(selected_config.map(|s| s.to_string()))
        .with_target_argv(target.to_string(), argv.to_vec());
    let registered = register_cookfile(builder, &lua_source, cache_ctx)
        .map_err(map_register_error)?;

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

/// Variant of [`register_workspace`] that binds argv to the targeted recipe /
/// chore in the root Cookfile (COOK-36 Task 4).
///
/// `target` and `argv` are passed only to the root Cookfile's builder.
/// Import Cookfiles are registered without argv (they are not the dispatch
/// target). This is correct because chores are always defined in the root
/// Cookfile and dispatch is always rooted there.
pub fn register_workspace_with_argv(
    workspace: &Workspace,
    config: Option<&str>,
    env_overrides: &[String],
    target: &str,
    argv: &[String],
    cache_ctx: Option<Arc<cook_cache::cache_ctx::CacheContext>>,
) -> Result<RegisteredWorkspace, PipelineError> {
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(config, dotenv_vars, env_overrides)?;
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let shared_outputs: SharedTerminalOutputs =
        Arc::new(std::sync::Mutex::new(BTreeMap::new()));
    let shared_member_outputs: SharedMemberOutputs =
        Arc::new(std::sync::Mutex::new(BTreeMap::new()));

    let mut ws = RegisteredWorkspace {
        names: Vec::new(),
        units_by_recipe: BTreeMap::new(),
        probes: BTreeMap::new(),
        final_env_by_cookfile: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix: BTreeMap::new(),
    };

    // Register Cookfiles importees-first / root-last (see
    // `cookfile_registration_order`). The dispatch target's argv binds only to
    // the root Cookfile — chores are always defined in and dispatched from root.
    let root_canon = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    for dir in cookfile_registration_order(workspace) {
        if dir == root_canon {
            // Root Cookfile: empty qualified prefix, with argv binding.
            let root_alias_dirs = workspace.alias_dirs_for(&workspace.root.dir);
            let root_alias_qp =
                workspace.alias_qualified_prefixes_for(&workspace.root.dir);
            let root_builder =
                RegisterSessionBuilder::new(workspace.root.dir.clone(), root_env.clone())
                    .with_cli_overrides(cli_overrides.clone())
                    .with_selected_config(config.map(|s| s.to_string()))
                    .with_shared_terminal_outputs(shared_outputs.clone())
                    .with_shared_member_outputs(shared_member_outputs.clone())
                    .with_qualified_prefix(String::new())
                    .with_alias_dirs(root_alias_dirs.clone())
                    .with_alias_qualified_prefixes(root_alias_qp.clone())
                    .with_target_argv(target.to_string(), argv.to_vec());
            let root_registered = register_cookfile(
                root_builder,
                &workspace.root.lua_source,
                cache_ctx.clone(),
            )
            .map_err(map_register_error)?;
            merge_into(&mut ws, "", &root_alias_qp, root_registered);
            ws.working_dir_by_prefix
                .insert(String::new(), workspace.root.dir.clone());
            ws.alias_dirs_by_prefix
                .insert(String::new(), root_alias_dirs);
        } else if let Some(loaded) = workspace.imports.get(&dir) {
            // Imports: canonical workspace qualified prefix; no argv.
            let prefix = find_full_prefix(workspace, &dir);
            let import_env = resolve_env(config, HashMap::new(), env_overrides)?;
            let alias_dirs = workspace.alias_dirs_for(&loaded.dir);
            let alias_qp = workspace.alias_qualified_prefixes_for(&loaded.dir);
            let builder = RegisterSessionBuilder::new(loaded.dir.clone(), import_env)
                .with_cli_overrides(cli_overrides.clone())
                .with_selected_config(config.map(|s| s.to_string()))
                .with_shared_terminal_outputs(shared_outputs.clone())
                .with_shared_member_outputs(shared_member_outputs.clone())
                .with_qualified_prefix(prefix.clone())
                .with_alias_dirs(alias_dirs.clone())
                .with_alias_qualified_prefixes(alias_qp.clone());
            let import_registered = register_cookfile(
                builder,
                &loaded.lua_source,
                cache_ctx.clone(),
            )
            .map_err(map_register_error)?;
            merge_into(&mut ws, &prefix, &alias_qp, import_registered);
            ws.working_dir_by_prefix
                .insert(prefix.clone(), loaded.dir.clone());
            ws.alias_dirs_by_prefix.insert(prefix.clone(), alias_dirs);
        }
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
    let names = cook_register::list_names(builder, &lua_source).map_err(map_register_error)?;
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
        .map_err(map_register_error)?;
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
        let names =
            cook_register::list_names(builder, &loaded.lua_source).map_err(map_register_error)?;
        for n in names {
            out.push((format!("{prefix}.{}", n.name), n.kind));
        }
    }

    Ok(out)
}

/// Re-run codegen against the *full* register-phase recipe set (§10.2 step 2).
///
/// The first codegen pass — run by [`super::parse::read_and_parse`] — classifies
/// `$<NAME>` placeholders using only the statically parsed `recipe` blocks
/// ([`cook_luagen::dep_ref::extract_recipe_names`]). A `$<NAME>` that names a
/// recipe registered at register-phase by a top-level module call (e.g.
/// `cook_cc.bin("x")`) is invisible to that pass, so it mis-lowers to
/// `cook.require_env("NAME")` and hard-errors when the recipe body runs.
///
/// This discovers the actual registered recipe names via
/// [`list_single_cookfile_names`] — the cheap, body-free register pass that
/// `cook list` / `cook menu` use (see [`cook_register::list_names`]) — unions
/// them with the static set, and regenerates the Lua so module-registered
/// recipes resolve to `cook.dep_output` per §10.2 step 2. Feeding the
/// *discovery* (static-name) Lua to `list_names` is safe: `list_names` never
/// invokes a recipe body, so the latent `require_env` mis-lowering is never
/// reached during discovery.
pub fn codegen_with_module_recipes_single(
    cookfile_dir: &Path,
    cookfile: &Cookfile,
    discovery_lua: String,
    env_vars: HashMap<String, String>,
    env_overrides: &[String],
    selected_config: Option<&str>,
) -> Result<String, PipelineError> {
    let discovered = list_single_cookfile_names(
        cookfile_dir,
        env_vars,
        env_overrides,
        discovery_lua,
        selected_config,
    )?;
    let mut names = cook_luagen::dep_ref::extract_recipe_names(cookfile);
    for (name, _kind) in discovered {
        names.insert(name);
    }
    cook_luagen::generate_with_names_checked(cookfile, &names)
        .map_err(|e| PipelineError::Codegen(e.to_string()))
}

/// Re-run codegen for every Cookfile in the workspace against the *full*
/// register-phase recipe set (§10.2 step 2, CS-0094), generalising
/// the former single-Cookfile-only pass to every workspace member.
///
/// The load-time codegen passes classify `$<NAME>` placeholders using only
/// statically parsed `recipe` blocks plus the §7.3 alias union. A `$<NAME>`
/// naming a recipe registered at register-phase by a top-level module call
/// (e.g. `cook_cc.bin("x")`) is invisible to those passes and mis-lowers to
/// `cook.require_env(...)`, hard-erroring when the body runs during the
/// register pass. This runs the cheap body-free [`cook_register::list_names`]
/// pass per member (same env policy as [`list_workspace_names`]), unions the
/// discovered names with the static set — locally, and as `alias.name` on
/// each importer — and regenerates every member's Lua in place. Feeding the
/// static-name Lua to `list_names` is safe: it never invokes a recipe body,
/// so the latent mis-lowering is never reached during discovery.
pub fn codegen_with_module_recipes(
    workspace: &mut Workspace,
    config: Option<&str>,
    env_overrides: &[String],
) -> Result<(), PipelineError> {
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let mut discovered: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();

    // Root: .env layering applies (mirror list_workspace_names).
    let root_canon = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(config, dotenv_vars, env_overrides)?;
    let root_builder = RegisterSessionBuilder::new(workspace.root.dir.clone(), root_env)
        .with_cli_overrides(cli_overrides.clone())
        .with_selected_config(config.map(|s| s.to_string()));
    let root_names = cook_register::list_names(root_builder, &workspace.root.lua_source)
        .map_err(map_register_error)?;
    discovered.insert(root_canon, root_names.into_iter().map(|n| n.name).collect());

    // Imports: fresh env baseline — no root .env layering (mirror
    // list_workspace_names / register_workspace policy).
    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        let import_env = resolve_env(config, HashMap::new(), env_overrides)?;
        let builder = RegisterSessionBuilder::new(loaded.dir.clone(), import_env)
            .with_cli_overrides(cli_overrides.clone())
            .with_selected_config(config.map(|s| s.to_string()))
            .with_qualified_prefix(prefix);
        let names = cook_register::list_names(builder, &loaded.lua_source)
            .map_err(map_register_error)?;
        discovered.insert(
            canonical_path.clone(),
            names.into_iter().map(|n| n.name).collect(),
        );
    }

    super::workspace::regenerate_lua_sources(workspace, &discovered)
}

/// Merge a per-Cookfile [`cook_register::RegisteredCookfile`] into the
/// workspace-level [`RegisteredWorkspace`], qualifying every recipe name,
/// unit key, probe key, and intra-Cookfile `requires` entry with `prefix`
/// (empty for the root).
///
/// Intra-Cookfile `requires` entries (e.g. `recipe wasm: generate` inside
/// `tree-sitter-cook/Cookfile` imported as `ts`) must be rewritten from the
/// local name `generate` to the qualified `ts.generate` so the analyzer's
/// dep-graph walk sees a consistent fully-qualified namespace. Without this
/// qualification `analyzer::build_adjacency` walks every recipe in the
/// workspace and errors `UnknownRecipe("generate")` even when the target
/// closure (e.g. `cook package`) does not transitively touch the import.
fn merge_into(
    ws: &mut RegisteredWorkspace,
    prefix: &str,
    alias_qualified_prefixes: &BTreeMap<String, String>,
    rc: cook_register::RegisteredCookfile,
) {
    let qualify = |name: &str| {
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        }
    };
    // Local recipe names registered by this Cookfile — used to distinguish
    // intra-Cookfile dep references (`requires=["generate"]` resolving inside
    // `tree-sitter-cook/Cookfile`) from already-qualified cross-Cookfile
    // references that callers may have produced explicitly. Intra-Cookfile
    // requires get the prefix; cross-Cookfile or already-qualified ones pass
    // through untouched.
    let local_names: std::collections::BTreeSet<String> =
        rc.names.iter().map(|n| n.name.clone()).collect();
    for n in rc.names {
        let mut qn = n.clone();
        qn.name = qualify(&n.name);
        qn.requires = n
            .requires
            .iter()
            .map(|req| {
                // Cross-Cookfile `alias.recipe` requires → the importee's
                // canonical global key (mirrors `resolve_global_key` and the
                // inferred-deps analyzer). Without this the analyzer sees the
                // local alias name (e.g. `proto.proto_lib`) and errors
                // `UnknownRecipe` when the canonical key is, say,
                // `server.queue.proto.proto_lib` (a diamond / transitive
                // importee whose prefix differs from the local alias).
                if let Some((alias, sub)) = req.split_once('.') {
                    if let Some(importee_prefix) = alias_qualified_prefixes.get(alias) {
                        return if importee_prefix.is_empty() {
                            sub.to_string()
                        } else {
                            format!("{importee_prefix}.{sub}")
                        };
                    }
                }
                // Intra-Cookfile local name → prefix it with this Cookfile's
                // qualified prefix. Anything else (already-global, or unknown —
                // rejected downstream) passes through untouched.
                if local_names.contains(req) {
                    qualify(req)
                } else {
                    req.clone()
                }
            })
            .collect();
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

/// Map a [`cook_register::RegisterError`] from one of the helpers in this
/// module onto a [`PipelineError`]. The collision variant is preserved as a
/// structured `PipelineError::RecipeCollision { name, sites }` so the CLI can
/// render the multi-line per-site diagnostic at emit time (SHI-222 Phase 5
/// Task 5.6, spec §8); all other variants fall through to
/// `PipelineError::Other` carrying `RegisterError`'s own `Display` impl —
/// matching the pre-Task-5.6 behavior for non-collision errors.
fn map_register_error(e: cook_register::RegisterError) -> PipelineError {
    match e {
        cook_register::RegisterError::RecipeCollision { name, sites } => {
            PipelineError::RecipeCollision { name, sites }
        }
        // COOK-36 Task 9: append a migration hint when a paramless chore
        // receives exactly one bare-ident-shaped positional — the user likely
        // meant to select a config preset with the old positional form.
        cook_register::RegisterError::ChoreTooManyArgv {
            ref chore,
            declared,
            supplied,
            ref first_unmatched,
        } if declared == 0
            && supplied == 1
            && !first_unmatched.is_empty()
            && first_unmatched
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') =>
        {
            let base = e.to_string();
            PipelineError::Other(format!(
                "{base}. Did you mean a config preset? \
                 Use 'cook {chore} @{first_unmatched}' or \
                 'cook {chore} --config {first_unmatched}'."
            ))
        }
        other => PipelineError::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHI-222 Phase 5 Task 5.6: `register_single_cookfile` must surface
    /// `RegisterError::RecipeCollision` as a structured
    /// `PipelineError::RecipeCollision { name, sites }` (not as
    /// `PipelineError::Other`), so the CLI can render the multi-line
    /// per-site diagnostic at emit time (spec §8) and exit with code 3.
    #[test]
    fn register_single_cookfile_maps_collision_to_typed_variant() {
        let lua_src = r#"
            cook.recipe("build", {requires = {}}, function() end)
            cook.recipe("build", {requires = {}}, function() end)
        "#;
        let tmpdir = tempfile::TempDir::new().unwrap();
        let result = register_single_cookfile(
            tmpdir.path(),
            HashMap::new(),
            &[],
            lua_src.to_string(),
            None,
            None,
        );

        match result {
            Ok(_) => panic!("expected PipelineError::RecipeCollision, got Ok"),
            Err(PipelineError::RecipeCollision { name, sites }) => {
                assert_eq!(name, "build");
                assert_eq!(sites.len(), 2, "both register-phase sites are captured");
                // Both are dynamic `cook.recipe(...)` calls — confirms the
                // typed mapping passes the kind through faithfully.
                for s in &sites {
                    assert_eq!(s.kind, cook_register::RegistrationSiteKind::Dynamic);
                }
            }
            Err(other) => panic!("expected PipelineError::RecipeCollision, got {other:?}"),
        }
    }

    /// `RegisterError` variants other than `RecipeCollision` continue to fall
    /// through to `PipelineError::Other` (pre-Task-5.6 behavior preserved).
    /// Exercises the fallthrough arm of `map_register_error` via a Lua-level
    /// error in the cookfile source.
    #[test]
    fn register_single_cookfile_maps_non_collision_to_other() {
        // Top-level Lua error (undefined function) → RegisterError::Lua →
        // PipelineError::Other.
        let lua_src = "this_function_does_not_exist()\n";
        let tmpdir = tempfile::TempDir::new().unwrap();
        let result = register_single_cookfile(
            tmpdir.path(),
            HashMap::new(),
            &[],
            lua_src.to_string(),
            None,
            None,
        );

        match result {
            Ok(_) => panic!("expected PipelineError::Other, got Ok"),
            Err(PipelineError::Other(_)) => {}
            Err(other) => panic!("expected PipelineError::Other, got {other:?}"),
        }
    }

    /// Workspace-of-one discovery — a recipe registered at
    /// register-phase (invisible to static codegen) must be folded into the
    /// $<NAME> classification set when the workspace path re-codegens.
    #[test]
    fn codegen_with_module_recipes_discovers_dynamic_recipe_workspace_of_one() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Cookfile"),
            "recipe consume\n    cook \"build/out\" { cat $<gen> > $<out> }\n",
        )
        .unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let mut ws = Workspace::load(&entry, &root, &[]).unwrap();
        // Static codegen cannot see `gen` → mis-lowers to require_env.
        assert!(ws.root.lua_source.contains("cook.require_env(\"gen\")"));
        // Simulate a module-registered recipe: append a dynamic registration
        // to the discovery Lua (list_names sees it; bodies never run).
        ws.root.lua_source.push_str(
            "\ncook.recipe(\"gen\", {requires = {}}, function() end)\n",
        );
        codegen_with_module_recipes(&mut ws, None, &[]).unwrap();
        assert!(
            ws.root.lua_source.contains("cook.dep_output(\"gen\")"),
            "expected $<gen> re-lowered to dep_output, got:\n{}",
            ws.root.lua_source
        );
    }

    /// The discovery pass must also cover IMPORTED members —
    /// an importer's `$<alias.recipe>` where `recipe` is module-registered in
    /// the importee must re-lower to dep_output on the workspace path.
    #[test]
    fn codegen_with_module_recipes_discovers_dynamic_recipe_in_importee() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("lib")).unwrap();
        std::fs::write(
            dir.path().join("lib/Cookfile"),
            "recipe lib_static\n    cook \"lib.o\" { echo $<out> }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Cookfile"),
            "import lib ./lib\nrecipe top\n    cook \"build/top\" { cat $<lib.gen> > $<out> }\n",
        )
        .unwrap();
        std::fs::write(dir.path().join(".cookroot"), "").unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let mut ws = Workspace::load(&entry, &root, &[]).unwrap();
        assert!(ws.root.lua_source.contains("cook.require_env(\"lib.gen\")"));
        // Simulate a module-registered recipe in the importee.
        let lib_canon = std::fs::canonicalize(dir.path().join("lib")).unwrap();
        ws.imports
            .get_mut(&lib_canon)
            .unwrap()
            .lua_source
            .push_str("\ncook.recipe(\"gen\", {requires = {}}, function() end)\n");
        codegen_with_module_recipes(&mut ws, None, &[]).unwrap();
        assert!(
            ws.root.lua_source.contains("cook.dep_output(\"lib.gen\")"),
            "expected $<lib.gen> re-lowered to dep_output, got:\n{}",
            ws.root.lua_source
        );
    }
}
