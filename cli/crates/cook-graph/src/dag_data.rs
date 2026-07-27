//! The build graph: a flat set of nodes and edges.
//!
//! There are no waves. The engine stopped scheduling in waves at SHI-222
//! Phase 4 — it walks one unified work-unit DAG — and the wave grouping that
//! lingered here was a display construct with no counterpart in execution.
//! Keeping it was actively harmful: it manufactured whole-recipe edges the
//! engine does not impose, which then masked the real per-unit ordering.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use cook_cache::ThreadSafeCacheManager;
use cook_fingerprint::{hash_file, stat_mtime};
use cook_contracts::{DepKind, DiscoveredInputs, RecipeUnits, WorkPayload};
use std::collections::BTreeSet;

use crate::DAG_SCHEMA_VERSION;

// ---------------------------------------------------------------------------
// Graph data model
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct DagData {
    /// Wire-format schema version (CS-0048). Bumped to 3 when the wave
    /// structure was removed — an incompatible structural change, which
    /// CS-0048's evolution policy requires a bump for.
    pub schema_version: u32,
    pub target: String,
    /// Every recipe reachable from the target, in registration order.
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
    /// CS-0171: the unit's recipe-local cache key. This is the join key that
    /// attaches `cook why`'s determinant/tier findings and the retained
    /// timings to this node; `(recipe, cache_key)` identifies a unit across
    /// runs, which the per-run node id does not.
    ///
    /// This crate deliberately computes NO cache verdict of its own. It used
    /// to: a local-index-only `needs_rebuild_cook` call that knew nothing of
    /// the shared tier and rendered as a bare `[cached]`/`[stale]` marker.
    /// Two cache truths in one binary is one too many, and it was the weaker
    /// one. The graph supplies structure; `cook_engine::why` supplies the
    /// verdict. (CS-0171.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
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

/// Why an edge exists — the thing that decides whether it is costing you
/// parallelism, and the question a kindless `{from, to}` pair cannot answer.
///
/// Ordered weakest-to-strongest by how much they constrain scheduling, so
/// aggregation can keep the most constraining kind visible when it merges a
/// bundle of edges between two collapsed nodes (see `Ord`).
#[derive(Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// A declared file input feeding a unit. A real data dependency.
    Data,
    /// A header reached through a `discovered_inputs` depfile.
    Discovered,
    /// A probe value consumed by a unit (§22.5.5).
    Probe,
    /// Entry into a step group: the members are siblings and run in parallel.
    Group,
    /// A §15.1 sequential barrier between units of one recipe.
    Serial,
    /// Per-unit cross-recipe ordering — `cook.dep_order` / `cook.dep_output`
    /// (§22.10). Fine-grained: only the referencing units wait.
    DepOrder,
    /// Whole-recipe barrier — a dep-list entry or `cook.require_recipe`
    /// (§22.8). Every unit of the consumer waits for every unit of the
    /// referent. The most expensive edge in the model, and the one most often
    /// reached for when `DepOrder` would do.
    Barrier,
}

impl EdgeKind {
    /// Short label used by every renderer.
    pub fn label(self) -> &'static str {
        match self {
            EdgeKind::Data => "data",
            EdgeKind::Discovered => "discovered",
            EdgeKind::Probe => "probe",
            EdgeKind::Group => "group",
            EdgeKind::Serial => "serial",
            EdgeKind::DepOrder => "dep_order",
            EdgeKind::Barrier => "barrier",
        }
    }

    /// True when the edge imposes ordering rather than carrying data. These
    /// are the edges worth interrogating when a build is less parallel than
    /// expected.
    pub fn is_ordering(self) -> bool {
        matches!(
            self,
            EdgeKind::Group | EdgeKind::Serial | EdgeKind::DepOrder | EdgeKind::Barrier
        )
    }
}

