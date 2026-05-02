//! Pipeline: glue layer that wires cook-lang -> cook-luagen -> cook-register
//! -> cook-engine together.
//!
//! This is the main orchestration module. It parses Cookfiles, builds recipe
//! metadata and registries, then delegates execution to `cook_engine::run::run()`
//! which handles wave-parallel DAG execution for both single-Cookfile and
//! workspace builds.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::mpsc;

use crate::cli::Cli;
use crate::env::{load_env, resolve_env};
use crate::error::CookError;
use crate::progress::spawn_new_renderer;

// Test output types are used by cmd_test once cook-engine supports test
// result collection. Keep the import for future wiring.
#[allow(unused_imports)]
use crate::test_output::{self, TestCaseResult, TestResults, TestStatus};
use crate::watcher::CookWatcher;
use crate::workspace::Workspace;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn read_and_parse(cli: &Cli) -> Result<(cook_lang::ast::Cookfile, String), CookError> {
    let source = std::fs::read_to_string(&cli.file)
        .map_err(|e| CookError::Other(format!("cannot read {}: {e}", cli.file.display())))?;

    let cookfile =
        cook_lang::parse(&source).map_err(|e| CookError::ParseError(e.to_string()))?;

    // Pre-scan: extract recipe names for codegen disambiguation
    let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);

    // § 5.4 — accessor placement validation rejects `{lib.ACCESSOR}` in
    // contexts that lack a matching driver in an output pattern.
    let lua_source = cook_luagen::generate_with_names_checked(&cookfile, &recipe_names)
        .map_err(|e| CookError::Other(e.to_string()))?;

    // § 5.5 — register-time warnings for references whose referent has an
    // empty output list.
    let (_, warnings) =
        cook_luagen::generate_with_names_and_warnings(&cookfile, &recipe_names);
    for w in warnings {
        eprintln!("cook: warning: {}", w);
    }

    Ok((cookfile, lua_source))
}

/// Validate that `config` (if supplied) matches a named `config NAME ... end`
/// block in the Cookfile. Errors with the list of available names on mismatch.
pub fn validate_selected_config(
    cookfile: &cook_lang::ast::Cookfile,
    config: Option<&str>,
) -> Result<(), CookError> {
    let Some(name) = config else { return Ok(()); };
    let has_match = cookfile
        .config_blocks
        .iter()
        .any(|b| b.name.as_deref() == Some(name));
    if has_match {
        return Ok(());
    }
    let available: Vec<&str> = cookfile
        .config_blocks
        .iter()
        .filter_map(|b| b.name.as_deref())
        .collect();
    if available.is_empty() {
        Err(CookError::Other(format!(
            "unknown config '{}': no named configs defined",
            name
        )))
    } else {
        Err(CookError::Other(format!(
            "unknown config '{}'. available: {}",
            name,
            available.join(", ")
        )))
    }
}

