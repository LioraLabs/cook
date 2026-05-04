//! Build `RecipeInfo` maps for the analyzer.
//!
//! `RecipeInfo` is the analyzer's view of each recipe (ingredients, served
//! outputs, explicit `requires` deps). The two helpers here translate from
//! parser-level AST and `Workspace` types into the form `analyzer::dependency_edges_*`
//! and `run::run` consume.

use std::collections::BTreeMap;

use cook_lang::ast::Cookfile;

use crate::analyzer::{self, RecipeInfo, WorkspaceLayout};

use super::workspace::Workspace;

/// Build recipe_infos from a single Cookfile's recipes and chores.
///
/// Chores are registered as recipes with no ingredients and no cook outputs
/// (they never produce cached artifacts). The engine sees them as ordinary
/// recipes; the chore contract (interactive-only, cache=false) is enforced
/// at codegen time by `compile_chore` (cook-luagen).
pub fn build_single_recipe_infos(cookfile: &Cookfile) -> BTreeMap<String, RecipeInfo> {
    let mut recipe_infos = BTreeMap::new();
    for recipe in &cookfile.recipes {
        recipe_infos.insert(
            recipe.name.clone(),
            RecipeInfo {
                ingredients: recipe.ingredients.clone(),
                serves: recipe
                    .steps
                    .iter()
                    .flat_map(|s| {
                        if let cook_lang::ast::Step::Cook { step, .. } = s {
                            step.outputs.clone()
                        } else {
                            Vec::new()
                        }
                    })
                    .collect(),
                requires: recipe.deps.clone(),
            },
        );
    }
    // Chores have no ingredients or cook outputs; their deps are explicit only.
    for chore in &cookfile.chores {
        recipe_infos.insert(
            chore.name.clone(),
            RecipeInfo {
                ingredients: vec![],
                serves: vec![],
                requires: chore.deps.clone(),
            },
        );
    }
    recipe_infos
}

/// Build a `WorkspaceLayout` snapshot from a `Workspace`.
///
/// This is the anti-corruption layer: pipeline owns workspace discovery and
/// loading; the analyzer owns namespace resolution and dependency analysis.
/// The layout carries just enough data (recipe names, dep lists, namespace map)
/// for the analyzer to do its job.
fn workspace_to_layout(workspace: &Workspace) -> WorkspaceLayout {
    let root_dir = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());

    // Chores are first-class peers of recipes from the engine's POV: they
    // carry a name and a deps list; cross-form deps work transparently.
    // Merge both into the layout's name→deps tables.
    let root_recipes: Vec<(String, Vec<String>)> = workspace
        .root
        .cookfile
        .recipes
        .iter()
        .map(|r| (r.name.clone(), r.deps.clone()))
        .chain(
            workspace
                .root
                .cookfile
                .chores
                .iter()
                .map(|c| (c.name.clone(), c.deps.clone())),
        )
        .collect();

    let imported_recipes: Vec<(std::path::PathBuf, Vec<(String, Vec<String>)>)> = workspace
        .imports
        .iter()
        .map(|(canonical_path, loaded)| {
            let recipes: Vec<(String, Vec<String>)> = loaded
                .cookfile
                .recipes
                .iter()
                .map(|r| (r.name.clone(), r.deps.clone()))
                .chain(
                    loaded
                        .cookfile
                        .chores
                        .iter()
                        .map(|c| (c.name.clone(), c.deps.clone())),
                )
                .collect();
            (canonical_path.clone(), recipes)
        })
        .collect();

    WorkspaceLayout {
        root_dir,
        root_recipes,
        imported_recipes,
        namespace_map: workspace.namespace_map.clone(),
    }
}

/// Build workspace recipe info and resolve via the analyzer.
pub fn build_workspace_recipe_info(workspace: &Workspace) -> BTreeMap<String, RecipeInfo> {
    let layout = workspace_to_layout(workspace);
    analyzer::build_workspace_recipe_info(&layout)
}

/// Find the full dotted prefix for a canonical import path.
/// Delegates to the analyzer.
pub fn find_full_prefix(workspace: &Workspace, canonical_path: &std::path::Path) -> String {
    let root_dir = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    analyzer::find_full_prefix(&workspace.namespace_map, &root_dir, canonical_path)
}
