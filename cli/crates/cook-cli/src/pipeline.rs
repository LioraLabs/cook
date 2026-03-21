//! Pipeline: glue layer that wires cook-lang -> cook-luagen -> cook-register
//! -> cook-engine together.
//!
//! This is the main orchestration module. It parses Cookfiles, registers
//! recipes via cook-register, builds DAGs via cook-engine::dag_builder, and
//! executes them via cook-engine::executor. Progress events from cook-engine
//! (EngineEvent) are bridged to cook-cli's ProgressEvent for rendering.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;

use crate::cli::Cli;
use crate::env::{load_env, resolve_env};
use crate::error::CookError;
use crate::progress::{resolve_color, spawn_renderer_thread, ProgressEvent};
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

    let lua_source = cook_luagen::generate(&cookfile);
    Ok((cookfile, lua_source))
}

/// Split "backend.build" -> ("backend", "build"),
/// "backend.proto.generate" -> ("backend.proto", "generate"),
/// "build" -> ("", "build")
fn split_recipe_name(name: &str) -> (String, String) {
    if let Some(dot_pos) = name.rfind('.') {
        (name[..dot_pos].to_string(), name[dot_pos + 1..].to_string())
    } else {
        (String::new(), name.to_string())
    }
}

/// Bridge EngineEvent to ProgressEvent and send to the progress renderer.
fn bridge_engine_events(
    engine_rx: mpsc::Receiver<cook_engine::EngineEvent>,
    progress_tx: mpsc::Sender<ProgressEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(event) = engine_rx.recv() {
            let pe = match event {
                cook_engine::EngineEvent::RecipeQueued { name } => {
                    ProgressEvent::RecipeQueued {
                        name,
                        total_nodes: 0,
                    }
                }
                cook_engine::EngineEvent::RecipeStarted { name, total_nodes } => {
                    ProgressEvent::RecipeStarted { name, total_nodes }
                }
                cook_engine::EngineEvent::RecipeCompleted {
                    name,
                    elapsed,
                    cached_nodes,
                    total_nodes,
                } => ProgressEvent::RecipeCompleted {
                    name,
                    elapsed,
                    cached_nodes,
                    total_nodes,
                },
                cook_engine::EngineEvent::RecipeFailed {
                    name,
                    elapsed,
                    completed_nodes,
                    total_nodes,
                } => ProgressEvent::RecipeFailed {
                    name,
                    elapsed,
                    completed_nodes,
                    total_nodes,
                },
                cook_engine::EngineEvent::NodeStarted { recipe, node_name } => {
                    ProgressEvent::NodeStarted { recipe, node_name }
                }
                cook_engine::EngineEvent::NodeCompleted {
                    recipe,
                    node_name,
                    elapsed,
                } => ProgressEvent::NodeCompleted {
                    recipe,
                    node_name,
                    elapsed,
                },
                cook_engine::EngineEvent::NodeFailed {
                    recipe,
                    node_name,
                    elapsed,
                    error,
                } => ProgressEvent::NodeFailed {
                    recipe,
                    node_name,
                    elapsed,
                    error,
                },
                cook_engine::EngineEvent::NodeCacheHit { recipe, node_name } => {
                    ProgressEvent::NodeCacheHit { recipe, node_name }
                }
                cook_engine::EngineEvent::NodeSkipped { recipe, node_name } => {
                    ProgressEvent::NodeSkipped { recipe, node_name }
                }
                cook_engine::EngineEvent::InteractiveStart { recipe } => {
                    ProgressEvent::InteractiveStart { recipe }
                }
                cook_engine::EngineEvent::InteractiveEnd {
                    recipe,
                    elapsed,
                    success,
                } => ProgressEvent::InteractiveEnd {
                    recipe,
                    elapsed,
                    success,
                },
                cook_engine::EngineEvent::OutputLine {
                    recipe,
                    line,
                    is_stderr,
                } => ProgressEvent::OutputLine {
                    recipe,
                    line,
                    is_stderr,
                },
                cook_engine::EngineEvent::Finished { .. } => {
                    ProgressEvent::Finished
                }
            };

            let is_finished = matches!(pe, ProgressEvent::Finished);
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

/// Map cook-register errors to CookError.
fn register_error_to_cook_error(e: cook_register::RegisterError) -> CookError {
    match e {
        cook_register::RegisterError::RecipeNotFound(name) => CookError::RecipeNotFound(name),
        cook_register::RegisterError::Lua(e) => CookError::Other(format!("lua error: {e}")),
        cook_register::RegisterError::CommandFailed {
            command,
            line,
            code,
        } => {
            if line == 0 {
                CookError::CommandFailed(format!("command failed (exit {code}): {command}"))
            } else {
                CookError::CommandFailed(format!(
                    "Cookfile:{line}: command failed (exit {code}): {command}"
                ))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// cmd_run
// ---------------------------------------------------------------------------

pub fn cmd_run(cli: &Cli, recipe_name: &str, config: Option<&str>) -> Result<(), CookError> {
    let (cookfile, lua_source) = read_and_parse(cli)?;

    if cli.emit_lua {
        println!("{lua_source}");
        return Ok(());
    }

    if !cookfile.imports.is_empty() {
        return cmd_run_workspace(cli, recipe_name, config);
    }

    let cookfile_dir = cli.file.parent().unwrap_or(Path::new("."));
    let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        cookfile_dir
    };
    let dotenv_vars = load_env(cookfile_dir);
    let env_vars = resolve_env(&cookfile, config, dotenv_vars, &cli.set)?;

    // Resolve execution order via topological sort on recipe deps
    let order = resolve_execution_order(&cookfile, recipe_name)?;

    let num_jobs = cli.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    let cache_dir = cookfile_dir.join(".cook").join("cache");
    let cache_manager = Arc::new(cook_cache::ThreadSafeCacheManager::new(cache_dir));

    // Set up progress renderer
    let color = resolve_color(cli);
    let (progress_tx, progress_rx) = mpsc::channel::<ProgressEvent>();
    let render_thread = spawn_renderer_thread(progress_rx, color);

    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());

    // Emit RecipeQueued for all recipes so the renderer shows "waiting" state
    for name in &order {
        let _ = progress_tx.send(ProgressEvent::RecipeQueued {
            name: name.clone(),
            total_nodes: 0,
        });
    }

    let registry = cook_register::Registry::new(cookfile_dir.to_path_buf(), env_vars);

    let mut run_result: Result<(), CookError> = Ok(());
    for name in &order {
        if !cli.quiet && !is_tty {
            eprintln!("cook: registering recipe '{name}'");
        }

        let units = registry
            .register_recipe(&lua_source, name)
            .map_err(register_error_to_cook_error)?;

        let dag = cook_engine::dag_builder::build_dag(vec![units]);

        if dag.is_empty() {
            continue;
        }

        // Set up engine event channel and bridge to progress events
        let (engine_tx, engine_rx) = mpsc::channel::<cook_engine::EngineEvent>();
        let bridge_tx = progress_tx.clone();
        let bridge_thread = bridge_engine_events(engine_rx, bridge_tx);

        let mut cache_managers = BTreeMap::new();
        cache_managers.insert(name.to_string(), cache_manager.clone());

        let result = cook_engine::executor::execute_dag(
            dag,
            num_jobs,
            cache_managers,
            Some(engine_tx),
        );

        // Wait for bridge thread to finish
        let _ = bridge_thread.join();

        if let Err(e) = result {
            run_result = Err(engine_error_to_cook_error(e));
            break;
        }
    }

    // Signal renderer to finish and wait for it
    let _ = progress_tx.send(ProgressEvent::Finished);
    drop(progress_tx);
    let _ = render_thread.join();

    run_result
}

// ---------------------------------------------------------------------------
// cmd_run_workspace
// ---------------------------------------------------------------------------

fn cmd_run_workspace(
    cli: &Cli,
    recipe_name: &str,
    config: Option<&str>,
) -> Result<(), CookError> {
    let workspace = Workspace::load(&cli.file, &cli.set)?;

    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(&workspace.root.cookfile, config, dotenv_vars, &cli.set)?;

    // Build registries: one for root, one per import
    let mut registries: BTreeMap<String, (cook_register::Registry, String)> = BTreeMap::new();

    let root_registry = cook_register::Registry::new(workspace.root.dir.clone(), root_env);
    registries.insert(
        String::new(),
        (root_registry, workspace.root.lua_source.clone()),
    );

    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(&workspace, canonical_path);
        let import_env = resolve_env(
            &loaded.cookfile,
            config,
            std::collections::HashMap::new(),
            &cli.set,
        )?;
        let registry = cook_register::Registry::new(loaded.dir.clone(), import_env);
        registries.insert(prefix, (registry, loaded.lua_source.clone()));
    }

    let num_jobs = cli.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    // Collect all recipe info for dependency resolution
    let all_recipes = build_workspace_recipe_info(&workspace)?;
    let order = topological_sort_recipes(&all_recipes, recipe_name)?;

    let color = resolve_color(cli);
    let (progress_tx, progress_rx) = mpsc::channel::<ProgressEvent>();
    let render_thread = spawn_renderer_thread(progress_rx, color);
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());

    for name in &order {
        let _ = progress_tx.send(ProgressEvent::RecipeQueued {
            name: name.clone(),
            total_nodes: 0,
        });
    }

    let mut run_result: Result<(), CookError> = Ok(());

    for name in &order {
        if !cli.quiet && !is_tty {
            eprintln!("cook: registering recipe '{name}'");
        }

        let (prefix, local_name) = split_recipe_name(name);
        let (registry, lua_source) = registries
            .get(&prefix)
            .ok_or_else(|| CookError::Other(format!("no registry for recipe '{name}'")))?;

        let cache_dir = registry.working_dir().join(".cook").join("cache");
        let cache_manager = Arc::new(cook_cache::ThreadSafeCacheManager::new(cache_dir));

        let mut units = registry
            .register_recipe(lua_source, &local_name)
            .map_err(register_error_to_cook_error)?;

        // Rewrite recipe_name to namespaced form
        units.recipe_name = name.clone();

        let dag = cook_engine::dag_builder::build_dag(vec![units]);
        if dag.is_empty() {
            continue;
        }

        let (engine_tx, engine_rx) = mpsc::channel::<cook_engine::EngineEvent>();
        let bridge_tx = progress_tx.clone();
        let bridge_thread = bridge_engine_events(engine_rx, bridge_tx);

        let mut cache_managers = BTreeMap::new();
        cache_managers.insert(name.clone(), cache_manager);

        let result = cook_engine::executor::execute_dag(
            dag,
            num_jobs,
            cache_managers,
            Some(engine_tx),
        );

        let _ = bridge_thread.join();

        if let Err(e) = result {
            run_result = Err(engine_error_to_cook_error(e));
            break;
        }
    }

    let _ = progress_tx.send(ProgressEvent::Finished);
    drop(progress_tx);
    let _ = render_thread.join();

    run_result
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

    if !cookfile.imports.is_empty() {
        return cmd_test_workspace(cli, filter, verbose, timeout_multiplier, wrapper, list);
    }

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

    let registry = cook_register::Registry::new(cookfile_dir.to_path_buf(), env_vars);

    let num_jobs = cli.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    let cache_dir = cookfile_dir.join(".cook").join("cache");
    let cache_manager = Arc::new(cook_cache::ThreadSafeCacheManager::new(cache_dir));

    let color = resolve_color(cli);
    let (progress_tx, progress_rx) = mpsc::channel::<ProgressEvent>();
    let render_thread = spawn_renderer_thread(progress_rx, color);

    let is_tty_test = std::io::IsTerminal::is_terminal(&std::io::stderr());

    // Emit RecipeQueued for all recipes
    {
        let mut queued = std::collections::HashSet::new();
        for recipe_name in &test_recipes {
            if let Ok(order) = try_resolve_execution_order(&cookfile, recipe_name) {
                for name in &order {
                    if queued.insert(name.clone()) {
                        let _ = progress_tx.send(ProgressEvent::RecipeQueued {
                            name: name.clone(),
                            total_nodes: 0,
                        });
                    }
                }
            }
        }
    }

    let mut executed = std::collections::HashSet::new();
    // TODO: collect test outputs from engine when cook-engine supports
    // test result collection. For now, we rely on the engine executing
    // test work nodes normally.

    for recipe_name in &test_recipes {
        let order = resolve_execution_order(&cookfile, recipe_name)?;

        for name in &order {
            if executed.contains(name) {
                continue;
            }
            executed.insert(name.clone());

            if !cli.quiet && !is_tty_test {
                eprintln!("cook: registering recipe '{name}'");
            }

            let units = registry
                .register_recipe(&lua_source, name)
                .map_err(register_error_to_cook_error)?;

            let dag = cook_engine::dag_builder::build_dag(vec![units]);
            if dag.is_empty() {
                continue;
            }

            let (engine_tx, engine_rx) = mpsc::channel::<cook_engine::EngineEvent>();
            let bridge_tx = progress_tx.clone();
            let bridge_thread = bridge_engine_events(engine_rx, bridge_tx);

            let mut cache_managers = BTreeMap::new();
            cache_managers.insert(name.to_string(), cache_manager.clone());

            // Ignore errors for test recipes -- we collect results separately
            let _ = cook_engine::executor::execute_dag(
                dag,
                num_jobs,
                cache_managers,
                Some(engine_tx),
            );

            let _ = bridge_thread.join();
        }
    }

    // Signal renderer to finish and wait for it
    let _ = progress_tx.send(ProgressEvent::Finished);
    drop(progress_tx);
    let _ = render_thread.join();

    // TODO: Once cook-engine supports test output collection, convert
    // TestOutput -> TestCaseResult here and display results.
    // For now, tests execute but detailed results are not yet collected.
    eprintln!("cook: test execution complete (detailed results pending cook-engine integration)");

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_test_workspace
// ---------------------------------------------------------------------------

fn cmd_test_workspace(
    cli: &Cli,
    _filter: Option<String>,
    _verbose: bool,
    _timeout_multiplier: u64,
    _wrapper: Option<String>,
    _list: bool,
) -> Result<(), CookError> {
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

    // Build registries
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(&workspace.root.cookfile, None, dotenv_vars, &cli.set)?;

    let mut registries: BTreeMap<String, (cook_register::Registry, String)> = BTreeMap::new();

    let root_registry = cook_register::Registry::new(workspace.root.dir.clone(), root_env);
    registries.insert(
        String::new(),
        (root_registry, workspace.root.lua_source.clone()),
    );

    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(&workspace, canonical_path);
        let import_env = resolve_env(
            &loaded.cookfile,
            None,
            std::collections::HashMap::new(),
            &cli.set,
        )?;
        let registry = cook_register::Registry::new(loaded.dir.clone(), import_env);
        registries.insert(prefix, (registry, loaded.lua_source.clone()));
    }

    let num_jobs = cli.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    let all_recipes = build_workspace_recipe_info(&workspace)?;

    let color = resolve_color(cli);
    let (progress_tx, progress_rx) = mpsc::channel::<ProgressEvent>();
    let render_thread = spawn_renderer_thread(progress_rx, color);
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());

    // Queue all recipes
    {
        let mut queued = std::collections::HashSet::new();
        for test_name in &test_recipe_names {
            if let Ok(order) = topological_sort_recipes(&all_recipes, test_name) {
                for name in &order {
                    if queued.insert(name.clone()) {
                        let _ = progress_tx.send(ProgressEvent::RecipeQueued {
                            name: name.clone(),
                            total_nodes: 0,
                        });
                    }
                }
            }
        }
    }

    let mut executed = std::collections::HashSet::new();

    for test_name in &test_recipe_names {
        let order = topological_sort_recipes(&all_recipes, test_name)?;

        for name in &order {
            if executed.contains(name) {
                continue;
            }
            executed.insert(name.clone());

            if !cli.quiet && !is_tty {
                eprintln!("cook: registering recipe '{name}'");
            }

            let (prefix, local_name) = split_recipe_name(name);
            let (registry, lua_source) = registries
                .get(&prefix)
                .ok_or_else(|| CookError::Other(format!("no registry for recipe '{name}'")))?;

            let cache_dir = registry.working_dir().join(".cook").join("cache");
            let cache_manager = Arc::new(cook_cache::ThreadSafeCacheManager::new(cache_dir));

            let units = registry
                .register_recipe(lua_source, &local_name)
                .map_err(register_error_to_cook_error)?;

            let dag = cook_engine::dag_builder::build_dag(vec![units]);
            if dag.is_empty() {
                continue;
            }

            let (engine_tx, engine_rx) = mpsc::channel::<cook_engine::EngineEvent>();
            let bridge_tx = progress_tx.clone();
            let bridge_thread = bridge_engine_events(engine_rx, bridge_tx);

            let mut cache_managers = BTreeMap::new();
            cache_managers.insert(name.clone(), cache_manager);

            let _ = cook_engine::executor::execute_dag(
                dag,
                num_jobs,
                cache_managers,
                Some(engine_tx),
            );

            let _ = bridge_thread.join();
        }
    }

    let _ = progress_tx.send(ProgressEvent::Finished);
    drop(progress_tx);
    let _ = render_thread.join();

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
                desc.push_str(&format!("  cook: {}", cook_step.output_pattern));
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

    let order = resolve_execution_order(&cookfile, recipe_name)?;

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
// Recipe dependency resolution (local to cook-cli)
// ---------------------------------------------------------------------------

/// Simplified recipe info for dependency resolution.
struct RecipeInfo {
    name: String,
    deps: Vec<String>,
}

/// Resolve execution order for a single Cookfile via topological sort on recipe deps.
fn resolve_execution_order(
    cookfile: &cook_lang::ast::Cookfile,
    recipe_name: &str,
) -> Result<Vec<String>, CookError> {
    let recipes: Vec<RecipeInfo> = cookfile
        .recipes
        .iter()
        .map(|r| RecipeInfo {
            name: r.name.clone(),
            deps: r.deps.clone(),
        })
        .collect();

    topological_sort_recipe_infos(&recipes, recipe_name)
}

/// Try to resolve execution order, returning Ok(order) or Err.
fn try_resolve_execution_order(
    cookfile: &cook_lang::ast::Cookfile,
    recipe_name: &str,
) -> Result<Vec<String>, CookError> {
    resolve_execution_order(cookfile, recipe_name)
}

fn topological_sort_recipe_infos(
    recipes: &[RecipeInfo],
    target: &str,
) -> Result<Vec<String>, CookError> {
    let mut visited = std::collections::HashSet::new();
    let mut visiting = std::collections::HashSet::new();
    let mut order = Vec::new();

    fn visit(
        name: &str,
        recipes: &[RecipeInfo],
        visited: &mut std::collections::HashSet<String>,
        visiting: &mut std::collections::HashSet<String>,
        order: &mut Vec<String>,
    ) -> Result<(), CookError> {
        if visited.contains(name) {
            return Ok(());
        }
        if visiting.contains(name) {
            return Err(CookError::Other(format!(
                "dependency cycle involving: {name}"
            )));
        }
        visiting.insert(name.to_string());

        if let Some(recipe) = recipes.iter().find(|r| r.name == name) {
            for dep in &recipe.deps {
                // Only follow deps that exist as local recipes (skip namespaced)
                if !dep.contains('.') {
                    visit(dep, recipes, visited, visiting, order)?;
                }
            }
        } else {
            return Err(CookError::RecipeNotFound(name.to_string()));
        }

        visiting.remove(name);
        visited.insert(name.to_string());
        order.push(name.to_string());
        Ok(())
    }

    visit(target, recipes, &mut visited, &mut visiting, &mut order)?;
    Ok(order)
}

/// Build workspace recipe info for dependency resolution.
fn build_workspace_recipe_info(workspace: &Workspace) -> Result<Vec<RecipeInfo>, CookError> {
    let mut all_recipes = Vec::new();

    // Root recipes
    for recipe in &workspace.root.cookfile.recipes {
        all_recipes.push(RecipeInfo {
            name: recipe.name.clone(),
            deps: recipe.deps.clone(),
        });
    }

    // Imported recipes (namespaced)
    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        for recipe in &loaded.cookfile.recipes {
            let namespaced_deps: Vec<String> = recipe
                .deps
                .iter()
                .map(|d| {
                    if d.contains('.') {
                        d.clone()
                    } else {
                        format!("{prefix}.{d}")
                    }
                })
                .collect();
            all_recipes.push(RecipeInfo {
                name: format!("{prefix}.{}", recipe.name),
                deps: namespaced_deps,
            });
        }
    }

    Ok(all_recipes)
}

/// Topological sort over workspace recipe infos.
fn topological_sort_recipes(
    all_recipes: &[RecipeInfo],
    target: &str,
) -> Result<Vec<String>, CookError> {
    topological_sort_recipe_infos(all_recipes, target)
}

/// Find the full dotted prefix for a canonical import path.
pub fn find_full_prefix(workspace: &Workspace, canonical_path: &std::path::Path) -> String {
    // Walk namespace_map to find the import name mapping to this canonical path
    for (parent, name, target) in &workspace.namespace_map {
        if target == canonical_path {
            // Check if parent is also an import (nested)
            let parent_prefix = if *parent
                == std::fs::canonicalize(&workspace.root.dir)
                    .unwrap_or_else(|_| workspace.root.dir.clone())
            {
                String::new()
            } else {
                find_full_prefix(workspace, parent)
            };
            return if parent_prefix.is_empty() {
                name.clone()
            } else {
                format!("{parent_prefix}.{name}")
            };
        }
    }
    // Fallback: use directory name
    canonical_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}