#[derive(Serialize, Clone)]
pub struct EdgeData {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
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
// Graph builder
// ---------------------------------------------------------------------------

/// Build the graph from registered `RecipeUnits`.
///
/// Two passes, which is what removing waves buys: pass one emits every
/// recipe's nodes and intra-recipe edges and records each recipe's terminal
/// units; pass two wires the cross-recipe edges against those terminals. No
/// ordering of recipes is needed, so no grouping is needed to establish one.
pub fn build_dag_data(
    target: &str,
    all_units: &[(String, RecipeUnits)],
    explicit_deps: &BTreeMap<String, Vec<String>>,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
) -> DagData {
    let units_by_name: BTreeMap<&str, &RecipeUnits> = all_units
        .iter()
        .map(|(name, ru)| (name.as_str(), ru))
        .collect();
    let recipe_names: Vec<String> = all_units.iter().map(|(n, _)| n.clone()).collect();

    // Pass 1: nodes + intra-recipe edges, collecting per-recipe terminals and
    // roots for pass 2.
    let (mut nodes, mut edges, recipe_terminals, recipe_roots) =
        build_nodes(&recipe_names, &units_by_name, cache_managers);

    // Pass 2a: per-unit cross-recipe edges (cook.dep_order / cook.dep_output).
    for recipe_name in &recipe_names {
        let Some(ru) = units_by_name.get(recipe_name.as_str()) else {
            continue;
        };
        for (unit_idx, dep_recipe) in &ru.dep_edges {
            let unit_id = format!("unit:{}:{}", recipe_name, unit_idx);
            let Some(terminals) = recipe_terminals.get(dep_recipe) else {
                continue;
            };
            for terminal in terminals {
                edges.push(EdgeData {
                    from: terminal.clone(),
                    to: unit_id.clone(),
                    kind: EdgeKind::DepOrder,
                });
            }
        }
    }

    // Pass 2b: whole-recipe edges, for each explicit dep A -> B, connecting
    // the terminals of B to the roots of A.
    //
    // Only where the dependency is NOT already fine-covered. A recipe-level
    // `requires` edge that a module has covered with per-unit `cook.dep_order`
    // references (CS-0161) does not execute as a whole-recipe barrier — the
    // consumer's units are free to run in one flat wave with the upstream's,
    // and only the units that named the upstream actually wait. Those fine
    // edges are already in the graph, emitted by pass 2a above.
    //
    // Emitting the coarse edge anyway would be worse than redundant: it
    // outnumbers the fine edges (one per consumer root, so ~1685 to 1 on a
    // large C++ target) and, once aggregated, would mask them entirely — the
    // tool would report a barrier that the engine does not impose. So the
    // coarse edge is emitted only when nothing fine-covers it, which makes its
    // presence meaningful: a `barrier` in the output is a real whole-recipe
    // barrier, and the absence of one is a dependency that was fine-covered.
    for (recipe, deps) in explicit_deps {
        let Some(roots) = recipe_roots.get(recipe) else {
            continue;
        };
        let fine_covered: BTreeSet<&str> = units_by_name
            .get(recipe.as_str())
            .map(|ru| ru.dep_edges.iter().map(|(_, dep)| dep.as_str()).collect())
            .unwrap_or_default();
        for dep in deps {
            if fine_covered.contains(dep.as_str()) {
                continue;
            }
            let Some(terminals) = recipe_terminals.get(dep) else {
                continue;
            };
            for terminal in terminals {
                for root in roots {
                    edges.push(EdgeData {
                        from: terminal.clone(),
                        to: root.clone(),
                        // An explicit dep-list edge between recipes: every unit
                        // of the consumer waits for every unit of the referent.
                        kind: EdgeKind::Barrier,
                    });
                }
            }
        }
    }

    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    DagData {
        schema_version: DAG_SCHEMA_VERSION,
        target: target.to_string(),
        recipes: recipe_names,
        nodes,
        edges,
    }
}

/// Pass 1: emit every recipe's nodes and intra-recipe edges.
///
/// Returns the nodes, the intra-recipe edges, and per-recipe terminal and
/// root unit IDs for pass 2 to wire cross-recipe edges against.
#[allow(clippy::type_complexity)]
fn build_nodes(
    recipe_names: &[String],
    units_by_name: &BTreeMap<&str, &RecipeUnits>,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
) -> (
    Vec<NodeData>,
    Vec<EdgeData>,
    BTreeMap<String, Vec<String>>,
    BTreeMap<String, Vec<String>>,
) {
    let mut nodes: Vec<NodeData> = Vec::new();
    let mut edges: Vec<EdgeData> = Vec::new();

    // Deduplicated file nodes: path → node id.
    let mut file_node_ids: BTreeMap<String, String> = BTreeMap::new();

    // Preflight in a single pass: gather every unit's declared inputs and
    // outputs across the whole graph.
    //
    // - `producer_by_output` maps each declared output path to the unit that
    //   produces it. An input matching one of these is an intermediate
    //   artifact (a .o consumed by an archive step, say) and gets a direct
    //   producer→consumer edge rather than a file node.
    // - `declared_input_paths` lets the discovered-inputs emission classify a
    //   path as declared regardless of the order units are processed in
    //   (spec §3.3): a path declared by *any* unit is rendered declared, even
    //   if a depfile sees it first.
    //
    // CS-0169 makes a literal output path declarable by at most one unit per
    // run, so this map cannot lose a producer to a collision.
    let mut producer_by_output: BTreeMap<String, String> = BTreeMap::new();
    let mut declared_input_paths: BTreeSet<String> = BTreeSet::new();
    for recipe_name in recipe_names {
        if let Some(ru) = units_by_name.get(recipe_name.as_str()) {
            for (unit_idx, unit) in ru.units.iter().enumerate() {
                if let Some(meta) = &unit.cache_meta {
                    for out in &meta.output_paths {
                        producer_by_output
                            .insert(out.clone(), format!("unit:{}:{}", recipe_name, unit_idx));
                    }
                    for inp in &meta.inputs {
                        declared_input_paths.insert(inp.path.clone());
                    }
                }
            }
        }
    }

    // Per-recipe terminal unit node IDs (final barrier after processing units).
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
                // `DepKind` is `#[non_exhaustive]`; surface unknown future
                // variants to the viewer as a generic label so the UI doesn't
                // silently drop them.
                _ => ("unknown".to_string(), None),
            };

