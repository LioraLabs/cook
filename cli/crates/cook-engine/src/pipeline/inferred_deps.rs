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
//! one of the two helpers below — passing an empty map silently drops
//! the §{xref.dep-implications} contract.

use std::collections::{BTreeMap, BTreeSet};

use cook_lang::ast::Cookfile;

use super::recipe_info::find_full_prefix;
use super::workspace::Workspace;

/// Compute inferred dependencies from `{NAME}` body refs in a single Cookfile.
///
/// Returns a `BTreeMap` keyed by recipe name, valued by a sorted-deduplicated
/// vector of dep recipe names (no namespace prefixes — this is the single-file
/// case).
pub fn compute_single_inferred_deps(cookfile: &Cookfile) -> BTreeMap<String, Vec<String>> {
    let recipe_names = cook_luagen::dep_ref::extract_recipe_names(cookfile);
    let mut inferred: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for recipe in &cookfile.recipes {
        let refs = cook_luagen::dep_ref::extract_dep_refs(recipe, &recipe_names);
        let dep_names: Vec<String> = refs
            .iter()
            .map(|r| r.recipe_name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !dep_names.is_empty() {
            inferred.insert(recipe.name.clone(), dep_names);
        }
    }
    inferred
}

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

/// Detect "explicit + inferred dep on the same name" conflicts in a single
/// Cookfile and return them as warning strings (one per offending pair).
///
/// Returning strings (rather than printing to stderr) keeps the engine free
/// of human-output formatting decisions; the CLI prints them with its
/// preferred prefix.
pub fn single_dep_conflicts(cookfile: &Cookfile) -> Vec<String> {
    let recipe_names = cook_luagen::dep_ref::extract_recipe_names(cookfile);
    let mut warnings = Vec::new();
    for recipe in &cookfile.recipes {
        let refs = cook_luagen::dep_ref::extract_dep_refs(recipe, &recipe_names);
        for dep_ref in &refs {
            if recipe.deps.contains(&dep_ref.recipe_name) {
                warnings.push(format!(
                    "recipe '{}' has both explicit ': {}' and inferred '$<{}>' dependency — conflicting scheduling intent",
                    recipe.name, dep_ref.recipe_name, dep_ref.recipe_name
                ));
            }
        }
    }
    warnings
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
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Helper: write minimal Cookfile content and return the workspace.
    fn make_workspace(
        root_cookfile: &str,
        imports: &[(&str, &str)], // (dir_name, cookfile_content)
    ) -> (TempDir, Workspace) {
        let dir = TempDir::new().unwrap();
        // Write sub-Cookfiles first.
        for (sub_dir, content) in imports {
            fs::create_dir_all(dir.path().join(sub_dir)).unwrap();
            fs::write(dir.path().join(sub_dir).join("Cookfile"), content).unwrap();
        }
        fs::write(dir.path().join("Cookfile"), root_cookfile).unwrap();
        fs::write(dir.path().join(".cookroot"), "").unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = Workspace::load(&entry, &root, &[]).unwrap();
        (dir, ws)
    }

    /// Tree-relative case: root has `recipe top` referencing `$<lib.lib_build>` in
    /// its body, lib has `recipe lib_build`.
    /// Expected: `{"top" -> ["lib.lib_build"]}`.
    #[test]
    fn workspace_inferred_deps_tree_relative() {
        let (_dir, ws) = make_workspace(
            "import lib ./lib\nrecipe top\n    cook \"build/top\" using { echo $<lib.lib_build> }\n",
            &[("lib", "recipe lib_build\n    cook \"lib.o\" using { echo $<out> }\n")],
        );
        let deps = compute_workspace_inferred_deps(&ws);
        assert_eq!(
            deps.get("top"),
            Some(&vec!["lib.lib_build".to_string()]),
            "expected top -> [lib.lib_build], got: {deps:?}"
        );
        // lib_build has no body refs → not in the map.
        assert!(deps.get("lib.lib_build").is_none());
    }

    /// Sigil case: root imports `apps/web` tree-relatively AND imports `core/lib`
    /// directly via sigil (`//core/lib`).  `apps/web` also imports `core/lib` via
    /// sigil.  This is a diamond: `core/lib` appears once in workspace.imports but
    /// is reachable from both root (as `core`) and web (as `core`).
    #[test]
    fn workspace_inferred_deps_sigil_alias_resolves_to_importee_prefix() {
        let dir = TempDir::new().unwrap();
        // core/lib Cookfile
        fs::create_dir_all(dir.path().join("core/lib")).unwrap();
        fs::write(
            dir.path().join("core/lib/Cookfile"),
            "recipe core_lib\n    cook \"core.o\" using { echo $<out> }\n",
        )
        .unwrap();
        // apps/web Cookfile — imports core via sigil, refs $<core.core_lib>
        fs::create_dir_all(dir.path().join("apps/web")).unwrap();
        fs::write(
            dir.path().join("apps/web/Cookfile"),
            "import core //core/lib\nrecipe web_app\n    cook \"web.o\" using { echo $<core.core_lib> }\n",
        )
        .unwrap();
        // root Cookfile: imports BOTH web (tree) AND core (sigil) directly.
        fs::write(
            dir.path().join("Cookfile"),
            "import web ./apps/web\nimport core //core/lib\nrecipe top\n    cook \"build/top\" using { echo $<web.web_app> $<core.core_lib> }\n",
        )
        .unwrap();
        fs::write(dir.path().join(".cookroot"), "").unwrap();

        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = Workspace::load(&entry, &root, &[]).unwrap();
        let deps = compute_workspace_inferred_deps(&ws);

        assert_eq!(
            deps.get("web.web_app"),
            Some(&vec!["core.core_lib".to_string()]),
            "web_app should have dep on core.core_lib (importee workspace prefix), got: {deps:?}"
        );
        assert_eq!(
            deps.get("top"),
            Some(&vec!["core.core_lib".to_string(), "web.web_app".to_string()]),
            "top should have deps on web.web_app and core.core_lib, got: {deps:?}"
        );
    }

    /// Empty case: workspace where no recipes have body refs returns empty map.
    #[test]
    fn workspace_inferred_deps_empty_when_no_body_refs() {
        let (_dir, ws) = make_workspace(
            "import lib ./lib\nrecipe top\n    echo hello\n",
            &[("lib", "recipe lib_build\n    echo world\n")],
        );
        let deps = compute_workspace_inferred_deps(&ws);
        assert!(
            deps.is_empty(),
            "expected empty inferred_deps when no body refs, got: {deps:?}"
        );
    }

    /// Single-Cookfile case: a recipe whose body references `{prepare}` should
    /// produce `{"verify" -> ["prepare"]}`.
    #[test]
    fn single_inferred_deps_body_ref_produces_edge() {
        let src = "recipe prepare\n    cook \"prepare.out\" using { echo $<out> }\nrecipe verify\n    test { echo $<prepare> }\n";
        let cf = cook_lang::parse(src).unwrap();
        let deps = compute_single_inferred_deps(&cf);
        assert_eq!(
            deps.get("verify"),
            Some(&vec!["prepare".to_string()]),
            "expected verify -> [prepare], got: {deps:?}"
        );
        assert!(deps.get("prepare").is_none());
    }

    /// Empty case: a single Cookfile with no body refs returns an empty map.
    #[test]
    fn single_inferred_deps_empty_when_no_body_refs() {
        let cf = cook_lang::parse("recipe a\n    echo hi\nrecipe b\n    echo bye\n").unwrap();
        let deps = compute_single_inferred_deps(&cf);
        assert!(deps.is_empty(), "expected empty, got: {deps:?}");
    }

    /// Diagnostic text guard: when a recipe declares both an explicit `: dep`
    /// and an inferred `$<dep>` body reference for the same name, the warning
    /// must spell the inferred form using the current `$<...>` sigil syntax,
    /// not the legacy `{...}` curly-brace form. The book and Standard teach
    /// `$<...>`; the diagnostic must match.
    #[test]
    fn single_dep_conflict_uses_sigil_placeholder_syntax() {
        let src = "recipe compile\n    cook \"compile.out\" using { echo hi > $<out> }\nrecipe link: compile\n    cook \"link.out\" using { echo $<compile> > $<out> }\n";
        let cf = cook_lang::parse(src).unwrap();
        let warnings = single_dep_conflicts(&cf);
        assert_eq!(warnings.len(), 1, "expected exactly one warning, got: {warnings:?}");
        let w = &warnings[0];
        assert!(
            w.contains("inferred '$<compile>'"),
            "warning must spell the inferred ref with $<...> sigil syntax, got: {w}"
        );
        assert!(
            !w.contains("'{compile}'"),
            "warning must not use the legacy {{NAME}} placeholder form, got: {w}"
        );
        assert_eq!(
            w,
            "recipe 'link' has both explicit ': compile' and inferred '$<compile>' dependency — conflicting scheduling intent"
        );
    }
}