/// Bridge cook-engine events to the new cook-progress ProgressEvent stream.
/// Interns recipe names and node names into stable `RecipeId` / `NodeId`.
fn bridge_engine_to_progress_events(
    engine_rx: mpsc::Receiver<cook_engine::EngineEvent>,
    progress_tx: mpsc::Sender<cook_progress::ProgressEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use cook_progress::{NodeId, RecipeId, RecipeTopo, SkipReason, Stream};
        use std::collections::BTreeMap;

        let mut recipe_ids: BTreeMap<String, RecipeId> = BTreeMap::new();
        let mut node_ids: BTreeMap<(String, String), NodeId> = BTreeMap::new();
        let mut next_recipe: u32 = 0;
        let mut next_node: u32 = 0;

        fn intern_recipe(
            name: &str,
            recipe_ids: &mut BTreeMap<String, RecipeId>,
            next_recipe: &mut u32,
        ) -> RecipeId {
            if let Some(id) = recipe_ids.get(name) {
                return *id;
            }
            let id = RecipeId::new(*next_recipe);
            *next_recipe += 1;
            recipe_ids.insert(name.to_string(), id);
            id
        }

        fn intern_node(
            recipe: &str,
            node: &str,
            node_ids: &mut BTreeMap<(String, String), NodeId>,
            next_node: &mut u32,
        ) -> NodeId {
            let key = (recipe.to_string(), node.to_string());
            if let Some(id) = node_ids.get(&key) {
                return *id;
            }
            let id = NodeId::new(*next_node);
            *next_node += 1;
            node_ids.insert(key, id);
            id
        }

        while let Ok(event) = engine_rx.recv() {
            let pe = match event {
                cook_engine::EngineEvent::BuildStarted { recipes, total_nodes } => {
                    let topos: Vec<RecipeTopo> = recipes
                        .into_iter()
                        .map(|r| {
                            let id = intern_recipe(&r.name, &mut recipe_ids, &mut next_recipe);
                            let deps: Vec<RecipeId> = r
                                .deps
                                .iter()
                                .map(|d| intern_recipe(d, &mut recipe_ids, &mut next_recipe))
                                .collect();
                            RecipeTopo {
                                id,
                                name: r.name,
                                deps,
                                expected_nodes: r.expected_nodes,
                            }
                        })
                        .collect();
                    cook_progress::ProgressEvent::BuildStarted {
                        recipes: topos,
                        total_nodes,
                    }
                }
                cook_engine::EngineEvent::RecipeQueued { .. } => continue,
                cook_engine::EngineEvent::RecipeStarted { name, .. } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeStarted { recipe: id }
                }
                cook_engine::EngineEvent::RecipeCompleted {
                    name,
                    elapsed,
                    cached_nodes,
                    total_nodes,
                } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeCompleted {
                        recipe: id,
                        elapsed,
                        cached: cached_nodes,
                        total: total_nodes,
                    }
                }
                cook_engine::EngineEvent::RecipeFailed {
                    name,
                    elapsed,
                    completed_nodes,
                    total_nodes,
                } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeFailed {
                        recipe: id,
                        elapsed,
                        completed: completed_nodes,
                        total: total_nodes,
                    }
                }
                cook_engine::EngineEvent::NodeStarted {
                    recipe,
                    node_name,
                    artifact,
                    fallback_label,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeStarted {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        artifact,
                        fallback_label,
                    }
                }
                cook_engine::EngineEvent::NodeCompleted {
                    recipe,
                    node_name,
                    elapsed,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeCompleted {
                        recipe: rid,
                        node: nid,
                        elapsed,
                    }
                }
                cook_engine::EngineEvent::NodeFailed {
                    recipe,
                    node_name,
                    elapsed,
                    error,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeFailed {
                        recipe: rid,
                        node: nid,
                        elapsed,
                        error,
                    }
                }
                cook_engine::EngineEvent::NodeCacheHit {
                    recipe,
                    node_name,
                    artifact,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeCacheHit {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        artifact,
                    }
                }
                cook_engine::EngineEvent::NodeSkipped { recipe, node_name } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeSkipped {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        reason: SkipReason::UpstreamFailed,
                    }
                }
                cook_engine::EngineEvent::OutputLine {
                    recipe,
                    line,
                    is_stderr,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    // OutputLine does not yet carry a node id at the engine level. Attribute to the
                    // most recently interned node in the same recipe; if none, use sentinel MAX.
                    let nid = node_ids
                        .iter()
                        .rev()
                        .find(|((r, _), _)| r == &recipe)
                        .map(|(_, id)| *id)
                        .unwrap_or_else(|| NodeId::new(u32::MAX));
                    let stream = if is_stderr { Stream::Stderr } else { Stream::Stdout };
                    cook_progress::ProgressEvent::NodeOutput {
                        recipe: rid,
                        node: nid,
                        line,
                        stream,
                    }
                }
                cook_engine::EngineEvent::InteractiveStart { recipe, node_name } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::InteractiveStart {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                    }
                }
                cook_engine::EngineEvent::InteractiveEnd {
                    recipe,
                    node_name,
                    elapsed,
                    success,
                    is_terminal,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::InteractiveEnd {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        elapsed,
                        success,
                        is_terminal,
                    }
                }
                cook_engine::EngineEvent::Finished { success, .. } => {
                    cook_progress::ProgressEvent::Finished { success }
                }
            };
            let is_finished = matches!(pe, cook_progress::ProgressEvent::Finished { .. });
            let _ = progress_tx.send(pe);
            if is_finished {
                break;
            }
        }
    })
}

/// Map cook-engine errors to CookError.
fn engine_error_to_cook_error(e: cook_engine::EngineError) -> CookError {
    match e {
        cook_engine::EngineError::TaskFailures { failures, .. } => {
            if let Some((_, _recipe_name, msg)) = failures.first() {
                if msg.contains("COOK_CMD_FAILED:") {
                    let parts: Vec<&str> = msg
                        .split("COOK_CMD_FAILED:")
                        .nth(1)
                        .unwrap_or("0:1:unknown")
                        .splitn(3, ':')
                        .collect();
                    let line = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0usize);
                    let code = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1i32);
                    let command = parts.get(2).unwrap_or(&"unknown").to_string();
                    if line == 0 {
                        CookError::CommandFailed(format!(
                            "command failed (exit {code}): {command}"
                        ))
                    } else {
                        CookError::CommandFailed(format!(
                            "Cookfile:{line}: command failed (exit {code}): {command}"
                        ))
                    }
                } else {
                    CookError::Other(msg.clone())
                }
            } else {
                CookError::Other("unknown engine error".into())
            }
        }
        cook_engine::EngineError::CycleDetected(name) => {
            CookError::Other(format!("dependency cycle involving: {name}"))
        }
        cook_engine::EngineError::UnknownRecipe(name) => CookError::RecipeNotFound(name),
        cook_engine::EngineError::RegistrationFailed { recipe, message } => {
            CookError::Other(format!("registration failed for '{recipe}': {message}"))
        }
        cook_engine::EngineError::CacheError(msg) => CookError::Other(msg),
    }
}

// ---------------------------------------------------------------------------
// Build recipe_infos + registries (shared by cmd_run and cmd_test)
// ---------------------------------------------------------------------------

/// Build recipe_infos from a single Cookfile's recipes and chores.
///
/// Chores are registered as recipes with no ingredients and no cook outputs
/// (they never produce cached artifacts). The engine sees them as ordinary
/// recipes; the chore contract (interactive-only, cache=false) is enforced
/// at codegen time by `compile_chore` (cook-luagen).
fn build_single_recipe_infos(
    cookfile: &cook_lang::ast::Cookfile,
) -> BTreeMap<String, cook_engine::analyzer::RecipeInfo> {
    let mut recipe_infos = BTreeMap::new();
    for recipe in &cookfile.recipes {
        recipe_infos.insert(
            recipe.name.clone(),
            cook_engine::analyzer::RecipeInfo {
                ingredients: recipe.ingredients.clone(),
                serves: recipe
                    .steps
                    .iter()
                    .flat_map(|s| {
                        if let cook_lang::ast::Step::Cook { step, .. } = s {
                            step.outputs.clone()
                        } else {
                            Vec::new()
                        }
                    })
                    .collect(),
                requires: recipe.deps.clone(),
            },
        );
    }
    // Chores have no ingredients or cook outputs; their deps are explicit only.
    for chore in &cookfile.chores {
        recipe_infos.insert(
            chore.name.clone(),
            cook_engine::analyzer::RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: chore.deps.clone(),
            },
        );
    }
    recipe_infos
}

