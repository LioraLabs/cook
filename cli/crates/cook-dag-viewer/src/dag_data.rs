//! DAG data model for JSON serialization to the frontend viewer.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use cook_cache::ThreadSafeCacheManager;
use cook_fingerprint::{hash_file, needs_rebuild_cook, stat_mtime, RebuildResult};
use cook_contracts::{DepKind, RecipeUnits, WorkPayload};
use cook_engine::wave_grouper;
use std::collections::BTreeSet;

use crate::VIEWER_SCHEMA_VERSION;

// ---------------------------------------------------------------------------
// Wave-grouped DAG data model
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct WaveDagData {
    /// Wire-format schema version. CS-0048: writers always emit
    /// `VIEWER_SCHEMA_VERSION`; the embedded JS viewer refuses payloads whose
    /// `schema_version` exceeds the highest version it recognises.
    pub schema_version: u32,
    pub target: String,
    pub waves: Vec<WaveData>,
    pub inter_wave_edges: Vec<EdgeData>,
}

#[derive(Serialize, Clone)]
pub struct WaveData {
    pub recipes: Vec<String>,
    pub nodes: Vec<NodeData>,
    pub edges: Vec<EdgeData>,
}

#[derive(Serialize, Clone)]
pub struct NodeData {
    pub id: String,
    pub kind: String,       // "file" or "unit"
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dep_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<bool>,
    /// `Some(true)` for file nodes whose path came from a `discovered_inputs`
    /// depfile rather than from `meta.input_paths`. `None` (omitted from JSON)
    /// for declared file nodes and units. See
    /// `docs/superpowers/specs/2026-05-06-dag-tui-discovered-deps-design.md` §4.1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered: Option<bool>,
}

#[derive(Serialize, Clone)]
pub struct EdgeData {
    pub from: String,
    pub to: String,
}


// ---------------------------------------------------------------------------
// Wave-grouped DAG builder
// ---------------------------------------------------------------------------

