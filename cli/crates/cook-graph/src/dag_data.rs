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
use cook_cache::{hash_file, stat_mtime};
use cook_contracts::unit_graph;
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
/// The unit-to-unit wiring — serial barriers, group entry, probe consumption,
/// coarse whole-recipe barriers, fine `dep_order` refs, and leaf pass-through
/// across unit-less meta-targets — is NOT derived here. It is
/// [`cook_contracts::unit_graph::plan`], the same law the engine lowers into
/// the DAG it executes, so the graph reports the edges the scheduler imposes
/// by construction. This crate used to hand-mirror those rules and drifted
/// twice (COOK-402): it kept the fine-covered narrowing rule the Standard
/// withdrew (CS-0161, "Rejected alternative" — the shipped design is strictly
/// additive), and it dropped every dependency routed through a unit-less
/// meta-target because it recorded no terminals for an empty barrier.
///
/// What remains derived here is the display layer the plan does not carry:
/// file nodes, declared/discovered input edges, producer→consumer data edges,
/// labels, and input staleness.
pub fn build_dag_data(
    target: &str,
    all_units: &[(String, RecipeUnits)],
    explicit_deps: &BTreeMap<String, Vec<String>>,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
) -> Result<DagData, unit_graph::UnitGraphError> {
    let units_by_name: BTreeMap<&str, &RecipeUnits> = all_units
        .iter()
        .map(|(name, ru)| (name.as_str(), ru))
        .collect();
    let recipe_names: Vec<String> = all_units.iter().map(|(n, _)| n.clone()).collect();

    // Display pass: unit and file nodes, plus the data/discovered edges.
    let (mut nodes, mut edges) = build_nodes(&recipe_names, &units_by_name, cache_managers);

    // Structural pass: the shared wiring law, on a slice prepared exactly the
    // way the engine's run path prepares it — topologically ordered (plan's
    // intra-call leaves accumulator needs every dep walked first) with coarse
    // deps restricted to the declared `requires` (the closure map merges
    // `orders` in; see `declared_coarse_deps`).
    let reachable: BTreeSet<String> = recipe_names.iter().cloned().collect();
    let mut topo_edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, ru) in all_units {
        let entry = topo_edges.entry(name.clone()).or_default();
        if let Some(deps) = explicit_deps.get(name) {
            entry.extend(deps.iter().cloned());
        }
        entry.extend(ru.deps.iter().cloned());
        entry.extend(ru.dep_edges.iter().map(|(_, dep)| dep.clone()));
    }
    let topo = unit_graph::toposort_recipes(&topo_edges, &reachable)?;
    let slice: Vec<RecipeUnits> = topo
        .iter()
        .map(|name| {
            let ru = units_by_name
                .get(name.as_str())
                .expect("topo order is a permutation of all_units");
            let mut u = (*ru).clone();
            // The plan reports origins under `recipe_name`; the display
            // nodes above are keyed by the workspace name. Force agreement.
            u.recipe_name = name.clone();
            if let Some(deps) = explicit_deps.get(name) {
                u.deps = unit_graph::declared_coarse_deps(&ru.deps, deps);
            }
            u
        })
        .collect();
    let graph = unit_graph::plan(&slice)?;

    for node in &graph.nodes {
        // Probes synthesised from register-block metadata have no captured
        // unit and therefore no display node; edges touching them are not
        // renderable yet. (Follow-up candidate: give them nodes.)
        let Some(to) = origin_node_id(&node.origin) else {
            continue;
        };
        for &(from_idx, kind) in &node.deps {
            let Some(from) = origin_node_id(&graph.nodes[from_idx].origin) else {
                continue;
            };
            edges.push(EdgeData { from, to: to.clone(), kind: edge_kind(kind) });
        }
    }

    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(DagData {
        schema_version: DAG_SCHEMA_VERSION,
        target: target.to_string(),
        recipes: recipe_names,
        nodes,
        edges,
    })
}

/// The display node id for a planned node's origin, or `None` for node kinds
/// the viewer does not render.
fn origin_node_id(origin: &unit_graph::NodeOrigin) -> Option<String> {
    match origin {
        unit_graph::NodeOrigin::Unit { recipe, unit_idx } => {
            Some(format!("unit:{recipe}:{unit_idx}"))
        }
        unit_graph::NodeOrigin::SynthProbe { .. } => None,
    }
}

fn edge_kind(kind: unit_graph::EdgeProvenance) -> EdgeKind {
    use unit_graph::EdgeProvenance;
    match kind {
        EdgeProvenance::Serial => EdgeKind::Serial,
        EdgeProvenance::Group => EdgeKind::Group,
        EdgeProvenance::Probe => EdgeKind::Probe,
        EdgeProvenance::DepOrder => EdgeKind::DepOrder,
        EdgeProvenance::Barrier => EdgeKind::Barrier,
    }
}

/// Display pass: emit every recipe's unit and file nodes, plus the data and
/// discovered edges. Unit-to-unit wiring is the shared law's
/// (`cook_contracts::unit_graph::plan`) and is deliberately absent here.
fn build_nodes(
    recipe_names: &[String],
    units_by_name: &BTreeMap<&str, &RecipeUnits>,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
) -> (Vec<NodeData>, Vec<EdgeData>) {
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
            cook_contracts::cache::recipe_cache_index_name(ru, recipe_name);
        let recipe_cache = cm.as_ref().map(|mgr| mgr.get_or_load(&cache_index_name));

        for (unit_idx, unit) in ru.units.iter().enumerate() {
            let unit_id = format!("unit:{}:{}", recipe_name, unit_idx);

            // --- Command label ---
            // CS-0191: test-ness is the unit's, not the payload's. Prefixing
            // here keeps the viewer's label unchanged for a test whose body is
            // a command, and finally labels one whose body is Lua — that used
            // to render as an anonymous `lua: …` because the arm below matched
            // only the command form.
            let command = match &unit.payload {
                _ if unit.test_name.is_some() => {
                    format!("test: {}", unit.payload.display_name())
                }
                WorkPayload::Shell { cmd, .. } => cmd.clone(),
                WorkPayload::Interactive { cmd, .. } => format!("@{cmd}"),
                WorkPayload::LuaChunk { code, .. } => {
                    format!("lua: {}", &code[..code.len().min(60)])
                }
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

        }
    }

    (nodes, edges)
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
