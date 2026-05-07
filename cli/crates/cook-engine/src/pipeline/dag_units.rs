//! Drive the recipe DAG to register every recipe reachable from `targets`
//! and collect their `RecipeUnits` for non-execution consumers (the DAG
//! viewer, future `--explain` output, etc.).
//!
//! Mirrors the per-wave dispatch loop in `crate::run::run` but stops short
//! of work-unit DAG construction and execution: this helper produces the
//! registered units, the explicit-edge map, the inferred-deps map, and the
//! per-recipe cache managers — exactly what `cook_dag_viewer::cmd_dag`
//! consumes. Works for both single-Cookfile (one registry under the `""`
//! prefix) and workspace (one registry per dotted prefix) inputs.

use std::collections::BTreeMap;
use std::sync::Arc;

use cook_cache::ThreadSafeCacheManager;

use crate::analyzer::{self, GraphError, RecipeInfo};
use crate::recipe_dag::RecipeDag;
use crate::RegistryEntry;

use super::error::PipelineError;

/// Bundle of intermediate state collected by `collect_dag_units`.
#[allow(clippy::type_complexity)]
pub struct DagUnits {
    pub all_units: Vec<(String, cook_contracts::RecipeUnits)>,
    pub explicit_edges: BTreeMap<String, Vec<String>>,
    pub inferred_deps: BTreeMap<String, Vec<String>>,
    pub cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>>,
}

/// Drive the recipe DAG to register every recipe reachable from `targets`
/// and collect their `RecipeUnits` plus the bookkeeping the viewer needs
/// (explicit edges, inferred deps, per-recipe cache managers).
///
/// `targets` MUST contain at least one name; `inferred_deps` is the same
/// map that `crate::run::run` consumes — pass a fresh `BTreeMap::new()`
/// only if the caller has confirmed there are no `{NAME}` body refs.
pub fn collect_dag_units(
    recipe_infos: &BTreeMap<String, RecipeInfo>,
    targets: &[String],
    registries: &BTreeMap<String, RegistryEntry>,
    inferred_deps: &BTreeMap<String, Vec<String>>,
) -> Result<DagUnits, PipelineError> {
    let mut edges = analyzer::dependency_edges_multi(recipe_infos, targets).map_err(|e| match e {
        GraphError::CycleDetected(s) => {
            PipelineError::Other(format!("dependency cycle involving: {s}"))
        }
        GraphError::UnknownRecipe(s) => PipelineError::Other(format!("unknown recipe: {s}")),
        // Io/Parse cannot be produced by dependency_edges_multi (pure graph op).
        e => PipelineError::Other(e.to_string()),
    })?;

    // Save explicit edges before merging inferred deps (needed for wave grouping).
    let explicit_edges = edges.clone();

    // Merge inferred deps into the edge map so the RecipeDag registers
    // recipes in the correct order.
    for (recipe_name, deps) in inferred_deps {
        for dep_name in deps {
            edges.entry(dep_name.clone()).or_default();
            let entry = edges.entry(recipe_name.clone()).or_default();
            if !entry.contains(dep_name) {
                entry.push(dep_name.clone());
            }
        }
    }
    for deps in edges.values_mut() {
        deps.sort();
    }

    let mut recipe_dag = RecipeDag::new(&edges);
    let mut all_units: Vec<(String, cook_contracts::RecipeUnits)> = Vec::new();
    let mut cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>> = BTreeMap::new();

    loop {
        let ready = recipe_dag.pop_ready();
        if ready.is_empty() {
            break;
        }

        for qualified_name in &ready {
            // Split off the namespace prefix so the right registry handles
            // registration. Single-Cookfile recipes always live under the "" prefix.
            let (prefix, local_name) = match qualified_name.rfind('.') {
                Some(pos) => (&qualified_name[..pos], &qualified_name[pos + 1..]),
                None => ("", qualified_name.as_str()),
            };
            let entry = registries.get(prefix).ok_or_else(|| {
                PipelineError::Other(format!(
                    "no registry for prefix '{prefix}' (recipe '{qualified_name}')"
                ))
            })?;

            let mut units = entry
                .registry
                .register_recipe(&entry.lua_source, local_name, None)
                .map_err(|e| {
                    PipelineError::Other(format!(
                        "registration failed for '{qualified_name}': {e}"
                    ))
                })?;
            // Rewrite to the fully qualified form so build_wave_dag_data
            // sees the same names everywhere.
            units.recipe_name = qualified_name.clone();

            let cache_dir = entry.registry.working_dir().join(".cook").join("cache");
            cache_managers
                .entry(qualified_name.clone())
                .or_insert_with(|| Arc::new(ThreadSafeCacheManager::new(cache_dir)));

            all_units.push((qualified_name.clone(), units));
        }

        recipe_dag.mark_done(&ready);
    }

    Ok(DagUnits {
        all_units,
        explicit_edges,
        inferred_deps: inferred_deps.clone(),
        cache_managers,
    })
}
