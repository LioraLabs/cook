//! CLI dispatch: thin wrappers around `cook_engine::pipeline` orchestration.
//!
//! Heavy lifting (parse, workspace load, registry assembly, dep inference)
//! lives in `cook_engine::pipeline`. This module owns CLI-specific glue:
//!   * mapping `menu` / `init` / `serve` / `dag` subcommands to the right
//!     engine entry point
//!   * bridging `cook_engine::EngineEvent` to `cook_progress::ProgressEvent`
//!     and wiring the renderer into a background thread
//!   * mapping `EngineError` / `PipelineError` into `CookError` for exit-code
//!     classification and human-facing diagnostics
//!
//! In particular: nothing in this file consumes a `cook_lang::ast::Cookfile`
//! directly anymore — that's the engine's concern.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc;

use cook_engine::pipeline::{self, ParsedCookfile, PipelineError, Workspace};
use cook_engine::RegisteredWorkspace;

use crate::cli::Globals;
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
        PipelineError::RecipeCollision { name, sites } => {
            // Multi-line diagnostic per spec §8: name each registration site
            // by line + kind label. The CLI's outer harness will print this
            // verbatim via `Display` and exit with code 3.
            let mut msg = format!("error: recipe '{name}' is registered more than once:\n");
            for s in &sites {
                let kind_str = match s.kind {
                    cook_engine::cook_register::RegistrationSiteKind::SurfaceRecipe => {
                        "as a `recipe` block"
                    }
                    cook_engine::cook_register::RegistrationSiteKind::SurfaceChore => {
                        "as a `chore` block"
                    }
                    cook_engine::cook_register::RegistrationSiteKind::Dynamic => {
                        "by cook.recipe at register-phase"
                    }
                };
                msg.push_str(&format!("  - Cookfile:{}: {}\n", s.line, kind_str));
            }
            msg.push_str("rename one of them.");
            CookError::RecipeCollision(msg)
        }
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
fn read_and_parse(globals: &Globals) -> Result<ParsedCookfile, CookError> {
    let parsed = pipeline::read_and_parse(&globals.file).map_err(pipeline_error_to_cook_error)?;
    for w in &parsed.warnings {
        eprintln!("cook: warning: {w}");
    }
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// EngineEvent → ProgressEvent bridge
// ---------------------------------------------------------------------------

/// Translate the engine's `NodeKind` mirror onto `cook_progress::NodeKind`.
/// The two enums are isomorphic by design — keeping them separate lets
/// `cook-engine` stay free of a `cook-progress` dependency.
fn translate_kind(k: cook_engine::NodeKind) -> cook_progress::NodeKind {
    match k {
        cook_engine::NodeKind::Compile => cook_progress::NodeKind::Compile,
        cook_engine::NodeKind::Link => cook_progress::NodeKind::Link,
        cook_engine::NodeKind::Resolve => cook_progress::NodeKind::Resolve,
        cook_engine::NodeKind::Generate => cook_progress::NodeKind::Generate,
        cook_engine::NodeKind::Write => cook_progress::NodeKind::Write,
        cook_engine::NodeKind::Test => cook_progress::NodeKind::Test,
        cook_engine::NodeKind::Cooked => cook_progress::NodeKind::Cooked,
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
                    kind,
                } => {
                    let id = intern_recipe(&name, &mut recipe_ids, &mut next_recipe);
                    cook_progress::ProgressEvent::RecipeCompleted {
                        recipe: id,
                        elapsed,
                        cached: cached_nodes,
                        total: total_nodes,
                        kind: match kind {
                            cook_engine::RecipeKind::Recipe => cook_progress::event::RecipeKind::Recipe,
                            cook_engine::RecipeKind::Chore => cook_progress::event::RecipeKind::Chore,
                        },
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
                    kind,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeStarted {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        artifact,
                        fallback_label,
                        kind: translate_kind(kind),
                    }
                }
                cook_engine::EngineEvent::NodeCompleted {
                    recipe,
                    node_name,
                    elapsed,
                    kind,
                } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::NodeCompleted {
                        recipe: rid,
                        node: nid,
                        elapsed,
                        kind: translate_kind(kind),
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
                        // CS-0049: `OutputStream` is `#[non_exhaustive]`; future
                        // variants (e.g. PTY-tagged output) default to stdout
                        // until a CS adds them to the wire enum mapping.
                        _ => Stream::Stdout,
                    };
                    cook_progress::ProgressEvent::NodeOutput {
                        recipe: rid,
                        node: nid,
                        line,
                        stream,
                    }
                }
                cook_engine::EngineEvent::InteractiveStart { recipe, node_name, chore_step_count } => {
                    let rid = intern_recipe(&recipe, &mut recipe_ids, &mut next_recipe);
                    let nid = intern_node(&recipe, &node_name, &mut node_ids, &mut next_node);
                    cook_progress::ProgressEvent::InteractiveStart {
                        recipe: rid,
                        node: nid,
                        name: node_name,
                        chore_step_count,
                    }
                }
                cook_engine::EngineEvent::InteractiveEnd {
                    recipe,
                    node_name,
                    elapsed,
                    success,
                    is_terminal,
                    failed_step,
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
                        failed_step,
                    }
                }
                cook_engine::EngineEvent::Finished { success, .. } => {
                    cook_progress::ProgressEvent::Finished { success }
                }
                // Phase 4 will wire real progress handlers for test events.
                cook_engine::EngineEvent::TestStarted { .. }
                | cook_engine::EngineEvent::TestPassed { .. }
                | cook_engine::EngineEvent::TestFailed { .. }
                | cook_engine::EngineEvent::TestBlocked { .. }
                | cook_engine::EngineEvent::TestTimedOut { .. } => continue,
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
///
/// Walks the unified work-unit DAG built from `registered_workspace`. The
/// recipe-level edge map (and therefore the reachable closure) is computed
/// here from `recipe_infos` via `analyzer::dependency_edges_multi`; the
/// engine takes responsibility for the topological order downstream.
fn run_with_progress(
    globals: &Globals,
    recipe_infos: &BTreeMap<String, cook_engine::analyzer::RecipeInfo>,
    targets: &[String],
    registered_workspace: &RegisteredWorkspace,
    num_jobs: usize,
) -> Result<cook_engine::run::RunResult, CookError> {
    let project_root = std::env::current_dir().map_err(|e| CookError::Other(e.to_string()))?;

    // Recipe-level dependency edges across the reachable closure. The engine
    // toposorts internally; we just need a complete edge map keyed by every
    // reachable recipe name.
    let edges = cook_engine::analyzer::dependency_edges_multi(recipe_infos, targets).map_err(
        |e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                CookError::RecipeNotFound(name)
            }
            other => CookError::Other(other.to_string()),
        },
    )?;
    let reachable: BTreeSet<String> = edges.keys().cloned().collect();

    let (progress_tx, progress_rx) = mpsc::channel::<cook_progress::ProgressEvent>();
    let render_thread = spawn_new_renderer(globals, project_root.clone(), progress_rx);

    let bridge_tx = progress_tx.clone();
    let (engine_tx, engine_rx) = mpsc::channel::<cook_engine::EngineEvent>();
    let bridge_thread = bridge_engine_to_progress_events(engine_rx, bridge_tx);

    let result = cook_engine::run::run(
        &project_root,
        registered_workspace,
        &edges,
        &reachable,
        num_jobs,
        &[],
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

/// Resolve num_jobs from globals or system parallelism.
fn resolve_num_jobs(globals: &Globals) -> usize {
    globals.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    })
}

