//! DAG data model for JSON serialization to the frontend viewer.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use cook_cache::{needs_rebuild_cook, RebuildResult, ThreadSafeCacheManager};
use cook_contracts::{CapturedUnit, DepKind, RecipeUnits, WorkPayload};

#[derive(Serialize)]
pub struct DagData {
    pub target: String,
    pub recipes: Vec<RecipeData>,
}

#[derive(Serialize)]
pub struct RecipeData {
    pub name: String,
    pub deps: Vec<String>,
    pub units: Vec<UnitData>,
    pub step_groups: Vec<Vec<usize>>,
    pub internal_edges: Vec<InternalEdge>,
}

#[derive(Serialize)]
pub struct UnitData {
    pub id: String,
    pub label: String,
    pub command: String,
    pub inputs: Vec<String>,
    pub output: Option<String>,
    pub dep_kind: String,
    pub group_index: Option<usize>,
    pub cached: bool,
}

#[derive(Serialize)]
pub struct InternalEdge {
    pub from: usize,
    pub to: usize,
}

/// Build a UnitData from a CapturedUnit, checking cache status.
fn build_unit_data(
    recipe_name: &str,
    unit_idx: usize,
    unit: &CapturedUnit,
    cache_manager: Option<&Arc<ThreadSafeCacheManager>>,
    working_dir: &Path,
) -> UnitData {
    let command = match &unit.payload {
        WorkPayload::Shell { cmd, .. } => cmd.clone(),
        WorkPayload::Interactive { cmd, .. } => format!("@{cmd}"),
        WorkPayload::LuaChunk { code, .. } => format!("lua: {}", &code[..code.len().min(60)]),
        WorkPayload::Test { cmd, .. } => format!("test: {cmd}"),
    };

    let (inputs, output) = match &unit.cache_meta {
        Some(meta) => (meta.input_paths.clone(), meta.output_path.clone()),
        None => (vec![], None),
    };

    let label = if let Some(ref out) = output {
        Path::new(out)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| out.clone())
    } else {
        command.chars().take(40).collect()
    };

    let (dep_kind_str, group_index) = match &unit.dep_kind {
        DepKind::StepGroup(idx) => ("step_group".to_string(), Some(*idx)),
        DepKind::Sequential => ("sequential".to_string(), None),
        DepKind::TestSibling(idx) => ("test_sibling".to_string(), Some(*idx)),
    };

    let cached = if let (Some(meta), Some(cm)) = (&unit.cache_meta, cache_manager) {
        if let Some(ref out) = meta.output_path {
            let cache = cm.get_or_load(&meta.recipe_name);
            let entry = cache.steps.get(&meta.cache_key);
            let input_refs: Vec<&str> = meta.input_paths.iter().map(|s| s.as_str()).collect();
            let (result, _) = needs_rebuild_cook(
                entry,
                &input_refs,
                out,
                meta.command_hash,
                working_dir,
            );
            matches!(result, RebuildResult::Skip)
        } else {
            false
        }
    } else {
        false
    };

    UnitData {
        id: format!("{}:{}", recipe_name, unit_idx),
        label,
        command,
        inputs,
        output,
        dep_kind: dep_kind_str,
        group_index,
        cached,
    }
}

/// Derive internal edges from step_groups and DepKind.
fn derive_internal_edges(units: &[CapturedUnit], step_groups: &[Vec<usize>]) -> Vec<InternalEdge> {
    let mut edges = Vec::new();
    let mut barrier: Vec<usize> = Vec::new();

    for (unit_idx, unit) in units.iter().enumerate() {
        match &unit.dep_kind {
            DepKind::Sequential => {
                for &b in &barrier {
                    edges.push(InternalEdge { from: b, to: unit_idx });
                }
                barrier = vec![unit_idx];
            }
            DepKind::StepGroup(gi) | DepKind::TestSibling(gi) => {
                let group = &step_groups[*gi];
                let is_first = group.first() == Some(&unit_idx);
                if is_first {
                    for &b in &barrier {
                        for &member in group {
                            edges.push(InternalEdge { from: b, to: member });
                        }
                    }
                }
                let is_last = group.last() == Some(&unit_idx);
                if is_last {
                    barrier = group.clone();
                }
            }
        }
    }

    edges
}

/// Build the full DagData from registered RecipeUnits.
pub fn build_dag_data(
    target: &str,
    all_units: &[(String, RecipeUnits)],
    recipe_deps: &BTreeMap<String, Vec<String>>,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
) -> DagData {
    let mut recipes = Vec::new();

    for (name, ru) in all_units {
        let deps = recipe_deps.get(name).cloned().unwrap_or_default();
        let cm = cache_managers.get(name);

        let units: Vec<UnitData> = ru
            .units
            .iter()
            .enumerate()
            .map(|(idx, unit)| build_unit_data(name, idx, unit, cm, &ru.working_dir))
            .collect();

        let internal_edges = derive_internal_edges(&ru.units, &ru.step_groups);

        recipes.push(RecipeData {
            name: name.clone(),
            deps,
            units,
            step_groups: ru.step_groups.clone(),
            internal_edges,
        });
    }

    DagData {
        target: target.to_string(),
        recipes,
    }
}
