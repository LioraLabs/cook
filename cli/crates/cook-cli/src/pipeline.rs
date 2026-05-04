//! CLI dispatch: thin wrappers around `cook_engine::pipeline` orchestration.
//!
//! Heavy lifting (parse, workspace load, registry assembly, dep inference)
//! lives in `cook_engine::pipeline`. This module owns CLI-specific glue:
//!   * mapping `--menu` / `--init` / `--serve` / `--dag` flags to the right
//!     engine entry point
//!   * bridging `cook_engine::EngineEvent` to `cook_progress::ProgressEvent`
//!     and wiring the renderer into a background thread
//!   * mapping `EngineError` / `PipelineError` into `CookError` for exit-code
//!     classification and human-facing diagnostics
//!
//! In particular: nothing in this file consumes a `cook_lang::ast::Cookfile`
//! directly anymore — that's the engine's concern.

use std::collections::BTreeMap;
use std::sync::mpsc;

use cook_engine::pipeline::{self, ParsedCookfile, PipelineError, Workspace};

use crate::cli::Cli;
use crate::error::CookError;
use crate::progress::spawn_new_renderer;
use crate::watcher::CookWatcher;

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// Map `cook_engine::pipeline::PipelineError` onto `CookError` for the CLI's
/// exit-code classification.
fn pipeline_error_to_cook_error(e: PipelineError) -> CookError {
    match e {
        PipelineError::Io { path, source } => {
            CookError::Other(format!("cannot read {}: {source}", path.display()))
        }
        PipelineError::Parse(msg) => CookError::ParseError(msg),
        PipelineError::Codegen(msg) => CookError::Other(msg),
        PipelineError::UnknownConfig { .. }
        | PipelineError::Workspace(_)
        | PipelineError::InvalidSet(_)
        | PipelineError::Other(_) => CookError::Other(e.to_string()),
    }
}

/// Parse the Cookfile and print any codegen warnings to stderr.
///
/// Thin convenience wrapper: the engine returns warnings as data; the CLI
/// is responsible for surfacing them in the human-output channel.
fn read_and_parse(cli: &Cli) -> Result<ParsedCookfile, CookError> {
    let parsed = pipeline::read_and_parse(&cli.file).map_err(pipeline_error_to_cook_error)?;
    for w in &parsed.warnings {
        eprintln!("cook: warning: {w}");
    }
    Ok(parsed)
}

fn print_dep_conflicts(warnings: &[String]) {
    for w in warnings {
        eprintln!("cook: warning: {w}");
    }
}