            // --- Unit node ---
            //
            // No cache verdict is computed here. The local-index-only check
            // that used to live at this point knew nothing about the shared
            // tier and could not re-fold a sealed unit's probe values, so it
            // answered a strictly weaker question than `cook_engine::why`
            // while looking like the same answer. All this node carries now is
            // the key to join the real verdict on. (CS-0171.)
            nodes.push(NodeData {
                id: unit_id.clone(),
                kind: "unit".to_string(),
                label,
                recipe: Some(recipe_name.clone()),
                command: Some(command),
                output: output.clone(),
                cache_key: unit.cache_meta.as_ref().map(|m| m.cache_key.clone()),
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

                // Issue 3: Deduplicate the declared entries before iterating
                // to avoid duplicate edges. A pattern entry is drawn as what it
                // is — the declaration — rather than as its expansion: the
                // graph reports what the unit declared (§17.1.1.2).
                let unique_paths: BTreeSet<&String> =
                    meta.inputs.iter().map(|e| &e.path).collect();

                for path in unique_paths {
                    // An input that another unit produces is an intermediate
                    // artifact, not a source. Emit the producer→consumer edge
                    // directly rather than interposing a file node.
                    //
                    // This edge used to be dropped entirely, on the assumption
                    // that the intra-recipe barrier already implied it. It does
                    // not: the barrier expresses ordering within one recipe,
                    // while this expresses data flow, including across recipes
                    // where no barrier exists. Without it the graph could not
                    // say which units a rebuild actually invalidates, which is
                    // what cascade attribution needs (§17.1.6.3, CS-0171).
                    if let Some(producer) = producer_by_output.get(path.as_str()) {
                        if producer != &unit_id {
                            edges.push(EdgeData {
                                from: producer.clone(),
                                to: unit_id.clone(),
                                kind: EdgeKind::Data,
                            });
                        }
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
                            cache_key: None,
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
                        kind: EdgeKind::Data,
                    });
                }
            }

            // --- Discovered file nodes + file→unit edges ---
            // Read the depfile if the unit declares discovered_inputs.
            // Errors (missing/malformed) downgrade to empty list — viewer
            // remains rendering-correct on stale or absent state.
            if let Some(meta) = &unit.cache_meta {
                if let Some(di) = &meta.discovered_inputs {
                    let source = meta.inputs.first().map(|e| e.path.as_str());
                    let discovered = read_discovered_paths(di, source, &ru.working_dir);
                    for path in discovered {
                        // Same as the declared-input case above: a discovered
                        // path that another unit produces is a generated header,
                        // and the producer→consumer edge is the honest rendering
                        // of that dependency.
                        if let Some(producer) = producer_by_output.get(path.as_str()) {
                            if producer != &unit_id {
                                edges.push(EdgeData {
                                    from: producer.clone(),
                                    to: unit_id.clone(),
                                    kind: EdgeKind::Discovered,
                                });
                            }
                            continue;
                        }
                        let file_id = format!("file:{}", path);
                        if file_node_ids.contains_key(&path) {
                            edges.push(EdgeData {
                                from: file_id,
                                to: unit_id.clone(),
                                kind: EdgeKind::Discovered,
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
                            cache_key: None,
                            dep_kind: None,
                            group_index: None,
                            modified: None,
                            discovered: discovered_flag,
                        });
                        file_node_ids.insert(path.clone(), file_id.clone());
                        edges.push(EdgeData {
                            from: file_id,
                            to: unit_id.clone(),
                            kind: EdgeKind::Discovered,
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
                        kind: EdgeKind::Probe,
                    });
                }
            }

            // --- Barrier / intra-recipe unit→unit edges ---
            // Probes never read or advance the barrier (mirrors dag_builder.rs):
            // they are pure fact-gathering, ordered only via `inputs.requires`.
            if is_probe {
                // Still record the recipe root if we haven't yet, so the
                // recipe has a reasonable entry point.
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
                            kind: EdgeKind::Serial,
                        });
                    }
                    barrier = vec![unit_id.clone()];
                }
                DepKind::StepGroup(gi) => {
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
                                    kind: EdgeKind::Group,
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
                            kind: EdgeKind::Serial,
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

    }

    (nodes, edges, recipe_terminals, recipe_roots)
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