// ---------------------------------------------------------------------------
// cmd_run
// ---------------------------------------------------------------------------

pub fn cmd_run(
    globals: &Globals,
    recipe_name: &str,
    argv: &[String],
    config: Option<&str>,
) -> Result<(), CookError> {
    let parsed = read_and_parse(globals)?;
    pipeline::validate_selected_config(&parsed.cookfile, config)
        .map_err(pipeline_error_to_cook_error)?;

    let num_jobs = resolve_num_jobs(globals);
    let targets = vec![recipe_name.to_string()];

    // Register every Cookfile in the workspace (root + imports) up front.
    // The resulting `RegisteredWorkspace` carries every recipe — including
    // Lua-registered ones from `cook.add_unit` / module helpers like
    // `cook_cc.bin` — under their qualified names, with `RecipeUnits` and
    // `dep_edges` already wired. Phase 5 Task 5.3: this replaces the prior
    // per-wave register loop. `cache_ctx` lifting is deferred to a follow-up;
    // for now register sees `None` and the executor builds its own.
    let registered = if !parsed.cookfile.imports.is_empty() {
        let workspace_root = pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
            .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::register_workspace_with_argv(
            &workspace,
            config,
            &globals.set,
            recipe_name,
            argv,
            /*cache_ctx*/ None,
        )
        .map_err(pipeline_error_to_cook_error)?
    } else {
        let cookfile_dir = globals.file.parent().unwrap_or(std::path::Path::new("."));
        let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            cookfile_dir
        };
        let dotenv_vars = pipeline::load_env(cookfile_dir);
        let env_vars = pipeline::resolve_env(config, dotenv_vars, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::register_single_cookfile_with_argv(
            cookfile_dir,
            env_vars,
            &globals.set,
            parsed.lua_source,
            config,
            recipe_name,
            argv,
            /*cache_ctx*/ None,
        )
        .map_err(pipeline_error_to_cook_error)?
    };

    // `inferred_deps` / `*_dep_conflicts` are obsolete in the unified-DAG
    // model: cross-recipe edges come from `RecipeUnits.dep_edges` (recorded
    // directly by `cook.dep_output` / `cook.add_unit` during the register
    // pass), and recipe-level coarse deps come from `RegisteredRecipePub.requires`.
    let recipe_infos = pipeline::build_recipe_infos_from_registered(&registered);

    run_with_progress(globals, &recipe_infos, &targets, &registered, num_jobs)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_emit_lua
// ---------------------------------------------------------------------------

pub fn cmd_emit_lua(globals: &Globals) -> Result<(), CookError> {
    let parsed = read_and_parse(globals)?;
    println!("{}", parsed.lua_source);
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_test
// ---------------------------------------------------------------------------

pub fn cmd_test(
    globals: &Globals,
    args: &crate::cli::TestArgs,
) -> Result<(), crate::error::CookError> {
    use cook_engine::TestScope;
    use std::sync::{Arc, Mutex};

    let project_root = std::env::current_dir()
        .map_err(|e| crate::error::CookError::Other(e.to_string()))?;

    // Determine scope from positional `scope` argument.
    //
    // To distinguish a recipe name from a namespace prefix we use the cheap
    // `list_workspace_names` path: it loads each Cookfile, runs register-phase
    // Lua, and returns just the qualified names + kinds — no recipe bodies
    // run, no probe queries fire. This sees Lua-registered recipes (e.g.
    // `cook_cc.bin`) the same way `cook list` does. We tolerate listing
    // failures (fall back to the raw arg as a Recipe scope) so a malformed
    // Cookfile still gets a single, recognisable error from the engine
    // instead of a duplicated diagnostic from the CLI.
    let scope: Option<TestScope> = match args.scope.as_deref() {
        None => None,
        Some(name) => {
            let recipe_names = collect_workspace_recipe_names(globals).unwrap_or_default();
            Some(resolve_test_scope(name, &recipe_names)?)
        }
    };

    // Phase 7 stub for --rerun-failed
    let rerun_failed_set = if args.rerun_failed {
        match crate::test_state::load_failed_set(&project_root) {
            Ok(set) if !set.is_empty() => Some(set),
            Ok(_) => {
                eprintln!("cook: warning: no previously-failed tests recorded");
                eprintln!("cook: hint: run `cook test` first to populate state");
                return Ok(());
            }
            Err(e) => {
                eprintln!("cook: warning: {e}");
                eprintln!("cook: hint: run `cook test` first to populate state");
                return Ok(());
            }
        }
    } else {
        None
    };

    let rerun_patterns: Vec<String> = args.rerun.clone().unwrap_or_default();
    let num_jobs = resolve_num_jobs(globals);

    // ── Register the workspace ────────────────────────────────────────────────
    // Same path as `cmd_run`: build a unified `RegisteredWorkspace` covering
    // every Cookfile (root + imports), then derive recipe_infos. This sees
    // Lua-registered recipes (cook_cc.bin, dynamic chores, …) under their
    // qualified names with `RecipeUnits` and `dep_edges` already wired.
    let parsed = read_and_parse(globals)?;
    let registered = if !parsed.cookfile.imports.is_empty() {
        let workspace_root = pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
            .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::register_workspace(&workspace, None, &globals.set, /*cache_ctx*/ None)
            .map_err(pipeline_error_to_cook_error)?
    } else {
        let cookfile_dir = globals.file.parent().unwrap_or(std::path::Path::new("."));
        let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            cookfile_dir
        };
        let dotenv_vars = pipeline::load_env(cookfile_dir);
        let env_vars = pipeline::resolve_env(None, dotenv_vars, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::register_single_cookfile(
            cookfile_dir,
            env_vars,
            &globals.set,
            parsed.lua_source,
            None,
            /*cache_ctx*/ None,
        )
        .map_err(pipeline_error_to_cook_error)?
    };
    let recipe_infos = pipeline::build_recipe_infos_from_registered(&registered);

    // Chore names — chores are excluded from `cook test` because they are
    // destructive by design (e.g. `cook clean` deletes build artefacts) and
    // have no test steps. Including them would cause unintended side-effects.
    let chore_names: std::collections::BTreeSet<String> = registered
        .names
        .iter()
        .filter(|n| matches!(n.kind, cook_engine::cook_register::RecipeKind::Chore))
        .map(|n| n.name.clone())
        .collect();

    // ── Determine candidate recipe names from scope ──────────────────────────
    let candidate_recipe_names: Vec<String> = match &scope {
        None => recipe_infos
            .keys()
            .filter(|n| !chore_names.contains(*n))
            .cloned()
            .collect(),
        Some(TestScope::Recipe(name)) => {
            cook_engine::analyzer::dependency_edges(&recipe_infos, name)
                .map_err(|e| match e {
                    cook_engine::analyzer::GraphError::CycleDetected(s) => {
                        crate::error::CookError::Other(format!("dependency cycle involving: {s}"))
                    }
                    cook_engine::analyzer::GraphError::UnknownRecipe(s) => {
                        crate::error::CookError::RecipeNotFound(s)
                    }
                    other => crate::error::CookError::Other(other.to_string()),
                })?
                .keys()
                .filter(|n| !chore_names.contains(*n))
                .cloned()
                .collect()
        }
        Some(TestScope::Namespace(ns)) => {
            let prefix = format!("{ns}.");
            recipe_infos
                .keys()
                .filter(|n| !chore_names.contains(*n))
                .filter(|n| n.starts_with(&prefix) || *n == ns)
                .cloned()
                .collect()
        }
    };

    // ── Recipe-level pre-filter by --filter glob ─────────────────────────────
    // When filter_patterns are present, limit the target recipe set to those
    // whose recipe name could plausibly match the glob. The glob pattern uses
    // `<recipe>:<test_name>` format; we match the recipe portion by checking
    // if any pattern matches `<recipe>:*`. This avoids running unrelated
    // recipes whose build steps may fail hard when we only care about specific
    // tests. Post-execution, we still apply the full per-TestId filter to
    // handle the test_name portion.
    let candidate_recipe_names: Vec<String> = if !args.filter.is_empty() {
        candidate_recipe_names
            .into_iter()
            .filter(|recipe_name| {
                args.filter.iter().any(|pat| {
                    let recipe_pat = if let Some(colon_pos) = pat.find(':') {
                        pat[..colon_pos].to_string()
                    } else {
                        pat.clone()
                    };
                    let wildcard_id_for_recipe = format!("{}:", recipe_name);
                    let full_match = globset::Glob::new(pat)
                        .map(|g| g.compile_matcher().is_match(&wildcard_id_for_recipe))
                        .unwrap_or(false);
                    let recipe_match = globset::Glob::new(&recipe_pat)
                        .map(|g| g.compile_matcher().is_match(recipe_name.as_str()))
                        .unwrap_or(false);
                    full_match || recipe_match
                })
            })
            .collect()
    } else {
        candidate_recipe_names
    };

    let reporter = Arc::new(Mutex::new(crate::test_reporter::Reporter::new(globals)));

    // ── Drive the unified-DAG executor ───────────────────────────────────────
    // The `on_event` closure clones the reporter Arc. It MUST be dropped
    // before we reclaim the inner reporter via `Arc::try_unwrap` below; the
    // simplest way to guarantee that is to scope the closure inside the
    // run-or-skip branch so it falls out of scope by the end of the
    // expression.
    let test_results: Vec<cook_engine::TestResult> = if candidate_recipe_names.is_empty() {
        // Nothing in the candidate set (e.g. `--filter` matched no recipe).
        // Skip the executor and return an empty result. The reporter still
        // gets `finish` called below with an empty slice.
        Vec::new()
    } else {
        let edges = cook_engine::analyzer::dependency_edges_multi(
            &recipe_infos,
            &candidate_recipe_names,
        )
        .map_err(|e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                crate::error::CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                crate::error::CookError::RecipeNotFound(name)
            }
            other => crate::error::CookError::Other(other.to_string()),
        })?;
        let reachable: std::collections::BTreeSet<String> = edges.keys().cloned().collect();

        let reporter_for_cb = reporter.clone();
        let on_event = move |evt: cook_engine::EngineEvent| {
            if let Ok(mut r) = reporter_for_cb.lock() {
                r.on_event(evt);
            }
        };

        // In test mode, a cook-step failure should not short-circuit with
        // EngineError::TaskFailures. The executor's cancel_subtree already
        // pushed Blocked TestResult rows for every downstream test node into
        // `partial_test_results`; carry them through so we return Ok with the
        // Blocked results rather than propagating the error.
        match cook_engine::run::run(
            &project_root,
            &registered,
            &edges,
            &reachable,
            num_jobs,
            &rerun_patterns,
            on_event,
        ) {
            Ok(r) => r.test_results,
            Err(cook_engine::EngineError::TaskFailures {
                partial_test_results,
                ..
            }) => partial_test_results,
            Err(other) => return Err(engine_error_to_cook_error(other)),
        }
    };

    // SAFETY: the `on_event` closure (if any) was moved into run() above and
    // has been dropped by now; no other Arc references remain.
    let mut reporter = Arc::try_unwrap(reporter)
        .unwrap_or_else(|_| panic!("reporter Arc still has other references after run returned"))
        .into_inner()
        .expect("reporter Mutex is poisoned");

    // Post-execution: filter test_results by --filter globs and --rerun-failed set.
    let test_results: Vec<cook_engine::TestResult> = test_results
        .into_iter()
        .filter(|r| {
            let id_matches = if args.filter.is_empty() {
                true
            } else {
                args.filter.iter().any(|pat| {
                    globset::Glob::new(pat)
                        .map(|g| g.compile_matcher().is_match(&r.id.0))
                        .unwrap_or(false)
                })
            };
            let rerun_matches = if let Some(failed_set) = rerun_failed_set.as_ref() {
                failed_set.contains(&r.id)
            } else {
                true
            };
            id_matches && rerun_matches
        })
        .collect();

    // Phase 7 stub: persist last-run state (no-op)
    let _ = crate::test_state::save(&project_root, &test_results);

    // Phase 8: write JSON/JUnit sidecars.
    let _ = crate::test_reporter::write_json_sidecar(
        &project_root,
        args.report_json.as_deref(),
        &test_results,
    );
    if let Some(path) = &args.report_junit {
        let _ = crate::test_reporter::write_junit_sidecar(path, &test_results);
    }

    let any_failed = test_results.iter().any(|r| {
        matches!(
            r.outcome,
            cook_engine::TestOutcome::Failed
                | cook_engine::TestOutcome::Blocked
                | cook_engine::TestOutcome::TimedOut
        )
    });

    reporter.finish(&test_results);

    if any_failed {
        Err(crate::error::CookError::TestFailure(
            "one or more tests failed".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// Resolve a `cook test <scope>` argument against the known recipe set.
///
/// Resolution order:
///   1. Exact match against a fully-qualified recipe name → `TestScope::Recipe`.
///   2. Otherwise, if any recipe name starts with `<scope>.` → `TestScope::Namespace`.
///   3. Otherwise, return a useful diagnostic that mentions both options and
///      the `--filter` escape hatch.
///
/// The recipe set passed in is the engine's view: dotted, fully-qualified
/// names (e.g. `apps.web.build`). For an empty recipe set (e.g. workspace
/// failed to load) the function still treats the arg as a Recipe so the
/// engine's "unknown recipe" path produces the canonical error.
fn resolve_test_scope(
    name: &str,
    recipe_names: &std::collections::BTreeSet<String>,
) -> Result<cook_engine::TestScope, crate::error::CookError> {
    use cook_engine::TestScope;

    // Empty set → defer to the engine, which has the authoritative diagnostic.
    if recipe_names.is_empty() {
        return Ok(TestScope::Recipe(name.to_string()));
    }

    // 1. Recipe match wins (preserves existing behaviour for `sub.pass` etc.)
    if recipe_names.contains(name) {
        return Ok(TestScope::Recipe(name.to_string()));
    }

    // 2. Namespace match: any recipe under `<name>.`
    let ns_prefix = format!("{name}.");
    if recipe_names.iter().any(|r| r.starts_with(&ns_prefix)) {
        return Ok(TestScope::Namespace(name.to_string()));
    }

    // 3. Neither — produce a diagnostic that explains the two valid forms
    // and points at `--filter` for glob-shaped arguments.
    let mut suggestions: Vec<String> = recipe_names
        .iter()
        .filter(|r| r.starts_with(name) || r.contains(name))
        .take(5)
        .cloned()
        .collect();
    suggestions.sort();
    suggestions.dedup();

    let mut msg = format!(
        "unknown test scope: '{name}'\n\
         hint: scope must be a recipe name (e.g. `cook test apps.web.build`)\n\
         hint: or an import-namespace prefix (e.g. `cook test apps.web`)\n\
         hint: for glob patterns use --filter (e.g. `cook test --filter '{name}.*'`)"
    );
    if !suggestions.is_empty() {
        msg.push_str("\nsimilar recipes:");
        for s in &suggestions {
            msg.push_str("\n  - ");
            msg.push_str(s);
        }
    }
    Err(crate::error::CookError::Other(msg))
}

/// Return the set of fully-qualified recipe names (recipes only, not chores —
/// chores are filtered from `cook test` anyway) using the cheap
/// `list_names` path: register-phase Lua runs but no recipe bodies execute
/// and no probe queries fire.
///
/// Returns `None` when the workspace cannot be loaded so the caller can fall
/// back to deferring to the engine's diagnostic path.
///
/// Lua-registered recipes (e.g. via `cook_cc.bin`) appear here the same way
/// they do in `cook list` — register-phase is enough to materialise their
/// names.
fn collect_workspace_recipe_names(
    globals: &Globals,
) -> Option<std::collections::BTreeSet<String>> {
    let parsed = pipeline::read_and_parse(&globals.file).ok()?;
    let names: Vec<(String, cook_engine::cook_register::RecipeKind)> =
        if parsed.cookfile.imports.is_empty() {
            let cookfile_dir = globals.file.parent().unwrap_or(std::path::Path::new("."));
            let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
                std::path::Path::new(".")
            } else {
                cookfile_dir
            };
            let dotenv_vars = pipeline::load_env(cookfile_dir);
            let env_vars = pipeline::resolve_env(None, dotenv_vars, &globals.set).ok()?;
            pipeline::list_single_cookfile_names(
                cookfile_dir,
                env_vars,
                &globals.set,
                parsed.lua_source,
                None,
            )
            .ok()?
        } else {
            let workspace_root =
                pipeline::resolve_workspace_root(&globals.file, globals.root.clone()).ok()?;
            let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set).ok()?;
            pipeline::list_workspace_names(&workspace, /*config*/ None, &globals.set).ok()?
        };
    Some(
        names
            .into_iter()
            .filter(|(_, kind)| matches!(kind, cook_engine::cook_register::RecipeKind::Recipe))
            .map(|(name, _)| name)
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// cmd_menu
// ---------------------------------------------------------------------------

/// Print recipe and chore names, one per kind-prefixed line.
///
/// Backed by the cheap `cook_register::list_names` path through the
/// pipeline-layer `list_workspace_names` / `list_single_cookfile_names`
/// helpers, so Lua-registered recipes (e.g. `cook_cc.bin`) appear in the
/// menu alongside surface `recipe NAME` blocks — no recipe body runs and
/// no probes fire. The previous AST-walk only saw `parsed.cookfile.recipes`
/// / `parsed.cookfile.chores`, missing every dynamically-registered
/// recipe.
pub fn cmd_menu(globals: &Globals) -> Result<(), CookError> {
    let parsed = read_and_parse(globals)?;

    let names = if !parsed.cookfile.imports.is_empty() {
        let workspace_root =
            pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
                .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::list_workspace_names(&workspace, /*config*/ None, &globals.set)
            .map_err(pipeline_error_to_cook_error)?
    } else {
        let cookfile_dir = globals.file.parent().unwrap_or(std::path::Path::new("."));
        let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            cookfile_dir
        };
        let dotenv_vars = pipeline::load_env(cookfile_dir);
        let env_vars = pipeline::resolve_env(None, dotenv_vars, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::list_single_cookfile_names(
            cookfile_dir,
            env_vars,
            &globals.set,
            parsed.lua_source,
            None,
        )
        .map_err(pipeline_error_to_cook_error)?
    };

    for (name, kind) in &names {
        match kind {
            cook_engine::cook_register::RecipeKind::Recipe => println!("  recipe {name}"),
            cook_engine::cook_register::RecipeKind::Chore => println!("  chore  {name}"),
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_list
// ---------------------------------------------------------------------------

/// Print recipe and chore names, one per line, with no decoration.
///
/// This is the machine-readable counterpart of `cmd_menu`: each line is
/// exactly the name a user would pass back to `cook` (qualified with the
/// import alias for workspace imports). Designed for shell pipelines such
/// as `cook list | fzf | xargs -r cook`.
///
/// Backed by the cheap `cook_register::list_names` path through the
/// pipeline-layer `list_workspace_names` / `list_single_cookfile_names`
/// helpers, so Lua-registered recipes (e.g. `cook_cc.bin`) appear in the
/// listing without invoking any recipe body or firing probe queries.
///
/// `--recipes-only` and `--chores-only` filter the output. They are
/// mutually exclusive at the clap layer; this function trusts that.
pub fn cmd_list(globals: &Globals, args: &crate::cli::ListArgs) -> Result<(), CookError> {
    // Defensive: clap enforces this with `conflicts_with`, but if a future
    // refactor changes the dispatch the error here is still meaningful.
    if args.recipes_only && args.chores_only {
        return Err(CookError::Other(
            "--recipes-only and --chores-only are mutually exclusive".to_string(),
        ));
    }

    let parsed = read_and_parse(globals)?;

    let want_recipes = !args.chores_only;
    let want_chores = !args.recipes_only;

    let names = if !parsed.cookfile.imports.is_empty() {
        let workspace_root =
            pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
                .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::list_workspace_names(&workspace, /*config*/ None, &globals.set)
            .map_err(pipeline_error_to_cook_error)?
    } else {
        let cookfile_dir = globals.file.parent().unwrap_or(std::path::Path::new("."));
        let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            cookfile_dir
        };
        let dotenv_vars = pipeline::load_env(cookfile_dir);
        let env_vars = pipeline::resolve_env(None, dotenv_vars, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::list_single_cookfile_names(
            cookfile_dir,
            env_vars,
            &globals.set,
            parsed.lua_source,
            None,
        )
        .map_err(pipeline_error_to_cook_error)?
    };

    for (name, kind) in names {
        let is_chore = matches!(kind, cook_engine::cook_register::RecipeKind::Chore);
        if (is_chore && want_chores) || (!is_chore && want_recipes) {
            println!("{name}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_init
// ---------------------------------------------------------------------------

pub fn cmd_init() -> Result<(), CookError> {
    let cookfile_path = std::path::Path::new("Cookfile");
    if cookfile_path.exists() {
        return Err(CookError::Other("Cookfile already exists".to_string()));
    }
    // CS-0019 dropped `end`: recipe bodies are indented and terminated by the
    // next column-0 keyword or EOF. Emitting `end` here was a v0.3-era
    // template; under the current grammar that line parses as a literal
    // shell command and the build fails with exit 127.
    std::fs::write(
        cookfile_path,
        r#"recipe build
    echo "Hello from Cook!"
"#,
    )
    .map_err(|e| CookError::Other(format!("failed to write Cookfile: {e}")))?;
    println!("Created Cookfile");

    let gitignore_path = std::path::Path::new(".gitignore");
    let existing = match std::fs::read_to_string(gitignore_path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(CookError::Other(format!(
                "failed to read .gitignore: {e}"
            )));
        }
    };
    match merge_cook_gitignore_section(existing.as_deref()) {
        GitignoreMerge::Unchanged => {
            println!(".gitignore already has Cook entries");
        }
        GitignoreMerge::Created(content) => {
            std::fs::write(gitignore_path, content)
                .map_err(|e| CookError::Other(format!("failed to write .gitignore: {e}")))?;
            println!("Created .gitignore");
        }
        GitignoreMerge::Appended(content) => {
            std::fs::write(gitignore_path, content)
                .map_err(|e| CookError::Other(format!("failed to write .gitignore: {e}")))?;
            println!("Updated .gitignore with Cook entries");
        }
    }
    Ok(())
}

/// Marker line that identifies a Cook-managed `.gitignore` section. Used to
/// keep `cook init` idempotent across re-runs.
const COOK_GITIGNORE_MARKER: &str = "# Cook artifacts (added by cook init)";

/// The Cook-managed `.gitignore` block. Only entries that are unambiguously
/// Cook-specific go here — language/toolchain ignores (target/, node_modules/)
/// are the user's call.
const COOK_GITIGNORE_SECTION: &str = "\
# Cook artifacts (added by cook init)
# .cook/ holds caches and per-project state; cloud.toml is the one tracked
.cook/**
**/.cook/**
!**/.cook/
!**/.cook/cloud.toml
# Project-local luarocks tree populated by `cook modules install`. Pinned by
# cook.lock + the registry, so it's build output, not source. Top-level
# user-authored lua files in cook_modules/ stay tracked.
cook_modules/lib/
";

#[derive(Debug, PartialEq, Eq)]
enum GitignoreMerge {
    Unchanged,
    Created(String),
    Appended(String),
}

/// Pure helper: given the current contents of `.gitignore` (or `None` if
/// missing), decide what the file should look like after `cook init`. The
/// result is a [`GitignoreMerge`] the caller can act on.
fn merge_cook_gitignore_section(existing: Option<&str>) -> GitignoreMerge {
    match existing {
        None => GitignoreMerge::Created(COOK_GITIGNORE_SECTION.to_string()),
        Some(s) if s.contains(COOK_GITIGNORE_MARKER) => GitignoreMerge::Unchanged,
        Some(s) => {
            let mut out = String::with_capacity(s.len() + COOK_GITIGNORE_SECTION.len() + 2);
            out.push_str(s);
            if !s.is_empty() && !s.ends_with('\n') {
                out.push('\n');
            }
            if !s.is_empty() {
                out.push('\n');
            }
            out.push_str(COOK_GITIGNORE_SECTION);
            GitignoreMerge::Appended(out)
        }
    }
}

#[cfg(test)]
mod cmd_init_tests {
    use super::*;

    #[test]
    fn merge_creates_section_when_no_gitignore() {
        let merged = merge_cook_gitignore_section(None);
        match merged {
            GitignoreMerge::Created(content) => {
                assert!(content.contains(COOK_GITIGNORE_MARKER));
                assert!(content.contains("cook_modules/lib/"));
                assert!(content.contains(".cook/**"));
                assert!(content.ends_with('\n'));
                // Guard against drift: the comment must reference the
                // current subcommand name, not the renamed-and-removed
                // `cook modules add`.
                assert!(content.contains("cook modules install"));
                assert!(!content.contains("cook modules add"));
            }
            other => panic!("expected Created, got {other:?}"),
        }
    }

    #[test]
    fn merge_is_idempotent_when_marker_present() {
        let existing = format!("target/\n\n{COOK_GITIGNORE_SECTION}");
        assert_eq!(
            merge_cook_gitignore_section(Some(&existing)),
            GitignoreMerge::Unchanged,
        );
    }

    #[test]
    fn merge_appends_with_blank_line_separator() {
        let existing = "target/\nnode_modules/\n";
        match merge_cook_gitignore_section(Some(existing)) {
            GitignoreMerge::Appended(content) => {
                assert!(content.starts_with("target/\nnode_modules/\n\n"));
                assert!(content.contains(COOK_GITIGNORE_MARKER));
                assert!(content.contains("cook_modules/lib/"));
            }
            other => panic!("expected Appended, got {other:?}"),
        }
    }

    #[test]
    fn merge_normalizes_missing_trailing_newline_before_appending() {
        let existing = "target/";
        match merge_cook_gitignore_section(Some(existing)) {
            GitignoreMerge::Appended(content) => {
                assert!(content.starts_with("target/\n\n"));
                assert!(content.contains(COOK_GITIGNORE_MARKER));
            }
            other => panic!("expected Appended, got {other:?}"),
        }
    }

    #[test]
    fn merge_treats_empty_file_like_creation() {
        match merge_cook_gitignore_section(Some("")) {
            GitignoreMerge::Appended(content) => {
                assert!(content.starts_with(COOK_GITIGNORE_MARKER));
            }
            other => panic!("expected Appended, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod resolve_test_scope_tests {
    use super::*;
    use cook_engine::TestScope;
    use std::collections::BTreeSet;

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_recipe_set_defers_to_engine_as_recipe() {
        // When the workspace can't be loaded we treat the arg as a recipe so
        // the engine's canonical "unknown recipe" diagnostic surfaces.
        let scope = resolve_test_scope("anything", &BTreeSet::new()).unwrap();
        match scope {
            TestScope::Recipe(n) => assert_eq!(n, "anything"),
            other => panic!("expected Recipe, got {other:?}"),
        }
    }

    #[test]
    fn exact_recipe_match_returns_recipe() {
        let set = names(&["build", "sub.pass", "sub.fail_one"]);
        let scope = resolve_test_scope("sub.pass", &set).unwrap();
        match scope {
            TestScope::Recipe(n) => assert_eq!(n, "sub.pass"),
            other => panic!("expected Recipe, got {other:?}"),
        }
    }

    #[test]
    fn bare_recipe_match_returns_recipe() {
        let set = names(&["build", "sub.pass"]);
        let scope = resolve_test_scope("build", &set).unwrap();
        match scope {
            TestScope::Recipe(n) => assert_eq!(n, "build"),
            other => panic!("expected Recipe, got {other:?}"),
        }
    }

    #[test]
    fn single_segment_namespace_match_returns_namespace() {
        // Reproduction case from the bug report: `cook test web` with
        // `web.build` defined under `import web ./web` MUST resolve as
        // a Namespace, not a (failing) Recipe lookup.
        let set = names(&["build", "web.build", "web.test"]);
        let scope = resolve_test_scope("web", &set).unwrap();
        match scope {
            TestScope::Namespace(n) => assert_eq!(n, "web"),
            other => panic!("expected Namespace, got {other:?}"),
        }
    }

    #[test]
    fn nested_namespace_match_returns_namespace() {
        let set = names(&["apps.web.build", "apps.web.unit", "apps.api.build"]);
        let scope = resolve_test_scope("apps.web", &set).unwrap();
        match scope {
            TestScope::Namespace(n) => assert_eq!(n, "apps.web"),
            other => panic!("expected Namespace, got {other:?}"),
        }
    }

    #[test]
    fn recipe_match_wins_over_namespace_match() {
        // If both a recipe `foo` and recipes `foo.bar` exist (which can happen
        // with deeply-nested imports), prefer the exact recipe match.
        let set = names(&["foo", "foo.bar", "foo.baz"]);
        let scope = resolve_test_scope("foo", &set).unwrap();
        match scope {
            TestScope::Recipe(n) => assert_eq!(n, "foo"),
            other => panic!("expected Recipe (exact match wins), got {other:?}"),
        }
    }

    #[test]
    fn unknown_scope_errors_with_useful_diagnostic() {
        let set = names(&["build", "web.build", "web.test"]);
        let err = resolve_test_scope("xyz", &set).expect_err("unknown scope must error");
        let msg = format!("{err}");
        assert!(msg.contains("unknown test scope: 'xyz'"), "message: {msg}");
        assert!(msg.contains("recipe name"), "message: {msg}");
        assert!(msg.contains("namespace"), "message: {msg}");
        assert!(msg.contains("--filter"), "message: {msg}");
    }

    #[test]
    fn unknown_scope_does_not_swallow_partial_namespace_typo() {
        // `webs` doesn't match the recipe `web.build` exactly nor the
        // namespace `webs.` — we must error rather than silently widening.
        let set = names(&["web.build", "web.test"]);
        let err = resolve_test_scope("webs", &set).expect_err("typo must error");
        let msg = format!("{err}");
        assert!(msg.contains("unknown test scope: 'webs'"), "message: {msg}");
    }
}

// ---------------------------------------------------------------------------
// cmd_serve
// ---------------------------------------------------------------------------

pub fn cmd_serve(
    globals: &Globals,
    recipe_name: &str,
    config: Option<&str>,
) -> Result<(), CookError> {
    let parsed = read_and_parse(globals)?;
    pipeline::validate_selected_config(&parsed.cookfile, config)
        .map_err(pipeline_error_to_cook_error)?;

    // Check for interactive steps -- not supported under cook serve
    for recipe in &parsed.cookfile.recipes {
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

    // Resolve execution order via engine analyzer for glob collection. The
    // analyzer's `recipe_infos` map now comes from the unified register
    // pass: we register the single root Cookfile (cmd_serve doesn't support
    // workspace imports for glob collection — imports only contribute file
    // paths to watch below) and derive `RecipeInfo` from it.
    let cookfile_dir = globals.file.parent().unwrap_or(std::path::Path::new("."));
    let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        cookfile_dir
    };
    let serve_dotenv = pipeline::load_env(cookfile_dir);
    let serve_env = pipeline::resolve_env(config, serve_dotenv, &globals.set)
        .map_err(pipeline_error_to_cook_error)?;
    let serve_registered = pipeline::register_single_cookfile(
        cookfile_dir,
        serve_env,
        &globals.set,
        parsed.lua_source.clone(),
        config,
        /*cache_ctx*/ None,
    )
    .map_err(pipeline_error_to_cook_error)?;
    let recipe_infos = pipeline::build_recipe_infos_from_registered(&serve_registered);
    let order =
        cook_engine::analyzer::topological_sort(&recipe_infos, recipe_name).map_err(|e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                CookError::RecipeNotFound(name)
            }
            // Io/Parse cannot be produced by topological_sort (pure graph op).
            e => CookError::Other(e.to_string()),
        })?;

    let globs = CookWatcher::collect_globs_for_recipes(&parsed.cookfile, &order);
    if globs.is_empty() {
        return Err(CookError::Other(
            "nothing to watch: no recipes in the chain have ingredients".to_string(),
        ));
    }

    let cookfile_path = std::fs::canonicalize(&globals.file)
        .map_err(|e| CookError::Other(format!("cannot resolve Cookfile path: {e}")))?;

    let mut cookfile_paths = vec![cookfile_path];

    // If imports exist, collect all imported Cookfile paths for watching
    if !parsed.cookfile.imports.is_empty() {
        let workspace_root =
            pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
                .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        for (_canonical_path, loaded) in &workspace.imports {
            let import_cookfile = loaded.dir.join("Cookfile");
            if let Ok(canonical) = std::fs::canonicalize(&import_cookfile) {
                cookfile_paths.push(canonical);
            }
        }
    }

    let watcher = CookWatcher::new(globs, cookfile_paths);

    eprintln!("cook serve: initial build...");
    let _ = cmd_run(globals, recipe_name, &[], config);

    eprintln!("cook serve: watching for changes...");
    watcher
        .watch(|cookfile_changed| {
            if cookfile_changed {
                eprintln!("cook serve: Cookfile changed, rebuilding...");
            }
            cmd_run(globals, recipe_name, &[], config).map_err(|e| e.to_string())?;
            Ok(())
        })
        .map_err(|e| CookError::Other(e.to_string()))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_dag — feature-gated
// ---------------------------------------------------------------------------
//
// The DAG viewer (`cook dag`) lives in the `cook-dag-viewer` crate and is
// pulled in only when the `viewer` cargo feature is enabled (see
// `Cargo.toml`). When the feature is off, `cmd_dag` short-circuits with a
// helpful error so users learn which build flag they need. The reference-
// implementation policy is documented in the Cook Standard at
// `standard/src/content/docs/appendix/D-changes.mdx#changes-cs-0047`.

#[cfg(not(feature = "viewer"))]
pub fn cmd_dag(_globals: &Globals, _args: &crate::cli::DagArgs) -> Result<(), CookError> {
    Err(CookError::Other(
        "the `cook dag` viewer is not built into this binary; rebuild with \
         `cargo build --features viewer` (or pass `--features viewer` when \
         running `cargo install`)"
            .to_string(),
    ))
}

#[cfg(feature = "viewer")]
pub fn cmd_dag(globals: &Globals, args: &crate::cli::DagArgs) -> Result<(), CookError> {
    use std::sync::Arc;

    let parsed = read_and_parse(globals)?;
    let recipe_name = args.recipe.as_deref().unwrap_or("build");
    let config = args.config.as_deref();
    pipeline::validate_selected_config(&parsed.cookfile, config)
        .map_err(pipeline_error_to_cook_error)?;

    let targets = vec![recipe_name.to_string()];

    // SHI-222 Phase 5 Task 5.5: cmd_dag now drives the same register pipeline
    // as cmd_run/cmd_test. The unified `RegisteredWorkspace` carries every
    // reachable recipe — including Lua-registered ones (`cook_cc.bin`, dynamic
    // chores, …) — with `RecipeUnits` already wired. The viewer's
    // `all_units` is the reachable slice of `registered.units_by_recipe`;
    // `explicit_edges` is the recipe-level edge map; `inferred_deps` is empty
    // in the unified-DAG world (cross-recipe edges now live on `dep_edges`
    // inside each `RecipeUnits`, not on a separate inferred-dep map).
    let registered = if !parsed.cookfile.imports.is_empty() {
        let workspace_root =
            pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
                .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::register_workspace(&workspace, config, &globals.set, /*cache_ctx*/ None)
            .map_err(pipeline_error_to_cook_error)?
    } else {
        let cookfile_dir = globals.file.parent().unwrap_or(std::path::Path::new("."));
        let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            cookfile_dir
        };
        let dotenv_vars = pipeline::load_env(cookfile_dir);
        let env_vars = pipeline::resolve_env(config, dotenv_vars, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::register_single_cookfile(
            cookfile_dir,
            env_vars,
            &globals.set,
            parsed.lua_source,
            config,
            /*cache_ctx*/ None,
        )
        .map_err(pipeline_error_to_cook_error)?
    };

    let recipe_infos = pipeline::build_recipe_infos_from_registered(&registered);
    let edges = cook_engine::analyzer::dependency_edges_multi(&recipe_infos, &targets).map_err(
        |e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(name) => {
                CookError::RecipeNotFound(name)
            }
            other => CookError::Other(other.to_string()),
        },
    )?;
    let reachable: BTreeSet<String> = edges.keys().cloned().collect();

    // Assemble the inputs the viewer expects from the registered workspace.
    // `all_units` is the reachable slice of `registered.units_by_recipe`,
    // tagged with the qualified recipe name. Recipes missing from the units
    // map (zero-unit meta-targets) get an empty `RecipeUnits` stub so the
    // viewer still sees them as a node in the graph.
    let all_units: Vec<(String, cook_engine::cook_contracts::RecipeUnits)> = reachable
        .iter()
        .map(|name| {
            let units = registered
                .units_by_recipe
                .get(name)
                .cloned()
                .unwrap_or_else(|| cook_engine::cook_contracts::RecipeUnits {
                    recipe_name: name.clone(),
                    deps: edges.get(name).cloned().unwrap_or_default(),
                    units: Vec::new(),
                    step_groups: Vec::new(),
                    working_dir: registered
                        .working_dir_by_prefix
                        .get(split_recipe_prefix(name))
                        .cloned()
                        .unwrap_or_else(|| std::path::PathBuf::from(".")),
                    env_vars: std::collections::BTreeMap::new(),
                    terminal_outputs: Vec::new(),
                    dep_edges: Vec::new(),
                    probes: Vec::new(),
                });
            (name.clone(), units)
        })
        .collect();

    // Per-recipe cache managers anchored at each recipe's prefix's working_dir.
    let cache_managers: BTreeMap<String, Arc<cook_engine::cook_cache::ThreadSafeCacheManager>> = reachable
        .iter()
        .map(|name| {
            let prefix = split_recipe_prefix(name);
            let wd = registered
                .working_dir_by_prefix
                .get(prefix)
                .cloned()
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let cache_dir = wd.join(".cook").join("cache");
            (
                name.clone(),
                Arc::new(cook_engine::cook_cache::ThreadSafeCacheManager::new(cache_dir)),
            )
        })
        .collect();

    // inferred_deps is empty in the unified-DAG model — cross-recipe edges
    // live directly on `RecipeUnits.dep_edges` inside `all_units`, not on a
    // separate analyzer-level map. The viewer's wave_grouper still accepts
    // the map (legacy compatibility), so we pass an empty one.
    let inferred_deps: BTreeMap<String, Vec<String>> = BTreeMap::new();

    cook_dag_viewer::cmd_dag(&cook_dag_viewer::DagViewerInputs {
        target: recipe_name,
        all_units: &all_units,
        explicit_edges: &edges,
        inferred_deps: &inferred_deps,
        cache_managers: &cache_managers,
        theme: cook_dag_viewer::theme::Theme::from_str(&args.theme),
    })
    .map_err(|e| CookError::Other(e.to_string()))
}

/// Split off the namespace prefix from a qualified recipe name.
///
/// `"backend.proto.generate"` → `"backend.proto"`
/// `"build"` → `""`
#[cfg(feature = "viewer")]
fn split_recipe_prefix(name: &str) -> &str {
    name.rfind('.').map(|p| &name[..p]).unwrap_or("")
}