// ---------------------------------------------------------------------------
// EngineEvent → ProgressEvent bridge
// ---------------------------------------------------------------------------

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
                    stream,
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
                    // CS-0035: map cook-contracts::OutputStream → cook-progress::Stream
                    // (the wire-format enum). The two enums are isomorphic; this
                    // is the renderer-side adapter, not a value mutation.
                    let stream = match stream {
                        cook_engine::Stream::Stdout => Stream::Stdout,
                        cook_engine::Stream::Stderr => Stream::Stderr,
                    };
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
        cook_engine::EngineError::OutputCollision { path, recipes } => CookError::Other(format!(
            "output collision: recipes [{}] all declare output {} with no dependency edge between them; \
             add an explicit `: <recipe>` dep or merge into one recipe",
            recipes.join(", "),
            path.display(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Run-with-progress glue
// ---------------------------------------------------------------------------

/// Run the engine with progress rendering wired up.
fn run_with_progress(
    cli: &Cli,
    recipe_infos: &BTreeMap<String, cook_engine::analyzer::RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, cook_engine::RegistryEntry>,
    num_jobs: usize,
    inferred_deps: &BTreeMap<String, Vec<String>>,
) -> Result<cook_engine::run::RunResult, CookError> {
    let project_root = std::env::current_dir().map_err(|e| CookError::Other(e.to_string()))?;
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
    let parsed = read_and_parse(cli)?;
    pipeline::validate_selected_config(&parsed.cookfile, config)
        .map_err(pipeline_error_to_cook_error)?;

    if cli.emit_lua {
        println!("{}", parsed.lua_source);
        return Ok(());
    }

    let num_jobs = resolve_num_jobs(cli);
    let targets = vec![recipe_name.to_string()];

    if !parsed.cookfile.imports.is_empty() {
        let workspace_root =
            pipeline::resolve_workspace_root(&cli.file, cli.root.clone())
                .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&cli.file, &workspace_root, &cli.set)
            .map_err(pipeline_error_to_cook_error)?;
        let recipe_infos = pipeline::build_workspace_recipe_info(&workspace);
        let registries = pipeline::build_workspace_registries(&workspace, config, &cli.set)
            .map_err(pipeline_error_to_cook_error)?;

        let inferred_deps = pipeline::compute_workspace_inferred_deps(&workspace);
        print_dep_conflicts(&pipeline::workspace_dep_conflicts(&workspace, &inferred_deps));

        run_with_progress(cli, &recipe_infos, &targets, &registries, num_jobs, &inferred_deps)?;
    } else {
        // Single Cookfile build
        let cookfile_dir = cli.file.parent().unwrap_or(std::path::Path::new("."));
        let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            cookfile_dir
        };
        let dotenv_vars = pipeline::load_env(cookfile_dir);
        let env_vars = pipeline::resolve_env(config, dotenv_vars, &cli.set)
            .map_err(pipeline_error_to_cook_error)?;

        let recipe_infos = pipeline::build_single_recipe_infos(&parsed.cookfile);
        let inferred_deps = pipeline::compute_single_inferred_deps(&parsed.cookfile);
        print_dep_conflicts(&pipeline::single_dep_conflicts(&parsed.cookfile));

        let registries =
            pipeline::build_single_registries(cookfile_dir, env_vars, parsed.lua_source, config);

        run_with_progress(cli, &recipe_infos, &targets, &registries, num_jobs, &inferred_deps)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_menu
// ---------------------------------------------------------------------------

pub fn cmd_menu(cli: &Cli) -> Result<(), CookError> {
    let parsed = read_and_parse(cli)?;

    for recipe in &parsed.cookfile.recipes {
        let mut desc = format!("  recipe {}", recipe.name);
        if !recipe.ingredients.is_empty() {
            desc.push_str(&format!("  ingredients: {:?}", recipe.ingredients));
        }
        if !recipe.deps.is_empty() {
            desc.push_str(&format!("  deps: {:?}", recipe.deps));
        }
        for step in &recipe.steps {
            if let cook_lang::ast::Step::Cook { step: cook_step, .. } = step {
                desc.push_str(&format!("  cook: {}", cook_step.outputs.join(" ")));
            }
        }
        println!("{desc}");
    }

    for chore in &parsed.cookfile.chores {
        let mut desc = format!("  chore  {}", chore.name);
        if !chore.deps.is_empty() {
            desc.push_str(&format!("  deps: {:?}", chore.deps));
        }
        println!("{desc}");
    }

    if !parsed.cookfile.imports.is_empty() {
        let workspace_root =
            pipeline::resolve_workspace_root(&cli.file, cli.root.clone())
                .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&cli.file, &workspace_root, &cli.set)
            .map_err(pipeline_error_to_cook_error)?;
        for (canonical_path, loaded) in &workspace.imports {
            let prefix = pipeline::find_full_prefix(&workspace, canonical_path);
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
    // CS-0019 dropped `end`: recipe bodies are indented and terminated by the
    // next column-0 keyword or EOF. Emitting `end` here was a v0.3-era
    // template; under the current grammar that line parses as a literal
    // shell command and the build fails with exit 127.
    std::fs::write(
        path,
        r#"recipe build
    echo "Hello from Cook!"
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
    let parsed = read_and_parse(cli)?;
    pipeline::validate_selected_config(&parsed.cookfile, config)
        .map_err(pipeline_error_to_cook_error)?;

    // Check for interactive steps -- not supported under cook --serve
    for recipe in &parsed.cookfile.recipes {
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
    let recipe_infos = pipeline::build_single_recipe_infos(&parsed.cookfile);
    let order =
        cook_engine::analyzer::topological_sort(&recipe_infos, recipe_name).map_err(|e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                CookError::RecipeNotFound(name)
            }
        })?;

    let globs = CookWatcher::collect_globs_for_recipes(&parsed.cookfile, &order);
    if globs.is_empty() {
        return Err(CookError::Other(
            "nothing to watch: no recipes in the chain have ingredients".to_string(),
        ));
    }

    let cookfile_path = std::fs::canonicalize(&cli.file)
        .map_err(|e| CookError::Other(format!("cannot resolve Cookfile path: {e}")))?;

    let mut cookfile_paths = vec![cookfile_path];

    // If imports exist, collect all imported Cookfile paths for watching
    if !parsed.cookfile.imports.is_empty() {
        let workspace_root =
            pipeline::resolve_workspace_root(&cli.file, cli.root.clone())
                .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&cli.file, &workspace_root, &cli.set)
            .map_err(pipeline_error_to_cook_error)?;
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
// cmd_dag — feature-gated
// ---------------------------------------------------------------------------
//
// The DAG viewer (`cook --dag`) lives in the `cook-dag-viewer` crate and is
// pulled in only when the `viewer` cargo feature is enabled (see
// `Cargo.toml`). When the feature is off, `cmd_dag` short-circuits with a
// helpful error so users learn which build flag they need. The reference-
// implementation policy is documented in the Cook Standard at
// `standard/src/content/docs/appendix/D-changes.mdx#changes-cs-0047`.

#[cfg(not(feature = "viewer"))]
pub fn cmd_dag(_cli: &Cli, _recipe_name: &str, _config: Option<&str>) -> Result<(), CookError> {
    Err(CookError::Other(
        "the `cook --dag` viewer is not built into this binary; rebuild with \
         `cargo build --features viewer` (or pass `--features viewer` when \
         running `cargo install`)"
            .to_string(),
    ))
}

#[cfg(feature = "viewer")]
pub fn cmd_dag(cli: &Cli, recipe_name: &str, config: Option<&str>) -> Result<(), CookError> {
    let parsed = read_and_parse(cli)?;
    pipeline::validate_selected_config(&parsed.cookfile, config)
        .map_err(pipeline_error_to_cook_error)?;

    let targets = vec![recipe_name.to_string()];

    // Workspace branch — App. E.10 closes the previous "cmd_dag has no
    // workspace mode at all" gap by mirroring cmd_run's workspace setup and
    // routing each ready recipe through its prefix's registry (the same
    // dispatch cook_engine::run::run does internally).
    let units = if !parsed.cookfile.imports.is_empty() {
        let workspace_root =
            pipeline::resolve_workspace_root(&cli.file, cli.root.clone())
                .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&cli.file, &workspace_root, &cli.set)
            .map_err(pipeline_error_to_cook_error)?;

        let recipe_infos = pipeline::build_workspace_recipe_info(&workspace);
        let registries = pipeline::build_workspace_registries(&workspace, config, &cli.set)
            .map_err(pipeline_error_to_cook_error)?;
        let inferred_deps = pipeline::compute_workspace_inferred_deps(&workspace);
        print_dep_conflicts(&pipeline::workspace_dep_conflicts(&workspace, &inferred_deps));

        pipeline::collect_dag_units(&recipe_infos, &targets, &registries, &inferred_deps)
            .map_err(pipeline_error_to_cook_error)?
    } else {
        // Single-Cookfile branch.
        let cookfile_dir = cli.file.parent().unwrap_or(std::path::Path::new("."));
        let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            cookfile_dir
        };
        let dotenv_vars = pipeline::load_env(cookfile_dir);
        let env_vars = pipeline::resolve_env(config, dotenv_vars, &cli.set)
            .map_err(pipeline_error_to_cook_error)?;

        let recipe_infos = pipeline::build_single_recipe_infos(&parsed.cookfile);
        let registries =
            pipeline::build_single_registries(cookfile_dir, env_vars, parsed.lua_source, config);
        let inferred_deps = pipeline::compute_single_inferred_deps(&parsed.cookfile);
        print_dep_conflicts(&pipeline::single_dep_conflicts(&parsed.cookfile));

        pipeline::collect_dag_units(&recipe_infos, &targets, &registries, &inferred_deps)
            .map_err(pipeline_error_to_cook_error)?
    };

    cook_dag_viewer::cmd_dag(&cook_dag_viewer::DagViewerInputs {
        target: recipe_name,
        all_units: &units.all_units,
        explicit_edges: &units.explicit_edges,
        inferred_deps: &units.inferred_deps,
        cache_managers: &units.cache_managers,
    })
    .map_err(|e| CookError::Other(e.to_string()))
}
