//! Build `RecipeInfo` maps for the analyzer.
//!
//! `RecipeInfo` is the analyzer's view of each recipe (ingredients, served
//! outputs, explicit `requires` deps). In the unified register-phase + DAG
//! model (SHI-222 Phase 5, CS-0077) the map is synthesised from a
//! `RegisteredWorkspace` rather than from AST: Lua-registered recipes
//! (`cook_cc.bin`, dynamic chores, …) become first-class members of the
//! dependency graph the engine resolves.
//!
//! Only the workspace-prefix helper [`find_full_prefix`] still operates on
//! `Workspace`; it is consumed by `pipeline::registers`, `pipeline::registries`,
//! and `pipeline::inferred_deps`, all of which walk the namespace map directly
//! rather than the AST.

use std::collections::BTreeMap;

use crate::analyzer::{self, RecipeInfo};

use super::workspace::Workspace;

/// Build recipe_infos from a [`RegisteredWorkspace`].
///
/// `serves` is populated only for surface recipes whose units carry
/// `terminal_outputs`; for dynamic recipes (e.g. `cook_cc.bin`) it is empty
/// and they rely on declared `requires` instead. `ingredients` is intentionally
/// empty — the analyzer-level inference that used to read ingredient lists is
/// obsolete in the unified-DAG world (cross-recipe edges come from
/// `RecipeUnits.dep_edges`, recorded directly by `cook.dep_output` /
/// `cook.add_unit` during the register pass).
pub fn build_recipe_infos_from_registered(
    ws: &crate::registered_workspace::RegisteredWorkspace,
) -> BTreeMap<String, RecipeInfo> {
    let mut infos = BTreeMap::new();
    for name in &ws.names {
        let serves: Vec<String> = ws
            .units_by_recipe
            .get(&name.name)
            .map(|u| u.terminal_outputs.clone())
            .unwrap_or_default();
        // Fine-grained per-unit references establish closure membership too:
        // an author who writes only `cook.dep_order("b")` has named b, and
        // naming is what puts b in the closure (Standard §10.6, CS-0121).
        // These land in `orders`, never in `requires`, so they add a per-unit
        // edge without manufacturing a whole-recipe barrier.
        let orders: Vec<String> = ws
            .units_by_recipe
            .get(&name.name)
            .map(|u| {
                let mut v: Vec<String> = u
                    .dep_edges
                    .iter()
                    .map(|(_, dep)| dep.clone())
                    .filter(|dep| !name.requires.contains(dep))
                    .collect();
                v.sort();
                v.dedup();
                v
            })
            .unwrap_or_default();
        infos.insert(
            name.name.clone(),
            RecipeInfo {
                ingredients: vec![],
                serves,
                requires: name.requires.clone(),
                orders,
            },
        );
    }
    infos
}

/// Find the full dotted prefix for a canonical import path.
/// Delegates to the analyzer.
///
/// Retained as a `Workspace`-keyed convenience wrapper around
/// [`analyzer::find_full_prefix`]. Used by `pipeline::registers` (Phase 5
/// Task 5.1) when qualifying per-import register results, and by the legacy
/// `pipeline::registries` / `pipeline::inferred_deps` paths that still walk
/// `Workspace` directly.
pub fn find_full_prefix(workspace: &Workspace, canonical_path: &std::path::Path) -> String {
    let root_dir = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    analyzer::find_full_prefix(&workspace.namespace_map, &root_dir, canonical_path)
}

#[cfg(test)]
#[path = "tests/recipe_info_tests.rs"]
mod tests;
