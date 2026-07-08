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
        infos.insert(
            name.name.clone(),
            RecipeInfo {
                ingredients: vec![],
                serves,
                requires: name.requires.clone(),
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
mod tests {
    use super::*;
    use crate::registered_workspace::RegisteredWorkspace;
    use cook_contracts::RecipeUnits;
    use cook_register::{RecipeKind, RegisteredRecipePub, RegistrationSource};
    use std::path::PathBuf;

    fn make_name(name: &str, requires: &[&str]) -> RegisteredRecipePub {
        RegisteredRecipePub {
            name: name.to_string(),
            source: RegistrationSource::Static { line: 1 },
            kind: RecipeKind::Recipe,
            requires: requires.iter().map(|s| s.to_string()).collect(),
            params: Vec::new(),
        }
    }

    fn empty_ws() -> RegisteredWorkspace {
        RegisteredWorkspace {
            names: Vec::new(),
            units_by_recipe: BTreeMap::new(),
            probes: BTreeMap::new(),
            final_env_by_cookfile: BTreeMap::new(),
            working_dir_by_prefix: BTreeMap::new(),
            alias_dirs_by_prefix: BTreeMap::new(),
            terminal_outputs: BTreeMap::new(),
        }
    }

    #[test]
    fn empty_workspace_yields_empty_map() {
        let ws = empty_ws();
        let infos = build_recipe_infos_from_registered(&ws);
        assert!(infos.is_empty());
    }

    #[test]
    fn surface_recipe_populates_serves_and_requires() {
        let mut ws = empty_ws();
        ws.names.push(make_name("build", &["compile"]));
        ws.units_by_recipe.insert(
            "build".to_string(),
            RecipeUnits {
                recipe_name: "build".to_string(),
                deps: vec!["compile".to_string()],
                units: Vec::new(),
                step_groups: Vec::new(),
                working_dir: PathBuf::from("/tmp"),
                env_vars: BTreeMap::new(),
                terminal_outputs: vec!["build/app".to_string()],
                dep_edges: Vec::new(),
                probes: Vec::new(),
            },
        );

        let infos = build_recipe_infos_from_registered(&ws);
        let info = infos.get("build").expect("build present");
        assert_eq!(info.serves, vec!["build/app".to_string()]);
        assert_eq!(info.requires, vec!["compile".to_string()]);
        assert!(info.ingredients.is_empty());
    }

    #[test]
    fn dynamic_recipe_with_no_units_has_empty_serves() {
        // Dynamic recipes (e.g. cook_cc.bin) register a name but may not
        // produce a RecipeUnits entry; build_recipe_infos_from_registered
        // must tolerate that and fall back to requires-only.
        let mut ws = empty_ws();
        ws.names.push(make_name("cc_bin", &["compile"]));
        let infos = build_recipe_infos_from_registered(&ws);
        let info = infos.get("cc_bin").expect("cc_bin present");
        assert!(info.serves.is_empty());
        assert_eq!(info.requires, vec!["compile".to_string()]);
    }
}
