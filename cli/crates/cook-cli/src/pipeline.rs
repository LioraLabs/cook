//! Pipeline: glue layer that wires cook-lang -> cook-luagen -> cook-register
//! -> cook-engine together.
//!
//! This is the main orchestration module. It parses Cookfiles, builds recipe
//! metadata and registries, then delegates execution to `cook_engine::run::run()`
//! which handles wave-parallel DAG execution for both single-Cookfile and
//! workspace builds.

use std::collections::BTreeMap;
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

    // § 5.5 — register-time warnings for references whose referent has an
    // empty output list.
    let (lua_source, warnings) =
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

/// Build recipe_infos from a single Cookfile's recipes.
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
    recipe_infos
}

/// Build a single-Cookfile registry map (empty-string prefix).
fn build_single_registries(
    cookfile_dir: &Path,
    env_vars: std::collections::HashMap<String, String>,
    lua_source: String,
    selected_config: Option<&str>,
) -> BTreeMap<String, (cook_register::Registry, String)> {
    let registry = cook_register::Registry::new(cookfile_dir.to_path_buf(), env_vars)
        .with_selected_config(selected_config.map(|s| s.to_string()));
    let mut registries = BTreeMap::new();
    registries.insert(String::new(), (registry, lua_source));
    registries
}

/// Build workspace registries: one for root (empty prefix), one per import.
fn build_workspace_registries(
    workspace: &Workspace,
    config: Option<&str>,
    cli_sets: &[String],
) -> Result<BTreeMap<String, (cook_register::Registry, String)>, CookError> {
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(&workspace.root.cookfile, config, dotenv_vars, cli_sets)?;

    let mut registries: BTreeMap<String, (cook_register::Registry, String)> = BTreeMap::new();

    let root_registry = cook_register::Registry::new(workspace.root.dir.clone(), root_env)
        .with_selected_config(config.map(|s| s.to_string()));
    registries.insert(
        String::new(),
        (root_registry, workspace.root.lua_source.clone()),
    );

    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        let import_env = resolve_env(
            &loaded.cookfile,
            config,
            std::collections::HashMap::new(),
            cli_sets,
        )?;
        let registry = cook_register::Registry::new(loaded.dir.clone(), import_env)
            .with_selected_config(config.map(|s| s.to_string()));
        registries.insert(prefix, (registry, loaded.lua_source.clone()));
    }

    Ok(registries)
}

/// Run the engine with progress rendering wired up.
fn run_with_progress(
    cli: &Cli,
    recipe_infos: &BTreeMap<String, cook_engine::analyzer::RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, (cook_register::Registry, String)>,
    num_jobs: usize,
    inferred_deps: &BTreeMap<String, Vec<String>>,
) -> Result<cook_engine::run::RunResult, CookError> {
    let project_root = std::env::current_dir()
        .map_err(|e| CookError::Other(e.to_string()))?;
    let (progress_tx, progress_rx) = mpsc::channel::<cook_progress::ProgressEvent>();
    let render_thread = spawn_new_renderer(cli, project_root, progress_rx);

    let bridge_tx = progress_tx.clone();
    let (engine_tx, engine_rx) = mpsc::channel::<cook_engine::EngineEvent>();
    let bridge_thread = bridge_engine_to_progress_events(engine_rx, bridge_tx);

    let result = cook_engine::run::run(
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
        // Workspace build — workspace {dep} support is future work
        let workspace = Workspace::load(&cli.file, &cli.set)?;
        let recipe_infos = build_workspace_recipe_info(&workspace)?;
        let registries = build_workspace_registries(&workspace, config, &cli.set)?;

        run_with_progress(cli, &recipe_infos, &targets, &registries, num_jobs, &BTreeMap::new())?;
    } else {
        // Single Cookfile build
        let cookfile_dir = cli.file.parent().unwrap_or(Path::new("."));
        let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            cookfile_dir
        };
        let dotenv_vars = load_env(cookfile_dir);
        let env_vars = resolve_env(&cookfile, config, dotenv_vars, &cli.set)?;

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
        let workspace = Workspace::load(&cli.file, &cli.set)?;

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
        let env_vars = resolve_env(&cookfile, None, dotenv_vars, &cli.set)?;

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
        let mut desc = format!("  {}", recipe.name);
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

    if !cookfile.imports.is_empty() {
        let workspace = Workspace::load(&cli.file, &cli.set)?;
        for (canonical_path, loaded) in &workspace.imports {
            let prefix = find_full_prefix(&workspace, canonical_path);
            for recipe in &loaded.cookfile.recipes {
                println!("  {}.{}", prefix, recipe.name);
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

    // Check for interactive steps -- not supported under cook serve
    for recipe in &cookfile.recipes {
        for step in &recipe.steps {
            if let cook_lang::ast::Step::Shell {
                interactive: true,
                line,
                ..
            } = step
            {
                return Err(CookError::Other(format!(
                    "line {}: interactive '@' steps are not supported under 'cook serve'",
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
        let workspace = Workspace::load(&cli.file, &cli.set)?;
        for (_canonical_path, loaded) in &workspace.imports {
            let import_cookfile = loaded.dir.join("Cookfile");
            if let Ok(canonical) = std::fs::canonicalize(&import_cookfile) {
                cookfile_paths.push(canonical);
            }
        }
    }

    let watcher = CookWatcher::new(globs, cookfile_paths);

    eprintln!("cook serve: initial build...");
    let _ = cmd_run(cli, recipe_name, config);

    eprintln!("cook serve: watching for changes...");
    watcher
        .watch(|cookfile_changed| {
            if cookfile_changed {
                eprintln!("cook serve: Cookfile changed, rebuilding...");
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
    let env_vars = resolve_env(&cookfile, config, dotenv_vars, &cli.set)?;

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
            let units = registry.register_recipe(&lua_source, name).map_err(|e| {
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

    let root_recipes: Vec<(String, Vec<String>)> = workspace
        .root
        .cookfile
        .recipes
        .iter()
        .map(|r| (r.name.clone(), r.deps.clone()))
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
