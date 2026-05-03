//! Unified engine entry point for both single-Cookfile and workspace builds.
//!
//! `run()` takes the full recipe graph, resolves dependencies, and executes
//! recipes in wave-parallel order using the recipe DAG + work-unit DAG pipeline.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;

use cook_cache::{
    backend::LocalBackend, cache_ctx::CacheContext, cloud_config::CloudConfig,
    context::ExecutionContext, envkey::EnvDenylist, CacheBackend, ThreadSafeCacheManager,
};

use crate::analyzer::{self, GraphError};
use crate::{dag_builder, executor, wave_grouper, EngineError, EngineEvent, RegistryEntry};

/// The result of a successful engine run.
#[derive(Debug)]
pub struct RunResult {
    pub test_outputs: Vec<cook_luaotp::TestOutput>,
}

/// Split a namespaced recipe name into (prefix, local_name).
///
/// `"backend.proto.generate"` -> `("backend.proto", "generate")`
/// `"build"` -> `("", "build")`
fn split_recipe_name(name: &str) -> (String, String) {
    if let Some(dot_pos) = name.rfind('.') {
        (name[..dot_pos].to_string(), name[dot_pos + 1..].to_string())
    } else {
        (String::new(), name.to_string())
    }
}

/// Unified engine entry point.
///
/// Resolves the dependency graph for all `targets`, then executes recipes in
/// wave-parallel order: each wave registers its recipes, builds a work-unit
/// DAG, and executes it with `num_jobs` parallelism.
///
/// # Arguments
///
/// * `project_root` - The project root directory. Used to probe execution context,
///   load `.cook/cloud.toml`, and compute cookfile-relative paths.
/// * `recipe_infos` - All known recipes and their dependency metadata.
/// * `targets` - The target recipes to build.
/// * `registries` - One `RegistryEntry` per namespace prefix. Root recipes use `""` as the key.
/// * `num_jobs` - Maximum number of parallel worker threads.
/// * `inferred_deps` - `{dep}` references: recipe -> recipes it references.
///   These cause same-wave merging rather than wave boundaries.
/// * `on_event` - Callback invoked for each engine event (progress, errors, etc.).
pub fn run(
    project_root: &Path,
    recipe_infos: &BTreeMap<String, analyzer::RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, RegistryEntry>,
    num_jobs: usize,
    inferred_deps: &BTreeMap<String, Vec<String>>,
    on_event: impl Fn(EngineEvent) + Send + Sync,
) -> Result<RunResult, EngineError> {
    let started = std::time::Instant::now();
    let result = run_inner(
        project_root,
        recipe_infos,
        targets,
        registries,
        num_jobs,
        inferred_deps,
        &on_event,
    );
    on_event(EngineEvent::Finished {
        elapsed: started.elapsed(),
        success: result.is_ok(),
    });
    result
}