/// Build the unified wave-grouped DAG data from registered RecipeUnits.
///
/// Calls `wave_grouper::compute_waves` to determine execution order, then for
/// each wave builds a flat node+edge graph that includes file nodes, unit
/// nodes, and all intra-wave edges.  Inter-wave edges connect the terminal
/// barrier nodes of wave N to the root barrier nodes of wave N+1 for recipes
/// that are joined by explicit deps.
pub fn build_wave_dag_data(
    target: &str,
    all_units: &[(String, RecipeUnits)],
    explicit_deps: &BTreeMap<String, Vec<String>>,
    inferred_deps: &BTreeMap<String, Vec<String>>,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
) -> WaveDagData {
    // Index by name for fast lookup.
    let units_by_name: BTreeMap<&str, &RecipeUnits> = all_units
        .iter()
        .map(|(name, ru)| (name.as_str(), ru))
        .collect();
    let all_recipe_names: BTreeSet<String> = all_units.iter().map(|(n, _)| n.clone()).collect();

    let waves = wave_grouper::compute_waves(explicit_deps, inferred_deps, &all_recipe_names)
        .unwrap_or_else(|_| {
            // Fall back to a single wave containing all recipes on cycle error.
            vec![wave_grouper::Wave {
                recipes: all_recipe_names.iter().cloned().collect(),
            }]
        });

    // Per-recipe terminal node IDs, populated while building each wave and
    // used to wire inter-wave edges.
    let mut recipe_terminals: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Per-recipe root node IDs (first barrier set after processing the first
    // unit of a recipe).
    let mut recipe_roots: BTreeMap<String, Vec<String>> = BTreeMap::new();

    let mut wave_data_list: Vec<WaveData> = Vec::new();

    for wave in &waves {
        let (wave_data, terminals, roots) = build_wave(
            &wave.recipes,
            &units_by_name,
            cache_managers,
            &recipe_terminals,
        );
        wave_data_list.push(wave_data);
        recipe_terminals.extend(terminals);
        // Only record roots for recipes not yet seen (first occurrence).
        for (recipe, root_ids) in roots {
            recipe_roots.entry(recipe).or_insert(root_ids);
        }
    }

    // Build inter-wave edges: for each explicit dep A -> B where A and B
    // live in different waves, connect the terminals of A to the roots of B.
    let mut inter_wave_edges: Vec<EdgeData> = Vec::new();
    for (recipe, deps) in explicit_deps {
        let Some(roots) = recipe_roots.get(recipe) else {
            continue;
        };
        for dep in deps {
            let Some(terminals) = recipe_terminals.get(dep) else {
                continue;
            };
            for terminal in terminals {
                for root in roots {
                    inter_wave_edges.push(EdgeData {
                        from: terminal.clone(),
                        to: root.clone(),
                    });
                }
            }
        }
    }

    WaveDagData {
        schema_version: VIEWER_SCHEMA_VERSION,
        target: target.to_string(),
        waves: wave_data_list,
        inter_wave_edges,
    }
}

/// Build the `WaveData` for a single wave.
///
/// Returns:
/// - The `WaveData` (nodes + edges for all recipes in the wave).
/// - A map of recipe_name → terminal unit node IDs for this wave.
/// - A map of recipe_name → root unit node IDs for this wave.
fn build_wave(
    recipe_names: &[String],
    units_by_name: &BTreeMap<&str, &RecipeUnits>,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    // Terminals from prior waves, used to wire intra-wave cross-recipe edges.
    prior_recipe_terminals: &BTreeMap<String, Vec<String>>,
) -> (WaveData, BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>) {
    let mut nodes: Vec<NodeData> = Vec::new();
    let mut edges: Vec<EdgeData> = Vec::new();

    // Deduplicated file nodes: path → node id.
    let mut file_node_ids: BTreeMap<String, String> = BTreeMap::new();

    // Collect all unit output paths in this wave so we can skip creating
    // file nodes for intermediate build artifacts (e.g. .o files that are
    // both a unit's output and another unit's input).
    let mut unit_output_paths: BTreeSet<String> = BTreeSet::new();
    for recipe_name in recipe_names {
        if let Some(ru) = units_by_name.get(recipe_name.as_str()) {
            for unit in &ru.units {
                if let Some(meta) = &unit.cache_meta {
                    for out in &meta.output_paths {
                        unit_output_paths.insert(out.clone());
                    }
                }
            }
        }
    }

    // Per-recipe terminal unit node IDs (last barrier after processing units).
    let mut recipe_terminals: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Per-recipe root unit node IDs (first barrier encountered).
    let mut recipe_roots: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for recipe_name in recipe_names {
        let Some(ru) = units_by_name.get(recipe_name.as_str()) else {
            continue;
        };
        let cm = cache_managers.get(recipe_name);

        // Issue 4: Load cache once per recipe, outside the unit loop.
        let recipe_cache = cm.as_ref().map(|mgr| mgr.get_or_load(recipe_name));

        let mut barrier: Vec<String> = Vec::new();
        let mut recipe_root_recorded = false;

        for (unit_idx, unit) in ru.units.iter().enumerate() {
            let unit_id = format!("unit:{}:{}", recipe_name, unit_idx);

            // --- Command label ---
            let command = match &unit.payload {
                WorkPayload::Shell { cmd, .. } => cmd.clone(),
                WorkPayload::Interactive { cmd, .. } => format!("@{cmd}"),
                WorkPayload::LuaChunk { code, .. } => {
                    format!("lua: {}", &code[..code.len().min(60)])
                }
                WorkPayload::Test { cmd, .. } => format!("test: {cmd}"),
                // `WorkPayload` is `#[non_exhaustive]`; viewer falls back to the
                // payload's `display_name` for unknown future kinds.
                _ => unit.payload.display_name(),
            };

            let output = unit.cache_meta.as_ref().and_then(|m| m.output_paths.first().cloned());

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
                // `DepKind` is `#[non_exhaustive]`; surface unknown future
                // variants to the viewer as a generic label so the UI doesn't
                // silently drop them.
                _ => ("unknown".to_string(), None),
            };

            // --- Cache status ---
            let cached = if let (Some(meta), Some(cache)) = (&unit.cache_meta, recipe_cache.as_ref()) {
                if meta.output_paths.is_empty() {
                    None
                } else {
                    let entry = cache.steps.get(&meta.cache_key);
                    let input_refs: Vec<&str> =
                        meta.input_paths.iter().map(|s| s.as_str()).collect();
                    let current_outputs: Vec<&str> =
                        meta.output_paths.iter().map(|s| s.as_str()).collect();
                    // Viewer query — no restore side-effects.
                    let (result, _) = needs_rebuild_cook(
                        entry,
                        &input_refs,
                        &current_outputs,
                        meta.command_hash,
                        meta.context_hash,
                        meta.env_contribution,
                        &ru.working_dir,
                        None,
                        None,
                    );
                    Some(matches!(result, RebuildResult::Skip))
                }
            } else {
                None
            };

            // --- Unit node ---
            nodes.push(NodeData {
                id: unit_id.clone(),
                kind: "unit".to_string(),
                label,
                recipe: Some(recipe_name.clone()),
                command: Some(command),
                output: output.clone(),
                cached,
                dep_kind: Some(dep_kind_str),
                group_index,
                modified: None,
                discovered: None,
            });

            // --- File nodes + file→unit edges ---
            if let Some(meta) = &unit.cache_meta {
                // Issue 4: Use pre-loaded recipe cache for staleness checks.
                let cache_entry = recipe_cache.as_ref().map(|cache| {
                    cache.steps.get(&meta.cache_key).cloned()
                });

                // Issue 3: Deduplicate input_paths before iterating to avoid duplicate edges.
                let unique_paths: BTreeSet<&String> = meta.input_paths.iter().collect();

                for path in unique_paths {
                    // Skip inputs that are outputs of other units in this wave
                    // (e.g. .o files produced by compile steps and consumed by
                    // archive steps). The unit→unit edge already captures this.
                    if unit_output_paths.contains(path.as_str()) {
                        continue;
                    }

                    let file_id = format!("file:{}", path);

                    if !file_node_ids.contains_key(path) {
                        // Determine staleness: mtime first, then hash.
                        // Issue 2: Look up by path instead of positional index.
                        let modified = compute_file_modified(
                            path,
                            &ru.working_dir,
                            cache_entry.as_ref().and_then(|e| e.as_ref()).and_then(|e| {
                                e.inputs.iter().find(|r| r.path == *path).map(|r| (r.mtime, r.hash))
                            }),
                        );

                        let file_label = Path::new(path)
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone());

                        nodes.push(NodeData {
                            id: file_id.clone(),
                            kind: "file".to_string(),
                            label: file_label,
                            recipe: None,
                            command: None,
                            output: None,
                            cached: None,
                            dep_kind: None,
                            group_index: None,
                            modified: Some(modified),
                            discovered: None,
                        });
                        file_node_ids.insert(path.clone(), file_id.clone());
                    }

                    edges.push(EdgeData {
                        from: file_id,
                        to: unit_id.clone(),
                    });
                }
            }

            // --- Barrier / intra-recipe unit→unit edges ---
            match &unit.dep_kind {
                DepKind::Sequential => {
                    for b in &barrier {
                        edges.push(EdgeData {
                            from: b.clone(),
                            to: unit_id.clone(),
                        });
                    }
                    barrier = vec![unit_id.clone()];
                }
                DepKind::StepGroup(gi) | DepKind::TestSibling(gi) => {
                    let group = &ru.step_groups[*gi];
                    let is_first = group.first() == Some(&unit_idx);
                    if is_first {
                        for b in &barrier {
                            for &member_idx in group {
                                let member_id =
                                    format!("unit:{}:{}", recipe_name, member_idx);
                                edges.push(EdgeData {
                                    from: b.clone(),
                                    to: member_id,
                                });
                            }
                        }
                    }
                    let is_last = group.last() == Some(&unit_idx);
                    if is_last {
                        barrier = group
                            .iter()
                            .map(|&idx| format!("unit:{}:{}", recipe_name, idx))
                            .collect();
                    }
                }
                // `DepKind` is `#[non_exhaustive]`; render unknown future
                // variants as a fresh sequential edge so the viewer stays
                // structurally honest.
                _ => {
                    for b in &barrier {
                        edges.push(EdgeData {
                            from: b.clone(),
                            to: unit_id.clone(),
                        });
                    }
                    barrier = vec![unit_id.clone()];
                }
            }

            // Issue 1: Record recipe root AFTER the dep_kind match updates the
            // barrier, so StepGroup-first recipes capture all group members.
            if !recipe_root_recorded && !barrier.is_empty() {
                recipe_roots
                    .entry(recipe_name.clone())
                    .or_insert(barrier.clone());
                recipe_root_recorded = true;
            }
        }

        // Terminal nodes for this recipe = final barrier.
        if !barrier.is_empty() {
            recipe_terminals.insert(recipe_name.clone(), barrier);
        }

        // --- Cross-recipe dep edges within this wave ---
        // For each (unit_idx, dep_recipe_name) in dep_edges, wire unit to the
        // terminal nodes of dep_recipe within this wave (or prior waves).
        for (unit_idx, dep_recipe) in &ru.dep_edges {
            let unit_id = format!("unit:{}:{}", recipe_name, unit_idx);

            let dep_terminals = recipe_terminals
                .get(dep_recipe)
                .or_else(|| prior_recipe_terminals.get(dep_recipe));

            if let Some(terminals) = dep_terminals {
                for terminal in terminals {
                    edges.push(EdgeData {
                        from: terminal.clone(),
                        to: unit_id.clone(),
                    });
                }
            }
        }
    }

    let wave_data = WaveData {
        recipes: recipe_names.to_vec(),
        nodes,
        edges,
    };

    (wave_data, recipe_terminals, recipe_roots)
}

/// Check whether a file is modified relative to its cached record.
///
/// Checks mtime first (cheap). If mtime differs, falls back to hash comparison.
/// Returns `true` if the file appears modified or cannot be read.
fn compute_file_modified(
    rel_path: &str,
    working_dir: &Path,
    cached: Option<(u64, u64)>,
) -> bool {
    let abs = working_dir.join(rel_path);
    let Some((cached_mtime, cached_hash)) = cached else {
        // No cache entry → treat as modified (needs build).
        return true;
    };
    let Some(disk_mtime) = stat_mtime(&abs) else {
        return true;
    };
    if disk_mtime == cached_mtime {
        return false;
    }
    // mtime differs — check hash to distinguish genuine content change from
    // a metadata-only touch.
    match hash_file(&abs) {
        Some(h) => h != cached_hash,
        None => true,
    }
}