/// Build a single-Cookfile registry map (empty-string prefix).
fn build_single_registries(
    cookfile_dir: &Path,
    env_vars: std::collections::HashMap<String, String>,
    lua_source: String,
    selected_config: Option<&str>,
) -> BTreeMap<String, cook_engine::RegistryEntry> {
    let registry = cook_register::Registry::new(cookfile_dir.to_path_buf(), env_vars)
        .with_selected_config(selected_config.map(|s| s.to_string()));
    let mut registries = BTreeMap::new();
    registries.insert(
        String::new(),
        cook_engine::RegistryEntry { registry, lua_source, alias_dirs: BTreeMap::new() },
    );
    registries
}

/// Build workspace registries: one for root (empty prefix), one per import.
fn build_workspace_registries(
    workspace: &Workspace,
    config: Option<&str>,
    cli_sets: &[String],
) -> Result<BTreeMap<String, cook_engine::RegistryEntry>, CookError> {
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(config, dotenv_vars, cli_sets)?;

    // One shared terminal-outputs map for the entire workspace invocation.
    // All Registries write to and read from the same map, keyed by
    // fully-qualified recipe name (e.g. "lib.lib_build" or "build").
    let shared_outputs: cook_register::SharedTerminalOutputs =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));

    let mut registries: BTreeMap<String, cook_engine::RegistryEntry> = BTreeMap::new();

    let root_alias_dirs = workspace.alias_dirs_for(&workspace.root.dir);
    // Root has empty prefix (already the default; explicit for clarity).
    let root_registry = cook_register::Registry::new(workspace.root.dir.clone(), root_env)
        .with_selected_config(config.map(|s| s.to_string()))
        .with_shared_terminal_outputs(shared_outputs.clone())
        .with_qualified_prefix(String::new())
        .with_alias_dirs(root_alias_dirs.clone());
    registries.insert(
        String::new(),
        cook_engine::RegistryEntry {
            registry: root_registry,
            lua_source: workspace.root.lua_source.clone(),
            alias_dirs: root_alias_dirs,
        },
    );

    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        let import_env = resolve_env(
            config,
            std::collections::HashMap::new(),
            cli_sets,
        )?;
        let alias_dirs = workspace.alias_dirs_for(&loaded.dir);
        let registry = cook_register::Registry::new(loaded.dir.clone(), import_env)
            .with_selected_config(config.map(|s| s.to_string()))
            .with_shared_terminal_outputs(shared_outputs.clone())
            .with_qualified_prefix(prefix.clone())
            .with_alias_dirs(alias_dirs.clone());
        registries.insert(
            prefix,
            cook_engine::RegistryEntry {
                registry,
                lua_source: loaded.lua_source.clone(),
                alias_dirs,
            },
        );
    }

    Ok(registries)
}

/// Run the engine with progress rendering wired up.
fn run_with_progress(
    cli: &Cli,
    recipe_infos: &BTreeMap<String, cook_engine::analyzer::RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, cook_engine::RegistryEntry>,
    num_jobs: usize,
    inferred_deps: &BTreeMap<String, Vec<String>>,
) -> Result<cook_engine::run::RunResult, CookError> {
    let project_root = std::env::current_dir()
        .map_err(|e| CookError::Other(e.to_string()))?;
    let (progress_tx, progress_rx) = mpsc::channel::<cook_progress::ProgressEvent>();
    let render_thread = spawn_new_renderer(cli, project_root.clone(), progress_rx);

    let bridge_tx = progress_tx.clone();
    let (engine_tx, engine_rx) = mpsc::channel::<cook_engine::EngineEvent>();
    let bridge_thread = bridge_engine_to_progress_events(engine_rx, bridge_tx);

    let result = cook_engine::run::run(
        &project_root,
        recipe_infos,
        targets,
        registries,
        num_jobs,
        inferred_deps,
        move |event| {
            let _ = engine_tx.send(event);
        },
    );

    // Wait for bridge to drain and forward Finished, then renderer to exit.
    let _ = bridge_thread.join();
    // Drop progress_tx before joining the render thread so the channel is
    // closed even if the engine exited abnormally without emitting Finished.
    // Without this, the renderer's rx.recv() would block forever when the
    // engine panics and never sends Finished.
    drop(progress_tx);
    let _success = render_thread.join().unwrap_or(false);

    result.map_err(engine_error_to_cook_error)
}

/// Resolve num_jobs from CLI or system parallelism.
fn resolve_num_jobs(cli: &Cli) -> usize {
    cli.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    })
}

// ---------------------------------------------------------------------------
// cmd_run
// ---------------------------------------------------------------------------