fn run_inner<F>(
    project_root: &Path,
    recipe_infos: &BTreeMap<String, analyzer::RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, RegistryEntry>,
    num_jobs: usize,
    inferred_deps: &BTreeMap<String, Vec<String>>,
    on_event: &F,
) -> Result<RunResult, EngineError>
where
    F: Fn(EngineEvent) + Send + Sync,
{
    // ── Cache bootstrap ──────────────────────────────────────────────────────
    // Load .cook/cloud.toml (default if absent), build EnvDenylist, probe
    // ExecutionContext (machine identity + tool-binary hashing), construct
    // LocalBackend, and assemble CacheContext. Built once per build invocation.
    let cloud_config = CloudConfig::load_or_default(project_root)
        .map_err(|e| EngineError::CacheError(format!("invalid .cook/cloud.toml: {e}")))?;
    let mut denylist = EnvDenylist::baseline();
    denylist.extend_with(cloud_config.cache_ignore_env());
    let denylist = Arc::new(denylist);
    let exec_ctx = Arc::new(ExecutionContext::probe());
    let cache_dir = cloud_config
        .cache_dir()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("cook")
                .join("cloud")
        });
    let backend: Arc<dyn CacheBackend> = Arc::new(LocalBackend::new(cache_dir));
    if let Err(e) = backend.health() {
        tracing::warn!("cache backend unavailable: {e}; continuing with backend disabled");
    }
    let project_id = cloud_config.project_id_or_fallback(project_root);
    let cache_ctx = Arc::new(CacheContext {
        exec_ctx,
        denylist,
        backend,
        cloud_config: Arc::new(cloud_config),
        project_root: project_root.to_path_buf(),
        project_id,
    });
    // ─────────────────────────────────────────────────────────────────────────

    // 1. Compute the full dependency graph for all targets.
    let edges = analyzer::dependency_edges_multi(recipe_infos, targets).map_err(|e| match e {
        GraphError::CycleDetected(s) => EngineError::CycleDetected(s),
        GraphError::UnknownRecipe(s) => EngineError::UnknownRecipe(s),
    })?;

    // 2. Compute waves using the two-tier grouping algorithm.
    //    `edges` (from the analyzer) contains all non-inferred dependency edges
    //    (`: dep` declarations + name references from codegen) and serves as
    //    `explicit_deps` — these create wave boundaries.
    //    `inferred_deps` (`{dep}` references) cause same-wave merging.
    let all_recipe_names: BTreeSet<String> = edges.keys().cloned().collect();
    let waves = wave_grouper::compute_waves(&edges, inferred_deps, &all_recipe_names)
        .map_err(|e| EngineError::CycleDetected(e))?;

    // Emit BuildStarted once. Include only recipes reachable from the target(s),
    // in topological order derived from the computed waves.
    let topos: Vec<crate::RecipeTopology> = waves
        .iter()
        .flat_map(|wave| wave.recipes.iter())
        .filter_map(|name| {
            recipe_infos.get(name).map(|info| crate::RecipeTopology {
                name: name.clone(),
                deps: info.requires.clone(),
                expected_nodes: 0, // filled in by RecipeStarted's implied count
            })
        })
        .collect();
    let total_nodes = topos.iter().map(|t| t.expected_nodes).sum();
    on_event(EngineEvent::BuildStarted {
        recipes: topos,
        total_nodes,
    });

    // Emit RecipeQueued for every recipe in the graph.
    for name in edges.keys() {
        on_event(EngineEvent::RecipeQueued {
            name: name.clone(),
        });
    }

    let all_test_outputs: Vec<cook_luaotp::TestOutput> = Vec::new();

    // 3. Wave loop: iterate over pre-computed waves, register, build DAG, execute.
    for wave in &waves {
        // Register all recipes in this wave and collect their RecipeUnits.
        let mut wave_units = Vec::new();
        let mut wave_cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>> = BTreeMap::new();

        for name in &wave.recipes {
            let (prefix, local_name) = split_recipe_name(name);
            let entry = registries.get(&prefix).ok_or_else(|| {
                EngineError::RegistrationFailed {
                    recipe: name.clone(),
                    message: format!("no registry for prefix '{prefix}'"),
                }
            })?;
            let registry = &entry.registry;
            let lua_source = &entry.lua_source;

            let mut units = registry
                .register_recipe(lua_source, &local_name, Some(cache_ctx.clone()))
                .map_err(|e| EngineError::RegistrationFailed {
                    recipe: name.clone(),
                    message: e.to_string(),
                })?;

            // Rewrite recipe_name to the fully namespaced form.
            units.recipe_name = name.clone();

            // Set cross-recipe deps from the edge map so build_dag can wire them.
            if let Some(deps) = edges.get(name) {
                units.deps = deps.clone();
            }

            // Create a cache manager for this recipe based on its registry's working_dir.
            let cache_dir = registry.working_dir().join(".cook").join("cache");
            wave_cache_managers
                .entry(name.clone())
                .or_insert_with(|| Arc::new(ThreadSafeCacheManager::new(cache_dir)));

            wave_units.push(units);
        }

        // Build work-unit DAG for this wave and execute it.
        let dag = dag_builder::build_dag(wave_units);

        // Emit synthetic lifecycle events for zero-work recipes (meta-targets
        // that have no cook steps of their own, only deps).  The executor never
        // sees these recipes, so without this they stay stuck in Waiting.
        {
            let recipes_in_dag: BTreeSet<&str> = (0..dag.len())
                .map(|i| dag.node(i).payload.recipe_name.as_str())
                .collect();
            for name in &wave.recipes {
                if !recipes_in_dag.contains(name.as_str()) {
                    on_event(EngineEvent::RecipeStarted {
                        name: name.clone(),
                        total_nodes: 0,
                    });
                    on_event(EngineEvent::RecipeCompleted {
                        name: name.clone(),
                        elapsed: std::time::Duration::ZERO,
                        cached_nodes: 0,
                        total_nodes: 0,
                    });
                }
            }
        }

        if !dag.is_empty() {
            // Bridge on_event through an mpsc channel so executor can use its
            // existing Option<Sender<EngineEvent>> interface.
            let (event_tx, event_rx) = mpsc::channel::<EngineEvent>();
            let bridge_handle = {
                let on_event_ref = &on_event;
                // We need to forward events in a separate thread because
                // execute_dag blocks the current thread.
                std::thread::scope(|s| {
                    let handle = s.spawn(move || {
                        while let Ok(event) = event_rx.recv() {
                            on_event_ref(event);
                        }
                    });

                    let exec_result = executor::execute_dag(
                        dag,
                        num_jobs,
                        wave_cache_managers,
                        Some(event_tx),
                        cache_ctx.clone(),
                    );

                    // Drop the sender end is handled by execute_dag returning
                    // (event_tx was moved in). Wait for bridge thread to drain.
                    let _ = handle.join();

                    exec_result
                })
            };

            bridge_handle?;
        }
    }

    Ok(RunResult {
        test_outputs: all_test_outputs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::RecipeInfo;

    #[test]
    fn test_split_recipe_name_with_prefix() {
        let (prefix, local) = split_recipe_name("backend.proto.generate");
        assert_eq!(prefix, "backend.proto");
        assert_eq!(local, "generate");
    }

    #[test]
    fn test_split_recipe_name_no_prefix() {
        let (prefix, local) = split_recipe_name("build");
        assert_eq!(prefix, "");
        assert_eq!(local, "build");
    }

    #[test]
    fn test_split_recipe_name_single_dot() {
        let (prefix, local) = split_recipe_name("backend.build");
        assert_eq!(prefix, "backend");
        assert_eq!(local, "build");
    }

    fn dummy_project_root() -> std::path::PathBuf {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        std::mem::forget(dir);
        path
    }

    #[test]
    fn test_run_unknown_target() {
        let recipes = BTreeMap::new();
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();
        let result = run(
            &dummy_project_root(),
            &recipes,
            &["missing".to_string()],
            &registries,
            1,
            &inferred,
            |_| {},
        );
        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::UnknownRecipe(name) => assert_eq!(name, "missing"),
            other => panic!("expected UnknownRecipe, got: {other:?}"),
        }
    }

    #[test]
    fn test_run_empty_targets() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: vec![],
            },
        );
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();
        // Empty targets means no edges, so the wave loop exits immediately.
        let result = run(&dummy_project_root(), &recipes, &[], &registries, 1, &inferred, |_| {});
        assert!(result.is_ok());
        assert!(result.unwrap().test_outputs.is_empty());
    }

    #[test]
    fn test_run_cycle_detected() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "a".to_string(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: vec!["b".to_string()],
            },
        );
        recipes.insert(
            "b".to_string(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: vec!["a".to_string()],
            },
        );
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();
        let result = run(&dummy_project_root(), &recipes, &["a".to_string()], &registries, 1, &inferred, |_| {});
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EngineError::CycleDetected(_)
        ));
    }

    #[test]
    fn test_run_emits_finished_success_on_empty_targets() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: vec![],
            },
        );
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();

        let events = std::sync::Mutex::new(Vec::new());
        let result = run(&dummy_project_root(), &recipes, &[], &registries, 1, &inferred, |event| {
            events.lock().unwrap().push(event)
        });
        assert!(result.is_ok());

        let events = events.lock().unwrap();
        let finished = events.iter().find_map(|e| match e {
            EngineEvent::Finished { success, .. } => Some(*success),
            _ => None,
        });
        assert_eq!(
            finished,
            Some(true),
            "expected Finished{{success:true}} event"
        );
    }

    #[test]
    fn test_run_emits_finished_failure_on_unknown_target() {
        let recipes = BTreeMap::new();
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();

        let events = std::sync::Mutex::new(Vec::new());
        let result = run(
            &dummy_project_root(),
            &recipes,
            &["missing".to_string()],
            &registries,
            1,
            &inferred,
            |event| events.lock().unwrap().push(event),
        );
        assert!(result.is_err());

        let events = events.lock().unwrap();
        let finished = events.iter().find_map(|e| match e {
            EngineEvent::Finished { success, .. } => Some(*success),
            _ => None,
        });
        assert_eq!(
            finished,
            Some(false),
            "expected Finished{{success:false}} event"
        );
    }

    #[test]
    fn test_run_accepts_inferred_deps() {
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "build".to_string(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: vec![],
            },
        );
        let registries = BTreeMap::new();
        let inferred = BTreeMap::new();
        // This will fail because no registry for "" prefix, but it shouldn't panic
        let result = run(&dummy_project_root(), &recipes, &["build".to_string()], &registries, 1, &inferred, |_| {});
        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::RegistrationFailed { recipe, .. } => assert_eq!(recipe, "build"),
            other => panic!("expected RegistrationFailed, got: {other:?}"),
        }
    }
}
