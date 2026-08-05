//! Compute `{NAME}` body-reference inferred dependencies.
//!
//! `{NAME}` references in a recipe body are an alternative to explicit
//! `: dep` declarations (Cook Standard § 5.3 / App. E.10). They produce
//! an inferred dep edge from the consumer recipe to the named recipe;
//! the engine consumes these via `run::run`'s `inferred_deps` parameter.
//!
//! Unlike explicit deps (which create wave boundaries), inferred deps
//! cause same-wave merging in the wave-grouper. Every CLI command path
//! that invokes `run::run` MUST pass an inferred-deps map produced by
//! [`compute_workspace_inferred_deps`] — passing an empty map silently
//! drops the §{xref.dep-implications} contract. A single-Cookfile project
//! (no imports) is a workspace of one: the same workspace helpers apply,
//! with the root registering under the empty qualified prefix.

use std::collections::{BTreeMap, BTreeSet};

use cook_lang::ast::Cookfile;

use super::recipe_info::find_full_prefix;
use super::workspace::Workspace;

/// Compute inferred dependencies from `{alias.recipe}` body refs across the
/// entire workspace (§7.3 union).
///
/// Returns a `BTreeMap<String, Vec<String>>` keyed by **qualified consumer name**
/// (e.g. `"top"` for a root recipe, `"web.web_obj"` for an imported one), valued
/// by a sorted-deduplicated vector of **qualified dep names**.
pub fn compute_workspace_inferred_deps(workspace: &Workspace) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();

    // Build a canonical-path → &Cookfile snapshot for alias resolution.
    let root_canon = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    let mut canon_to_cookfile: BTreeMap<std::path::PathBuf, &Cookfile> = BTreeMap::new();
    canon_to_cookfile.insert(root_canon.clone(), &workspace.root.cookfile);
    for (canon, loaded) in &workspace.imports {
        canon_to_cookfile.insert(canon.clone(), &loaded.cookfile);
    }

    // Collect all (canon_path, qualified_prefix, &Cookfile) triples.
    // Root has empty prefix; each import has a dotted prefix computed via find_full_prefix.
    let entries: Vec<(std::path::PathBuf, String, &Cookfile)> =
        std::iter::once((root_canon.clone(), String::new(), &workspace.root.cookfile))
            .chain(workspace.imports.iter().map(|(canon, loaded)| {
                let prefix = find_full_prefix(workspace, canon);
                (canon.clone(), prefix, &loaded.cookfile)
            }))
            .collect();

    for (cookfile_canon, prefix, cookfile) in &entries {
        // For this Cookfile, build two maps keyed by local alias:
        //   alias_to_importee_prefix: alias → qualified prefix of the importee
        //   imports_by_alias:         alias → &Cookfile of the importee
        // Used to resolve `{alias.recipe}` tokens.
        let mut alias_to_importee_prefix: BTreeMap<String, String> = BTreeMap::new();
        let mut imports_by_alias: BTreeMap<String, &Cookfile> = BTreeMap::new();
        for (parent_canon, alias, target_canon) in &workspace.namespace_map {
            if parent_canon != cookfile_canon {
                continue;
            }
            let importee_prefix = find_full_prefix(workspace, target_canon);
            alias_to_importee_prefix.insert(alias.clone(), importee_prefix);
            if let Some(cf) = canon_to_cookfile.get(target_canon) {
                imports_by_alias.insert(alias.clone(), cf);
            }
        }

        // Build the §7.3 union: local recipe names ∪ {alias.recipe} pairs for
        // direct imports.  This is what extract_dep_refs uses to distinguish
        // recipe references from env-var tokens.
        let union = cook_luagen::dep_ref::extract_recipe_names_with_imports(
            cookfile,
            &imports_by_alias,
        );

        for recipe in &cookfile.recipes {
            let refs = cook_luagen::dep_ref::extract_dep_refs(recipe, &union);
            if refs.is_empty() {
                continue;
            }

            // Qualify the consumer name.
            let consumer = if prefix.is_empty() {
                recipe.name.clone()
            } else {
                format!("{prefix}.{}", recipe.name)
            };

            let mut deps_set: BTreeSet<String> = BTreeSet::new();
            for dep_ref in refs {
                // dep_ref.recipe_name is either:
                //   "local_recipe"    — same-Cookfile reference (no dot)
                //   "alias.recipe"    — cross-Cookfile reference via local alias
                let qualified = if let Some((alias, sub)) = dep_ref.recipe_name.split_once('.') {
                    // Cross-Cookfile: resolve alias → importee's qualified prefix.
                    if let Some(importee_prefix) = alias_to_importee_prefix.get(alias) {
                        if importee_prefix.is_empty() {
                            sub.to_string()
                        } else {
                            format!("{importee_prefix}.{sub}")
                        }
                    } else {
                        // Should not happen if the union was built correctly;
                        // skip defensively.
                        continue;
                    }
                } else if prefix.is_empty() {
                    // Same-Cookfile, root: no prefix needed.
                    dep_ref.recipe_name.clone()
                } else {
                    // Same-Cookfile, imported: prepend the Cookfile's prefix.
                    format!("{prefix}.{}", dep_ref.recipe_name)
                };
                deps_set.insert(qualified);
            }

            if !deps_set.is_empty() {
                out.insert(consumer, deps_set.into_iter().collect());
            }
        }
    }

    out
}

/// Detect "explicit + inferred dep on the same name" conflicts across an
/// entire workspace and return them as warning strings.
pub fn workspace_dep_conflicts(
    workspace: &Workspace,
    inferred_deps: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    for recipe in &workspace.root.cookfile.recipes {
        if let Some(dep_list) = inferred_deps.get(&recipe.name) {
            for inferred_dep in dep_list {
                if recipe.deps.contains(inferred_dep) {
                    warnings.push(format!(
                        "recipe '{}' has both explicit ': {}' and inferred '$<{}>' dependency — conflicting scheduling intent",
                        recipe.name, inferred_dep, inferred_dep
                    ));
                }
            }
        }
    }
    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        for recipe in &loaded.cookfile.recipes {
            let qualified_consumer = format!("{prefix}.{}", recipe.name);
            if let Some(dep_list) = inferred_deps.get(&qualified_consumer) {
                for inferred_dep in dep_list {
                    if recipe.deps.contains(inferred_dep) {
                        warnings.push(format!(
                            "recipe '{}' has both explicit ': {}' and inferred '$<{}>' dependency — conflicting scheduling intent",
                            qualified_consumer, inferred_dep, inferred_dep
                        ));
                    }
                }
            }
        }
    }
    warnings
}

#[cfg(test)]
#[path = "tests/inferred_deps_tests.rs"]
mod tests;
