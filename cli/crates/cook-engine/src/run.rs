//! Unified engine entry point for both single-Cookfile and workspace builds.
//!
//! `run()` takes the full recipe graph, resolves dependencies, and executes
//! recipes in wave-parallel order using the recipe DAG + work-unit DAG pipeline.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::Arc;

use cook_cache::ThreadSafeCacheManager;

use crate::analyzer::{self, GraphError};
use crate::recipe_dag::RecipeDag;
use crate::{dag_builder, executor, EngineError, EngineEvent};

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
/// * `recipe_infos` - All known recipes and their dependency metadata.
/// * `targets` - The target recipes to build.
/// * `registries` - One `(Registry, lua_source)` per namespace prefix.
///   Root recipes use `""` as the key.
/// * `num_jobs` - Maximum number of parallel worker threads.
/// * `on_event` - Callback invoked for each engine event (progress, errors, etc.).
pub fn run(
    recipe_infos: &BTreeMap<String, analyzer::RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, (cook_register::Registry, String)>,
    num_jobs: usize,
    on_event: impl Fn(EngineEvent) + Send + Sync,
) -> Result<RunResult, EngineError> {
    // 1. Compute the full dependency graph for all targets.
    let edges = analyzer::dependency_edges_multi(recipe_infos, targets).map_err(|e| match e {
        GraphError::CycleDetected(s) => EngineError::CycleDetected(s),
        GraphError::UnknownRecipe(s) => EngineError::UnknownRecipe(s),
    })?;

    // 2. Build the recipe-level DAG.
    let mut recipe_dag = RecipeDag::new(&edges);

    // Emit RecipeQueued for every recipe in the graph.
    for name in edges.keys() {
        on_event(EngineEvent::RecipeQueued {
            name: name.clone(),
        });
    }

    let all_test_outputs: Vec<cook_luaotp::TestOutput> = Vec::new();

    // 3. Wave loop: pop ready recipes, register, build DAG, execute.
    loop {
        let ready = recipe_dag.pop_ready();
        if ready.is_empty() {
            break;
        }

        // Register all recipes in this wave and collect their RecipeUnits.
        let mut wave_units = Vec::new();
        let mut wave_cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>> = BTreeMap::new();

        for name in &ready {
            let (prefix, local_name) = split_recipe_name(name);
            let (registry, lua_source) = registries.get(&prefix).ok_or_else(|| {
                EngineError::RegistrationFailed {
                    recipe: name.clone(),
                    message: format!("no registry for prefix '{prefix}'"),
                }
            })?;

            let mut units = registry.register_recipe(lua_source, &local_name).map_err(
                |e| EngineError::RegistrationFailed {
                    recipe: name.clone(),
                    message: e.to_string(),
                },
            )?;

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
                    );

                    // Drop the sender end is handled by execute_dag returning
                    // (event_tx was moved in). Wait for bridge thread to drain.
                    let _ = handle.join();

                    exec_result
                })
            };

            bridge_handle?;
        }

        recipe_dag.mark_done(&ready);
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

    #[test]
    fn test_run_unknown_target() {
        let recipes = BTreeMap::new();
        let registries = BTreeMap::new();
        let result = run(
            &recipes,
            &["missing".to_string()],
            &registries,
            1,
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
        // Empty targets means no edges, so the wave loop exits immediately.
        let result = run(&recipes, &[], &registries, 1, |_| {});
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
        let result = run(&recipes, &["a".to_string()], &registries, 1, |_| {});
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EngineError::CycleDetected(_)
        ));
    }
}
