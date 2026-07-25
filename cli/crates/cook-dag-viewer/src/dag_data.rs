//! DAG data model for JSON serialization to the frontend viewer.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use cook_cache::ThreadSafeCacheManager;
use cook_fingerprint::{hash_file, needs_rebuild_cook, stat_mtime, RebuildResult};
use cook_contracts::{DepKind, DiscoveredInputs, RecipeUnits, WorkPayload};
use crate::wave_grouper;
use std::collections::BTreeSet;

use crate::VIEWER_SCHEMA_VERSION;

// ---------------------------------------------------------------------------
// Wave-grouped DAG data model
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct WaveDagData {
    /// Wire-format schema version (CS-0048). Currently write-only: the JS
    /// viewer that gated on this was removed by CS-0060, so no consumer
    /// reads it. Bumped when the payload shape changes so a future external
    /// consumer can reason about compatibility.
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
    /// for declared file nodes and units. `Some(false)` is not emitted —
    /// "not discovered" is always `None`, not an explicit boolean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered: Option<bool>,
}

#[derive(Serialize, Clone)]
pub struct EdgeData {
    pub from: String,
    pub to: String,
}

/// Read and parse the depfile a unit declares via `discovered_inputs`.
///
/// On any error (missing file, I/O, malformed) returns an empty `Vec`. The
/// viewer is a tooling affordance; a stale or absent depfile must never
/// fail rendering. Errors are dropped silently for now — the crate has no
/// `tracing` dep yet; if diagnostics prove useful later, swap the
/// `unwrap_or_default()` for an `inspect_err` that emits at `debug`.
fn read_discovered_paths(
    di: &DiscoveredInputs,
    source_path: Option<&str>,
    working_dir: &Path,
) -> Vec<String> {
    let depfile = working_dir.join(&di.from);
    let source = source_path.unwrap_or("");
    cook_cache::parse_make_depfile(&depfile, source, working_dir).unwrap_or_default()
}