pub fn cmd_run(cli: &Cli, recipe_name: &str, config: Option<&str>) -> Result<(), CookError> {
    let (cookfile, lua_source) = read_and_parse(cli)?;
    validate_selected_config(&cookfile, config)?;

    if cli.emit_lua {
        println!("{lua_source}");
        return Ok(());
    }

    let num_jobs = resolve_num_jobs(cli);
    let targets = vec![recipe_name.to_string()];

    if !cookfile.imports.is_empty() {
        let workspace_root = crate::workspace::resolve_workspace_root(
            &cli.file,
            cli.root.clone(),
        )?;
        let workspace = Workspace::load(&cli.file, &workspace_root, &cli.set)?;
        let recipe_infos = build_workspace_recipe_info(&workspace)?;
        let registries = build_workspace_registries(&workspace, config, &cli.set)?;

        let inferred_deps = compute_workspace_inferred_deps(&workspace);

        // Emit warning for conflicting dep types (mirrors single-Cookfile path).
        // Iterate root recipes — qualify names with empty prefix.
        for recipe in &workspace.root.cookfile.recipes {
            for (qualified_consumer, dep_list) in &inferred_deps {
                if qualified_consumer == &recipe.name {
                    for inferred_dep in dep_list {
                        if recipe.deps.contains(inferred_dep) {
                            eprintln!(
                                "cook: warning: recipe '{}' has both explicit ': {}' and inferred '{{{}}}' dependency — conflicting scheduling intent",
                                recipe.name, inferred_dep, inferred_dep
                            );
                        }
                    }
                }
            }
        }
        // Iterate imported recipes — qualify names with their prefix.
        for (canonical_path, loaded) in &workspace.imports {
            let prefix = find_full_prefix(&workspace, canonical_path);
            for recipe in &loaded.cookfile.recipes {
                let qualified_consumer = format!("{prefix}.{}", recipe.name);
                if let Some(dep_list) = inferred_deps.get(&qualified_consumer) {
                    for inferred_dep in dep_list {
                        // recipe.deps may be written as unqualified ("x") or qualified
                        // ("alias.x"). Normalize by checking both the raw dep string and
                        // the qualified form against each recipe.deps entry.
                        if recipe.deps.contains(inferred_dep) {
                            eprintln!(
                                "cook: warning: recipe '{}' has both explicit ': {}' and inferred '{{{}}}' dependency — conflicting scheduling intent",
                                qualified_consumer, inferred_dep, inferred_dep
                            );
                        }
                    }
                }
            }
        }

        run_with_progress(cli, &recipe_infos, &targets, &registries, num_jobs, &inferred_deps)?;
    } else {
        // Single Cookfile build
        let cookfile_dir = cli.file.parent().unwrap_or(Path::new("."));
        let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            cookfile_dir
        };
        let dotenv_vars = load_env(cookfile_dir);
        let env_vars = resolve_env(config, dotenv_vars, &cli.set)?;

        let recipe_infos = build_single_recipe_infos(&cookfile);

        // Extract inferred deps from {dep} references in recipe steps
        let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);
        let mut inferred_deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for recipe in &cookfile.recipes {
            let refs = cook_luagen::dep_ref::extract_dep_refs(recipe, &recipe_names);
            let dep_names: Vec<String> = refs
                .iter()
                .map(|r| r.recipe_name.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            if !dep_names.is_empty() {
                inferred_deps.insert(recipe.name.clone(), dep_names);
            }
        }

        // Emit warning for conflicting dep types
        for recipe in &cookfile.recipes {
            let refs = cook_luagen::dep_ref::extract_dep_refs(recipe, &recipe_names);
            for dep_ref in &refs {
                if recipe.deps.contains(&dep_ref.recipe_name) {
                    eprintln!(
                        "cook: warning: recipe '{}' has both explicit ': {}' and inferred '{{{}}}' dependency — conflicting scheduling intent",
                        recipe.name, dep_ref.recipe_name, dep_ref.recipe_name
                    );
                }
            }
        }

        let registries = build_single_registries(cookfile_dir, env_vars, lua_source, config);

        run_with_progress(cli, &recipe_infos, &targets, &registries, num_jobs, &inferred_deps)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_test
// ---------------------------------------------------------------------------

pub fn cmd_test(
    cli: &Cli,
    filter: Option<String>,
    verbose: bool,
    timeout_multiplier: u64,
    wrapper: Option<String>,
    list: bool,
) -> Result<(), CookError> {
    // Warn about unimplemented flags
    if filter.is_some() {
        eprintln!("cook: warning: --filter is not yet implemented, running all tests");
    }
    if verbose {
        eprintln!("cook: warning: --verbose is not yet implemented");
    }
    if timeout_multiplier != 1 {
        eprintln!("cook: warning: --timeout-multiplier is not yet implemented");
    }
    if wrapper.is_some() {
        eprintln!("cook: warning: --wrapper is not yet implemented");
    }
    if list {
        eprintln!("cook: warning: --list is not yet implemented");
    }

    let (cookfile, lua_source) = read_and_parse(cli)?;

    let num_jobs = resolve_num_jobs(cli);

    if !cookfile.imports.is_empty() {
        // Workspace test
        let workspace_root = crate::workspace::resolve_workspace_root(
            &cli.file,
            cli.root.clone(),
        )?;
        let workspace = Workspace::load(&cli.file, &workspace_root, &cli.set)?;

        // Discover test recipes across ALL Cookfiles (root + imports)
        let mut test_recipe_names: Vec<String> = Vec::new();

        // Root test recipes
        for recipe in &workspace.root.cookfile.recipes {
            if recipe
                .steps
                .iter()
                .any(|s| matches!(s, cook_lang::ast::Step::Test { .. }))
            {
                test_recipe_names.push(recipe.name.clone());
            }
        }

        // Imported test recipes (namespaced)
        for (canonical_path, loaded) in &workspace.imports {
            let prefix = find_full_prefix(&workspace, canonical_path);
            for recipe in &loaded.cookfile.recipes {
                if recipe
                    .steps
                    .iter()
                    .any(|s| matches!(s, cook_lang::ast::Step::Test { .. }))
                {
                    test_recipe_names.push(format!("{prefix}.{}", recipe.name));
                }
            }
        }

        if test_recipe_names.is_empty() {
            eprintln!("cook: no test recipes found");
            return Ok(());
        }

        let recipe_infos = build_workspace_recipe_info(&workspace)?;
        let registries = build_workspace_registries(&workspace, None, &cli.set)?;

        let _result =
            run_with_progress(cli, &recipe_infos, &test_recipe_names, &registries, num_jobs, &BTreeMap::new());
    } else {
        // Single Cookfile test
        let cookfile_dir = cli.file.parent().unwrap_or(Path::new("."));
        let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            cookfile_dir
        };
        let dotenv_vars = load_env(cookfile_dir);
        let env_vars = resolve_env(None, dotenv_vars, &cli.set)?;

        // Find all recipes that contain test steps
        let test_recipes: Vec<String> = cookfile
            .recipes
            .iter()
            .filter(|r| {
                r.steps
                    .iter()
                    .any(|s| matches!(s, cook_lang::ast::Step::Test { .. }))
            })
            .map(|r| r.name.clone())
            .collect();

        if test_recipes.is_empty() {
            eprintln!("cook: no test recipes found");
            return Ok(());
        }

        let recipe_infos = build_single_recipe_infos(&cookfile);
        let registries = build_single_registries(cookfile_dir, env_vars, lua_source, None);

        let _result =
            run_with_progress(cli, &recipe_infos, &test_recipes, &registries, num_jobs, &BTreeMap::new());
    }

    // TODO: Once cook-engine supports test output collection, convert
    // TestOutput -> TestCaseResult here and display results.
    // For now, tests execute but detailed results are not yet collected.
    eprintln!("cook: test execution complete (detailed results pending cook-engine integration)");

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_menu
// ---------------------------------------------------------------------------

pub fn cmd_menu(cli: &Cli) -> Result<(), CookError> {
    let (cookfile, _) = read_and_parse(cli)?;

    for recipe in &cookfile.recipes {
        let mut desc = format!("  recipe {}", recipe.name);
        if !recipe.ingredients.is_empty() {
            desc.push_str(&format!("  ingredients: {:?}", recipe.ingredients));
        }
        if !recipe.deps.is_empty() {
            desc.push_str(&format!("  deps: {:?}", recipe.deps));
        }
        for step in &recipe.steps {
            if let cook_lang::ast::Step::Cook {
                step: cook_step, ..
            } = step
            {
                desc.push_str(&format!("  cook: {}", cook_step.outputs.join(" ")));
            }
        }
        println!("{desc}");
    }

    for chore in &cookfile.chores {
        let mut desc = format!("  chore  {}", chore.name);
        if !chore.deps.is_empty() {
            desc.push_str(&format!("  deps: {:?}", chore.deps));
        }
        println!("{desc}");
    }

    if !cookfile.imports.is_empty() {
        let workspace_root = crate::workspace::resolve_workspace_root(
            &cli.file,
            cli.root.clone(),
        )?;
        let workspace = Workspace::load(&cli.file, &workspace_root, &cli.set)?;
        for (canonical_path, loaded) in &workspace.imports {
            let prefix = find_full_prefix(&workspace, canonical_path);
            for recipe in &loaded.cookfile.recipes {
                println!("  recipe {}.{}", prefix, recipe.name);
            }
            for chore in &loaded.cookfile.chores {
                println!("  chore  {}.{}", prefix, chore.name);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_init
// ---------------------------------------------------------------------------

pub fn cmd_init() -> Result<(), CookError> {
    let path = std::path::Path::new("Cookfile");
    if path.exists() {
        return Err(CookError::Other("Cookfile already exists".to_string()));
    }
    std::fs::write(
        path,
        r#"recipe "build"
    echo "Hello from Cook!"
end
"#,
    )
    .map_err(|e| CookError::Other(format!("failed to write Cookfile: {e}")))?;
    println!("Created Cookfile");
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_serve
// ---------------------------------------------------------------------------

pub fn cmd_serve(cli: &Cli, recipe_name: &str, config: Option<&str>) -> Result<(), CookError> {
    let (cookfile, _lua_source) = read_and_parse(cli)?;
    validate_selected_config(&cookfile, config)?;

    // Check for interactive steps -- not supported under cook --serve
    for recipe in &cookfile.recipes {
        for step in &recipe.steps {
            if let cook_lang::ast::Step::Shell {
                interactive: true,
                line,
                ..
            } = step
            {
                return Err(CookError::Other(format!(
                    "line {}: interactive '@' steps are not supported under 'cook --serve'",
                    line
                )));
            }
        }
    }

    // Resolve execution order via engine analyzer for glob collection
    let recipe_infos = build_single_recipe_infos(&cookfile);
    let order = cook_engine::analyzer::topological_sort(&recipe_infos, recipe_name)
        .map_err(|e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                CookError::RecipeNotFound(name)
            }
        })?;

    let globs = CookWatcher::collect_globs_for_recipes(&cookfile, &order);
    if globs.is_empty() {
        return Err(CookError::Other(
            "nothing to watch: no recipes in the chain have ingredients".to_string(),
        ));
    }

    let cookfile_path = std::fs::canonicalize(&cli.file)
        .map_err(|e| CookError::Other(format!("cannot resolve Cookfile path: {e}")))?;

    let mut cookfile_paths = vec![cookfile_path];

    // If imports exist, collect all imported Cookfile paths for watching
    if !cookfile.imports.is_empty() {
        let workspace_root = crate::workspace::resolve_workspace_root(
            &cli.file,
            cli.root.clone(),
        )?;
        let workspace = Workspace::load(&cli.file, &workspace_root, &cli.set)?;
        for (_canonical_path, loaded) in &workspace.imports {
            let import_cookfile = loaded.dir.join("Cookfile");
            if let Ok(canonical) = std::fs::canonicalize(&import_cookfile) {
                cookfile_paths.push(canonical);
            }
        }
    }

    let watcher = CookWatcher::new(globs, cookfile_paths);

    eprintln!("cook --serve: initial build...");
    let _ = cmd_run(cli, recipe_name, config);

    eprintln!("cook --serve: watching for changes...");
    watcher
        .watch(|cookfile_changed| {
            if cookfile_changed {
                eprintln!("cook --serve: Cookfile changed, rebuilding...");
            }
            cmd_run(cli, recipe_name, config).map_err(|e| e.to_string())?;
            Ok(())
        })
        .map_err(|e| CookError::Other(e.to_string()))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_dag
// ---------------------------------------------------------------------------

pub fn cmd_dag(cli: &Cli, recipe_name: &str, config: Option<&str>) -> Result<(), CookError> {
    let (cookfile, lua_source) = read_and_parse(cli)?;
    validate_selected_config(&cookfile, config)?;

    let cookfile_dir = cli.file.parent().unwrap_or(Path::new("."));
    let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        cookfile_dir
    };
    let dotenv_vars = load_env(cookfile_dir);
    let env_vars = resolve_env(config, dotenv_vars, &cli.set)?;

    let recipe_infos = build_single_recipe_infos(&cookfile);
    let targets = vec![recipe_name.to_string()];

    let mut edges = cook_engine::analyzer::dependency_edges_multi(&recipe_infos, &targets)
        .map_err(|e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(s) => {
                CookError::Other(format!("dependency cycle involving: {s}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(s) => CookError::RecipeNotFound(s),
        })?;

    // Save explicit edges before merging inferred deps (needed for wave grouping).
    let explicit_edges = edges.clone();

    // Extract inferred deps from {dep} references in recipe steps.
    let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);
    let mut inferred_deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for recipe in &cookfile.recipes {
        let refs = cook_luagen::dep_ref::extract_dep_refs(recipe, &recipe_names);
        let dep_names: Vec<String> = refs
            .iter()
            .map(|r| r.recipe_name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if !dep_names.is_empty() {
            inferred_deps.insert(recipe.name.clone(), dep_names);
        }
    }

    // Merge inferred deps into the edge map so the RecipeDag registers
    // recipes in the correct order.
    for (recipe_name, deps) in &inferred_deps {
        for dep_name in deps {
            edges.entry(dep_name.clone()).or_insert_with(Vec::new);
            let entry = edges.entry(recipe_name.clone()).or_insert_with(Vec::new);
            if !entry.contains(dep_name) {
                entry.push(dep_name.clone());
            }
        }
    }
    // Re-sort deps for deterministic output
    for deps in edges.values_mut() {
        deps.sort();
    }

    let mut recipe_dag = cook_engine::recipe_dag::RecipeDag::new(&edges);
    let mut all_units: Vec<(String, cook_contracts::RecipeUnits)> = Vec::new();
    let mut cache_managers: std::collections::BTreeMap<
        String,
        std::sync::Arc<cook_cache::ThreadSafeCacheManager>,
    > = std::collections::BTreeMap::new();

    let registry = cook_register::Registry::new(cookfile_dir.to_path_buf(), env_vars)
        .with_selected_config(config.map(|s| s.to_string()));

    loop {
        let ready = recipe_dag.pop_ready();
        if ready.is_empty() {
            break;
        }

        for name in &ready {
            let units = registry.register_recipe(&lua_source, name, None).map_err(|e| {
                CookError::Other(format!("registration failed for '{name}': {e}"))
            })?;

            let cache_dir = cookfile_dir.join(".cook").join("cache");
            cache_managers
                .entry(name.clone())
                .or_insert_with(|| {
                    std::sync::Arc::new(cook_cache::ThreadSafeCacheManager::new(cache_dir))
                });

            all_units.push((name.clone(), units));
        }

        recipe_dag.mark_done(&ready);
    }

    let dag_data = crate::dag_data::build_wave_dag_data(
        recipe_name,
        &all_units,
        &explicit_edges,
        &inferred_deps,
        &cache_managers,
    );

    let json = serde_json::to_string(&dag_data)
        .map_err(|e| CookError::Other(format!("failed to serialize DAG: {e}")))?;

    crate::dag_server::serve_dag(&json)
}

// ---------------------------------------------------------------------------
// Workspace helpers (kept — used by cmd_run, cmd_test, cmd_menu, cmd_serve)
// ---------------------------------------------------------------------------

/// Build a WorkspaceLayout from a Workspace for cook-engine's analyzer.
/// This is the anti-corruption layer: cook-cli owns Workspace (discovery/loading),
/// cook-engine owns namespace resolution and dependency analysis.
fn workspace_to_layout(
    workspace: &Workspace,
) -> cook_engine::analyzer::WorkspaceLayout {
    let root_dir = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());

    // Chores are first-class peers of recipes from the engine's POV: they
    // carry a name and a deps list; cross-form deps work transparently.
    // Merge both into the layout's name→deps tables.
    let root_recipes: Vec<(String, Vec<String>)> = workspace
        .root
        .cookfile
        .recipes
        .iter()
        .map(|r| (r.name.clone(), r.deps.clone()))
        .chain(
            workspace
                .root
                .cookfile
                .chores
                .iter()
                .map(|c| (c.name.clone(), c.deps.clone())),
        )
        .collect();

    let imported_recipes: Vec<(std::path::PathBuf, Vec<(String, Vec<String>)>)> = workspace
        .imports
        .iter()
        .map(|(canonical_path, loaded)| {
            let recipes: Vec<(String, Vec<String>)> = loaded
                .cookfile
                .recipes
                .iter()
                .map(|r| (r.name.clone(), r.deps.clone()))
                .chain(
                    loaded
                        .cookfile
                        .chores
                        .iter()
                        .map(|c| (c.name.clone(), c.deps.clone())),
                )
                .collect();
            (canonical_path.clone(), recipes)
        })
        .collect();

    cook_engine::analyzer::WorkspaceLayout {
        root_dir,
        root_recipes,
        imported_recipes,
        namespace_map: workspace.namespace_map.clone(),
    }
}

/// Build workspace recipe info and resolve via cook-engine's analyzer.
fn build_workspace_recipe_info(
    workspace: &Workspace,
) -> Result<std::collections::BTreeMap<String, cook_engine::analyzer::RecipeInfo>, CookError> {
    let layout = workspace_to_layout(workspace);
    Ok(cook_engine::analyzer::build_workspace_recipe_info(&layout))
}

/// Find the full dotted prefix for a canonical import path.
/// Delegates to cook-engine's analyzer.
pub fn find_full_prefix(workspace: &Workspace, canonical_path: &std::path::Path) -> String {
    let root_dir = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    cook_engine::analyzer::find_full_prefix(
        &workspace.namespace_map,
        &root_dir,
        canonical_path,
    )
}

/// Compute inferred dependencies from `{alias.recipe}` body refs across the
/// entire workspace (§7.3 union).
///
/// Returns a `BTreeMap<String, Vec<String>>` keyed by **qualified consumer name**
/// (e.g. `"top"` for a root recipe, `"web.web_obj"` for an imported one), valued
/// by a sorted-deduplicated vector of **qualified dep names**.  This is the same
/// shape that `cook_engine::run::run` already consumes via the `inferred_deps`
/// parameter.
///
/// The single-Cookfile path (see `cmd_run`) already computes this for the trivial
/// case; this function handles the workspace case where deps span Cookfile
/// boundaries and alias names must be resolved to canonical qualified prefixes.
fn compute_workspace_inferred_deps(workspace: &Workspace) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();

    // Build a canonical-path → &Cookfile snapshot for alias resolution.
    let root_canon = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    let mut canon_to_cookfile: BTreeMap<std::path::PathBuf, &cook_lang::ast::Cookfile> =
        BTreeMap::new();
    canon_to_cookfile.insert(root_canon.clone(), &workspace.root.cookfile);
    for (canon, loaded) in &workspace.imports {
        canon_to_cookfile.insert(canon.clone(), &loaded.cookfile);
    }

    // Collect all (canon_path, qualified_prefix, &Cookfile) triples.
    // Root has empty prefix; each import has a dotted prefix computed via find_full_prefix.
    let entries: Vec<(std::path::PathBuf, String, &cook_lang::ast::Cookfile)> =
        std::iter::once((root_canon.clone(), String::new(), &workspace.root.cookfile))
            .chain(workspace.imports.iter().map(|(canon, loaded)| {
                let prefix = find_full_prefix(workspace, canon);
                (canon.clone(), prefix, &loaded.cookfile)
            }))
            .collect();

    for (cookfile_canon, prefix, cookfile) in &entries {
        // For this Cookfile, build two maps keyed by local alias:
        //   alias_to_importee_prefix: alias → qualified prefix of the importee
        //   imports_by_alias:         alias → &Cookfile of the importee
        // Used to resolve `{alias.recipe}` tokens.
        let mut alias_to_importee_prefix: BTreeMap<String, String> = BTreeMap::new();
        let mut imports_by_alias: BTreeMap<String, &cook_lang::ast::Cookfile> = BTreeMap::new();
        for (parent_canon, alias, target_canon) in &workspace.namespace_map {
            if parent_canon != cookfile_canon {
                continue;
            }
            let importee_prefix =
                find_full_prefix(workspace, target_canon);
            alias_to_importee_prefix.insert(alias.clone(), importee_prefix);
            if let Some(cf) = canon_to_cookfile.get(target_canon) {
                imports_by_alias.insert(alias.clone(), cf);
            }
        }

        // Build the §7.3 union: local recipe names ∪ {alias.recipe} pairs for
        // direct imports.  This is what extract_dep_refs uses to distinguish
        // recipe references from env-var tokens.
        let union = cook_luagen::dep_ref::extract_recipe_names_with_imports(
            cookfile,
            &imports_by_alias,
        );

        for recipe in &cookfile.recipes {
            let refs = cook_luagen::dep_ref::extract_dep_refs(recipe, &union);
            if refs.is_empty() {
                continue;
            }

            // Qualify the consumer name.
            let consumer = if prefix.is_empty() {
                recipe.name.clone()
            } else {
                format!("{prefix}.{}", recipe.name)
            };

            let mut deps_set: BTreeSet<String> = BTreeSet::new();
            for dep_ref in refs {
                // dep_ref.recipe_name is either:
                //   "local_recipe"    — same-Cookfile reference (no dot)
                //   "alias.recipe"    — cross-Cookfile reference via local alias
                let qualified = if let Some((alias, sub)) =
                    dep_ref.recipe_name.split_once('.')
                {
                    // Cross-Cookfile: resolve alias → importee's qualified prefix.
                    if let Some(importee_prefix) = alias_to_importee_prefix.get(alias) {
                        if importee_prefix.is_empty() {
                            sub.to_string()
                        } else {
                            format!("{importee_prefix}.{sub}")
                        }
                    } else {
                        // Should not happen if the union was built correctly;
                        // skip defensively.
                        continue;
                    }
                } else if prefix.is_empty() {
                    // Same-Cookfile, root: no prefix needed.
                    dep_ref.recipe_name.clone()
                } else {
                    // Same-Cookfile, imported: prepend the Cookfile's prefix.
                    format!("{prefix}.{}", dep_ref.recipe_name)
                };
                deps_set.insert(qualified);
            }

            if !deps_set.is_empty() {
                out.insert(consumer, deps_set.into_iter().collect());
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Helper: write minimal Cookfile content and return the workspace.
    fn make_workspace(
        root_cookfile: &str,
        imports: &[(&str, &str)], // (dir_name, cookfile_content)
    ) -> (TempDir, Workspace) {
        let dir = TempDir::new().unwrap();
        // Write sub-Cookfiles first.
        for (sub_dir, content) in imports {
            fs::create_dir_all(dir.path().join(sub_dir)).unwrap();
            fs::write(dir.path().join(sub_dir).join("Cookfile"), content).unwrap();
        }
        fs::write(dir.path().join("Cookfile"), root_cookfile).unwrap();
        fs::write(dir.path().join(".cookroot"), "").unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = Workspace::load(&entry, &root, &[]).unwrap();
        (dir, ws)
    }

    /// Tree-relative case: root has `recipe top` referencing `{lib.lib_build}` in
    /// its body, lib has `recipe lib_build`.
    /// Expected: `{"top" -> ["lib.lib_build"]}`.
    #[test]
    fn workspace_inferred_deps_tree_relative() {
        let (_dir, ws) = make_workspace(
            "import lib ./lib\nrecipe top\n    cook \"build/top\" using { echo {lib.lib_build} }\n",
            &[("lib", "recipe lib_build\n    cook \"lib.o\" using { echo {out} }\n")],
        );
        let deps = compute_workspace_inferred_deps(&ws);
        assert_eq!(
            deps.get("top"),
            Some(&vec!["lib.lib_build".to_string()]),
            "expected top -> [lib.lib_build], got: {deps:?}"
        );
        // lib_build has no body refs → not in the map.
        assert!(deps.get("lib.lib_build").is_none());
    }

    /// Sigil case: root imports `apps/web` tree-relatively AND imports `core/lib`
    /// directly via sigil (`//core/lib`).  `apps/web` also imports `core/lib` via
    /// sigil.  This is a diamond: `core/lib` appears once in workspace.imports but
    /// is reachable from both root (as `core`) and web (as `core`).
    ///
    /// `web`'s `web_app` recipe references `{core.core_lib}`.  Because root
    /// directly imports core/lib with alias `core`, `find_full_prefix` walks up:
    /// core/lib → root → prefix = `"core"`.  So the dep should qualify as
    /// `core.core_lib`, not `web.core.core_lib`.
    #[test]
    fn workspace_inferred_deps_sigil_alias_resolves_to_importee_prefix() {
        let dir = TempDir::new().unwrap();
        // core/lib Cookfile
        fs::create_dir_all(dir.path().join("core/lib")).unwrap();
        fs::write(
            dir.path().join("core/lib/Cookfile"),
            "recipe core_lib\n    cook \"core.o\" using { echo {out} }\n",
        )
        .unwrap();
        // apps/web Cookfile — imports core via sigil, refs {core.core_lib}
        fs::create_dir_all(dir.path().join("apps/web")).unwrap();
        fs::write(
            dir.path().join("apps/web/Cookfile"),
            "import core //core/lib\nrecipe web_app\n    cook \"web.o\" using { echo {core.core_lib} }\n",
        )
        .unwrap();
        // root Cookfile: imports BOTH web (tree) AND core (sigil) directly.
        // This creates the diamond: core/lib is reachable as root→core AND as
        // root→web→core.  The workspace-level prefix is "core" (shortest root path).
        fs::write(
            dir.path().join("Cookfile"),
            "import web ./apps/web\nimport core //core/lib\nrecipe top\n    cook \"build/top\" using { echo {web.web_app} {core.core_lib} }\n",
        )
        .unwrap();
        fs::write(dir.path().join(".cookroot"), "").unwrap();

        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = Workspace::load(&entry, &root, &[]).unwrap();
        let deps = compute_workspace_inferred_deps(&ws);

        // web_app's {core.core_lib}: the local alias "core" in apps/web maps to the
        // workspace-level prefix "core" (core/lib is directly imported by root).
        assert_eq!(
            deps.get("web.web_app"),
            Some(&vec!["core.core_lib".to_string()]),
            "web_app should have dep on core.core_lib (importee workspace prefix), got: {deps:?}"
        );
        // top's body refs: {web.web_app} → "web.web_app" and {core.core_lib} → "core.core_lib".
        assert_eq!(
            deps.get("top"),
            Some(&vec!["core.core_lib".to_string(), "web.web_app".to_string()]),
            "top should have deps on web.web_app and core.core_lib, got: {deps:?}"
        );
    }

    /// Empty case: workspace where no recipes have body refs returns empty map.
    #[test]
    fn workspace_inferred_deps_empty_when_no_body_refs() {
        let (_dir, ws) = make_workspace(
            "import lib ./lib\nrecipe top\n    echo hello\n",
            &[("lib", "recipe lib_build\n    echo world\n")],
        );
        let deps = compute_workspace_inferred_deps(&ws);
        assert!(
            deps.is_empty(),
            "expected empty inferred_deps when no body refs, got: {deps:?}"
        );
    }
}