/// Render a file path's basename for use as a node label, falling back to
/// the full path when the path has no terminal component.
fn file_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
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

    // Wave preflight in a single pass: gather every unit's declared inputs
    // and outputs across the wave.
    //
    // - `unit_output_paths` lets the per-unit emission skip making file nodes
    //   for intermediate artifacts (e.g. .o files that are both one unit's
    //   output and another's input — the unit→unit edge already encodes that).
    // - `declared_input_paths` lets the discovered-inputs emission classify a
    //   path as declared regardless of the order units are processed in
    //   (spec §3.3): a path declared by *any* unit in the wave is rendered
    //   declared, even if a depfile sees it first.
    let mut unit_output_paths: BTreeSet<String> = BTreeSet::new();
    let mut declared_input_paths: BTreeSet<String> = BTreeSet::new();
    for recipe_name in recipe_names {
        if let Some(ru) = units_by_name.get(recipe_name.as_str()) {
            for unit in &ru.units {
                if let Some(meta) = &unit.cache_meta {
                    for out in &meta.output_paths {
                        unit_output_paths.insert(out.clone());
                    }
                    for inp in &meta.input_paths {
                        declared_input_paths.insert(inp.clone());
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
        //
        // The on-disk index is written under the units' Cookfile-local
        // `CacheMeta.recipe_name`, not the (possibly import-qualified)
        // workspace key `recipe_name`. For import recipes (e.g. workspace
        // key "rust.build", cache_meta name "build") loading under the
        // qualified key would silently miss the index and render every node
        // as never-cached.
        let cache_index_name =
            cook_engine::run::recipe_cache_index_name(ru, recipe_name);
        let recipe_cache = cm.as_ref().map(|mgr| mgr.get_or_load(&cache_index_name));

        // Probe key → unit id index. Used to wire probe→consumer edges from
        // each unit's `probes` field (mirrored from `inputs.requires` /
        // `needs`). The engine's dag_builder does the equivalent wiring in
        // `probe_unit_index_by_key`. Without this, the viewer would lose
        // probe-to-consumer edges once probes stop participating in the
        // sequential barrier below.
        let probe_unit_id_by_key: BTreeMap<String, String> = ru
            .units
            .iter()
            .enumerate()
            .filter_map(|(idx, u)| match &u.payload {
                WorkPayload::Probe { key, .. } => {
                    Some((key.clone(), format!("unit:{}:{}", recipe_name, idx)))
                }
                _ => None,
            })
            .collect();

        let mut barrier: Vec<String> = Vec::new();
        let mut recipe_root_recorded = false;

        for (unit_idx, unit) in ru.units.iter().enumerate() {
            let unit_id = format!("unit:{}:{}", recipe_name, unit_idx);
            let is_probe = matches!(unit.payload, WorkPayload::Probe { .. });

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
                    // Viewer query — no restore side-effects (COOK-161). The
                    // viewer has no live ProbeValueStore (sealed probe values only
                    // exist during an execute-phase DAG walk, not in this static
                    // graph view), so it cannot re-fold the seal set. It instead
                    // compares the persisted entry's own seal_contribution against
                    // itself, so a clean *sealed* unit is correctly shown as
                    // up-to-date rather than falsely flagged SealChanged. The one
                    // thing this cannot detect is a probe-value drift since the
                    // last build — invisible in a static view, and harmless since
                    // the viewer never writes the cache.
                    let seal_contribution = entry.map(|e| e.seal_contribution).unwrap_or(0);
                    let (result, _) = needs_rebuild_cook(
                        entry,
                        &input_refs,
                        &current_outputs,
                        meta.command_hash,
                        meta.env_contribution,
                        seal_contribution,
                        &ru.working_dir,
                        None,
                        None,
                        // COOK-163: honour the unit's record disposition so the
                        // static view doesn't falsely flag a present-but-drifted
                        // record output as needing a rebuild (execute-phase would
                        // Skip it — byte-equivalence is waived, §17.1.3).
                        meta.record,
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
                                e.inputs.iter().find(|r| &*r.path == path.as_str()).map(|r| (r.mtime, r.hash))
                            }),
                        );

                        nodes.push(NodeData {
                            id: file_id.clone(),
                            kind: "file".to_string(),
                            label: file_label(path),
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

            // --- Discovered file nodes + file→unit edges ---
            // Read the depfile if the unit declares discovered_inputs.
            // Errors (missing/malformed) downgrade to empty list — viewer
            // remains rendering-correct on stale or absent state.
            if let Some(meta) = &unit.cache_meta {
                if let Some(di) = &meta.discovered_inputs {
                    let source = meta.input_paths.first().map(String::as_str);
                    let discovered = read_discovered_paths(di, source, &ru.working_dir);
                    for path in discovered {
                        if unit_output_paths.contains(path.as_str()) {
                            continue;
                        }
                        let file_id = format!("file:{}", path);
                        if file_node_ids.contains_key(&path) {
                            edges.push(EdgeData {
                                from: file_id,
                                to: unit_id.clone(),
                            });
                            continue;
                        }
                        let discovered_flag = if declared_input_paths.contains(&path) {
                            None
                        } else {
                            Some(true)
                        };
                        nodes.push(NodeData {
                            id: file_id.clone(),
                            kind: "file".to_string(),
                            label: file_label(&path),
                            recipe: None,
                            command: None,
                            output: None,
                            cached: None,
                            dep_kind: None,
                            group_index: None,
                            modified: None,
                            discovered: discovered_flag,
                        });
                        file_node_ids.insert(path.clone(), file_id.clone());
                        edges.push(EdgeData {
                            from: file_id,
                            to: unit_id.clone(),
                        });
                    }
                }
            }

            // --- Probe → consumer edges (from this unit's `probes` field) ---
            // Mirror of the engine's CapturedUnit.probes wiring. Works for
            // any consumer kind (including a probe whose `inputs.requires`
            // names another probe in the same recipe).
            for req_key in &unit.probes {
                if let Some(probe_uid) = probe_unit_id_by_key.get(req_key) {
                    edges.push(EdgeData {
                        from: probe_uid.clone(),
                        to: unit_id.clone(),
                    });
                }
            }

            // --- Barrier / intra-recipe unit→unit edges ---
            // Probes never read or advance the barrier (mirrors dag_builder.rs):
            // they are pure fact-gathering, ordered only via `inputs.requires`.
            if is_probe {
                // Still record the recipe root if we haven't yet, so the wave
                // visualisation has a reasonable entry point.
                if !recipe_root_recorded {
                    recipe_roots
                        .entry(recipe_name.clone())
                        .or_insert_with(|| vec![unit_id.clone()]);
                    recipe_root_recorded = true;
                }
                continue;
            }
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

#[cfg(test)]
#[path = "tests/dag_data_tests.rs"]
mod tests;
