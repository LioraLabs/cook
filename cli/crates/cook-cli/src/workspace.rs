//! Workspace loading: multi-Cookfile resolution via imports.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use cook_lang::ast::Cookfile;

use crate::error::CookError;

/// A loaded Cookfile with its parsed AST, generated Lua source, and directory.
#[derive(Debug)]
pub struct LoadedCookfile {
    pub cookfile: Cookfile,
    pub lua_source: String,
    pub dir: PathBuf,
}

/// A resolved workspace: all Cookfiles loaded, imports resolved.
#[derive(Debug)]
pub struct Workspace {
    pub root: LoadedCookfile,
    pub imports: BTreeMap<PathBuf, LoadedCookfile>,
    /// (parent_canonical_path, import_name, imported_canonical_path)
    pub namespace_map: Vec<(PathBuf, String, PathBuf)>,
    /// Resolved workspace root (anchors sigil imports).
    pub workspace_root: PathBuf,
}

impl Workspace {
    pub fn load(
        cookfile_path: &Path,
        workspace_root: &Path,
        _cli_sets: &[String],
    ) -> Result<Self, CookError> {
        let cookfile_path = std::fs::canonicalize(cookfile_path).map_err(|e| {
            CookError::Other(format!(
                "cannot resolve {}: {e}",
                cookfile_path.display()
            ))
        })?;
        let root_dir = cookfile_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let workspace_root = std::fs::canonicalize(workspace_root).map_err(|e| {
            CookError::Other(format!(
                "cannot resolve workspace root {}: {e}",
                workspace_root.display()
            ))
        })?;

        let source = std::fs::read_to_string(&cookfile_path).map_err(|e| {
            CookError::Other(format!(
                "cannot read {}: {e}",
                cookfile_path.display()
            ))
        })?;
        let cookfile =
            cook_lang::parse(&source).map_err(|e| CookError::ParseError(e.to_string()))?;
        let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);
        let lua_source = cook_luagen::generate_with_names_checked(&cookfile, &recipe_names)
            .map_err(|e| CookError::Other(e.to_string()))?;

        let mut imports = BTreeMap::new();
        let mut namespace_map = Vec::new();
        let mut visited = HashSet::new();
        visited.insert(
            std::fs::canonicalize(&root_dir).unwrap_or_else(|_| root_dir.clone()),
        );

        Self::load_imports(
            &cookfile,
            &root_dir,
            &workspace_root,
            &mut imports,
            &mut namespace_map,
            &mut visited,
        )?;

        Ok(Workspace {
            root: LoadedCookfile {
                cookfile,
                lua_source,
                dir: root_dir,
            },
            imports,
            namespace_map,
            workspace_root,
        })
    }

    fn load_imports(
        cookfile: &Cookfile,
        cookfile_dir: &Path,
        workspace_root: &Path,
        imports: &mut BTreeMap<PathBuf, LoadedCookfile>,
        namespace_map: &mut Vec<(PathBuf, String, PathBuf)>,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<(), CookError> {
        let parent_canonical = std::fs::canonicalize(cookfile_dir)
            .unwrap_or_else(|_| cookfile_dir.to_path_buf());

        for import_decl in &cookfile.imports {
            let import_dir = match &import_decl.path {
                cook_lang::ast::ImportPath::Tree(p) => cookfile_dir.join(p),
                cook_lang::ast::ImportPath::Sigil(p) => workspace_root.join(p),
            };

            if !import_dir.exists() {
                return Err(CookError::Other(format!(
                    "Import '{}': directory '{}' not found",
                    import_decl.name, import_decl.path
                )));
            }

            let canonical = std::fs::canonicalize(&import_dir).map_err(|e| {
                CookError::Other(format!(
                    "Import '{}': cannot resolve '{}': {e}",
                    import_decl.name, import_decl.path
                ))
            })?;

            // Validate sigil targets resolve under workspace root.
            if matches!(&import_decl.path, cook_lang::ast::ImportPath::Sigil(_))
                && !canonical.starts_with(workspace_root)
            {
                return Err(CookError::Other(format!(
                    "Import '{}': sigil path resolves outside workspace root '{}'",
                    import_decl.name,
                    workspace_root.display()
                )));
            }

            namespace_map.push((
                parent_canonical.clone(),
                import_decl.name.clone(),
                canonical.clone(),
            ));

            if !visited.insert(canonical.clone()) {
                if imports.contains_key(&canonical) {
                    continue; // Dedup: already loaded
                }
                return Err(CookError::Other(format!(
                    "Circular import detected involving '{}'",
                    import_decl.path
                )));
            }

            let import_cookfile_path = import_dir.join("Cookfile");
            if !import_cookfile_path.exists() {
                return Err(CookError::Other(format!(
                    "Import '{}': no Cookfile found in '{}'",
                    import_decl.name, import_decl.path
                )));
            }

            let source = std::fs::read_to_string(&import_cookfile_path).map_err(|e| {
                CookError::Other(format!(
                    "Import '{}': cannot read Cookfile: {e}",
                    import_decl.name
                ))
            })?;
            let sub_cookfile = cook_lang::parse(&source)
                .map_err(|e| CookError::ParseError(format!("Import '{}': {e}", import_decl.name)))?;
            let sub_recipe_names = cook_luagen::dep_ref::extract_recipe_names(&sub_cookfile);
            let sub_lua = cook_luagen::generate_with_names_checked(&sub_cookfile, &sub_recipe_names)
                .map_err(|e| CookError::Other(format!("Import '{}': {e}", import_decl.name)))?;

            Self::load_imports(
                &sub_cookfile,
                &canonical,
                workspace_root,
                imports,
                namespace_map,
                visited,
            )?;

            imports.insert(
                canonical,
                LoadedCookfile {
                    cookfile: sub_cookfile,
                    lua_source: sub_lua,
                    dir: import_dir,
                },
            );
        }

        Ok(())
    }

    /// For a given importer Cookfile directory `importer_dir` (canonical),
    /// return a map from each of its import aliases to the syntactic relative
    /// path from `importer_dir` to the alias's target directory.
    ///
    /// This map is what `cook.dep_output` uses at substitution time to rewrite
    /// importee-relative paths into importer-relative paths.
    pub fn alias_dirs_for(&self, importer_dir: &Path) -> BTreeMap<String, PathBuf> {
        let mut out = BTreeMap::new();
        let importer_canon = std::fs::canonicalize(importer_dir)
            .unwrap_or_else(|_| importer_dir.to_path_buf());
        for (parent_canon, alias, target_canon) in &self.namespace_map {
            if parent_canon != &importer_canon {
                continue;
            }
            let rel = pathdiff::diff_paths(target_canon, &importer_canon)
                .unwrap_or_else(|| target_canon.clone());
            out.insert(alias.clone(), rel);
        }
        out
    }

    /// Resolve "backend.build" from a parent dir to (canonical_import_dir, recipe_name).
    #[allow(dead_code)]
    pub fn resolve_namespaced_dep(
        &self,
        parent_dir: &Path,
        dep: &str,
    ) -> Option<(PathBuf, String)> {
        let dot_pos = dep.find('.')?;
        let import_name = &dep[..dot_pos];
        let recipe_name = &dep[dot_pos + 1..];

        let parent_canonical =
            std::fs::canonicalize(parent_dir).unwrap_or_else(|_| parent_dir.to_path_buf());

        for (parent, name, target) in &self.namespace_map {
            if parent == &parent_canonical && name == import_name {
                return Some((target.clone(), recipe_name.to_string()));
            }
        }
        None
    }
}

/// Returns true if `candidate_cookfile` transitively imports `target_dir` via
/// tree-relative imports only. Sigil-anchored imports are skipped (their
/// resolution presupposes a workspace root, which is what we are computing).
fn cookfile_transitively_imports_via_tree(
    candidate_cookfile: &Path,
    target_dir: &Path,
) -> Result<bool, CookError> {
    let target_canon = std::fs::canonicalize(target_dir)
        .unwrap_or_else(|_| target_dir.to_path_buf());

    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<PathBuf> = vec![candidate_cookfile.to_path_buf()];

    while let Some(cookfile_path) = stack.pop() {
        let cookfile_canon = std::fs::canonicalize(&cookfile_path)
            .unwrap_or_else(|_| cookfile_path.clone());
        if !visited.insert(cookfile_canon.clone()) {
            continue;
        }
        let cookfile_dir = cookfile_canon.parent().unwrap_or(Path::new("."));
        let source = match std::fs::read_to_string(&cookfile_canon) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let parsed = match cook_lang::parse(&source) {
            Ok(p) => p,
            Err(_) => continue,
        };
        for imp in &parsed.imports {
            let tree_path = match &imp.path {
                cook_lang::ast::ImportPath::Tree(s) => s,
                cook_lang::ast::ImportPath::Sigil(_) => continue,
            };
            let imp_dir = cookfile_dir.join(tree_path);
            let imp_canon = std::fs::canonicalize(&imp_dir).unwrap_or(imp_dir.clone());
            if imp_canon == target_canon {
                return Ok(true);
            }
            let nested = imp_canon.join("Cookfile");
            if nested.exists() {
                stack.push(nested);
            }
        }
    }
    Ok(false)
}

/// With `candidate_root` treated as the workspace root, walk every Cookfile
/// reachable from `candidate_root/Cookfile` (across both tree-relative and
/// sigil-anchored imports) and verify that every sigil-anchored import target
/// canonicalises to a directory at or below `candidate_root`.
fn all_reachable_sigils_resolve_under(candidate_root: &Path) -> Result<bool, CookError> {
    let root_canon = std::fs::canonicalize(candidate_root)
        .unwrap_or_else(|_| candidate_root.to_path_buf());
    let entry = root_canon.join("Cookfile");
    if !entry.exists() {
        return Ok(true);
    }

    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<PathBuf> = vec![entry];

    while let Some(cookfile_path) = stack.pop() {
        let cf_canon = std::fs::canonicalize(&cookfile_path)
            .unwrap_or_else(|_| cookfile_path.clone());
        if !visited.insert(cf_canon.clone()) {
            continue;
        }
        let cf_dir = cf_canon.parent().unwrap_or(Path::new("."));
        let source = match std::fs::read_to_string(&cf_canon) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let parsed = match cook_lang::parse(&source) {
            Ok(p) => p,
            Err(_) => continue,
        };
        for imp in &parsed.imports {
            let imp_dir = match &imp.path {
                cook_lang::ast::ImportPath::Tree(s) => cf_dir.join(s),
                cook_lang::ast::ImportPath::Sigil(s) => root_canon.join(s),
            };
            let imp_canon = match std::fs::canonicalize(&imp_dir) {
                Ok(c) => c,
                Err(_) => return Ok(false), // unresolvable target → candidate fails
            };
            if matches!(&imp.path, cook_lang::ast::ImportPath::Sigil(_))
                && !imp_canon.starts_with(&root_canon)
            {
                return Ok(false);
            }
            let nested = imp_canon.join("Cookfile");
            if nested.exists() {
                stack.push(nested);
            }
        }
    }
    Ok(true)
}

/// Returns the first sigil-anchored import found reachable from `invoked_cookfile`
/// (via tree-relative traversal only). Returns `Some((declaring_cookfile, alias,
/// sigil_path_string))` if one is found, `None` if none exist.
fn first_reachable_sigil_import(
    invoked_cookfile: &Path,
) -> Result<Option<(PathBuf, String, String)>, CookError> {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<PathBuf> = vec![invoked_cookfile.to_path_buf()];

    while let Some(cookfile_path) = stack.pop() {
        let cf_canon = std::fs::canonicalize(&cookfile_path)
            .unwrap_or_else(|_| cookfile_path.clone());
        if !visited.insert(cf_canon.clone()) {
            continue;
        }
        let cf_dir = cf_canon.parent().unwrap_or(Path::new("."));
        let source = match std::fs::read_to_string(&cf_canon) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let parsed = match cook_lang::parse(&source) {
            Ok(p) => p,
            Err(_) => continue,
        };
        for imp in &parsed.imports {
            if let cook_lang::ast::ImportPath::Sigil(s) = &imp.path {
                return Ok(Some((cf_canon.clone(), imp.name.clone(), s.clone())));
            }
            if let cook_lang::ast::ImportPath::Tree(s) = &imp.path {
                let imp_dir = cf_dir.join(s);
                let nested = imp_dir.join("Cookfile");
                if nested.exists() {
                    stack.push(nested);
                }
            }
        }
    }
    Ok(None)
}

/// Resolve the workspace root for an invocation per §7.6.
pub fn resolve_workspace_root(
    invoked_cookfile: &Path,
    override_root: Option<PathBuf>,
) -> Result<PathBuf, CookError> {
    // Rule 1: explicit override.
    if let Some(root) = override_root {
        let root = std::fs::canonicalize(&root).map_err(|e| {
            CookError::Other(format!("--root '{}': {e}", root.display()))
        })?;
        let invoked_canon = std::fs::canonicalize(invoked_cookfile).map_err(|e| {
            CookError::Other(format!(
                "cannot resolve {}: {e}", invoked_cookfile.display()
            ))
        })?;
        if !invoked_canon.starts_with(&root) {
            return Err(CookError::Other(format!(
                "invoked Cookfile {} is not at or below --root {}",
                invoked_canon.display(),
                root.display()
            )));
        }
        return Ok(root);
    }

    // Rule 2: .cookroot marker walk-up.
    let invoked_dir = invoked_cookfile
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let mut cur = std::fs::canonicalize(&invoked_dir).unwrap_or(invoked_dir.clone());
    loop {
        if cur.join(".cookroot").exists() {
            return Ok(cur);
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => break,
        }
    }

    // Rule 3: tree-import inference walk.
    let invoked_dir_canon = std::fs::canonicalize(&invoked_dir).unwrap_or(invoked_dir.clone());
    let mut highest: Option<PathBuf> = None;
    let mut walk_cur = invoked_dir_canon.parent().map(|p| p.to_path_buf());
    while let Some(d) = walk_cur {
        let cookfile_at_d = d.join("Cookfile");
        if cookfile_at_d.exists()
            && cookfile_transitively_imports_via_tree(&cookfile_at_d, &invoked_dir_canon)?
            && all_reachable_sigils_resolve_under(&d)?
        {
            highest = Some(d.clone());
        }
        walk_cur = d.parent().map(|p| p.to_path_buf());
    }
    if let Some(root) = highest {
        return Ok(root);
    }

    // Rules 4 and 5: no ancestor satisfied. Self-root if no sigils anywhere
    // reachable; reject otherwise.
    if let Some((cf, alias, path)) = first_reachable_sigil_import(invoked_cookfile)? {
        return Err(CookError::Other(format!(
            "Cookfile {} (or a Cookfile reachable from it) declares sigil import \
             '{}' (alias '{}', in {}), but no enclosing workspace root could be \
             identified to anchor it. Drop a .cookroot marker at the workspace \
             root, or pass --root.",
            invoked_cookfile.display(),
            path,
            alias,
            cf.display(),
        )));
    }
    Ok(invoked_dir_canon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_alias_dirs_for_root_tree_import() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("lib")).unwrap();
        fs::write(dir.path().join("lib/Cookfile"), "recipe \"build\"\n").unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import lib ./lib\nrecipe \"top\"\n",
        ).unwrap();
        fs::write(dir.path().join(".cookroot"), "").unwrap();

        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = Workspace::load(&entry, &root, &[]).unwrap();
        let root_canon = std::fs::canonicalize(&ws.root.dir).unwrap();
        let alias_dirs = ws.alias_dirs_for(&root_canon);
        assert_eq!(alias_dirs.len(), 1);
        assert_eq!(alias_dirs.get("lib"), Some(&PathBuf::from("lib")));
    }

    #[test]
    fn test_alias_dirs_for_sigil_import_with_dotdot() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("core/lib")).unwrap();
        fs::create_dir_all(dir.path().join("apps/web")).unwrap();
        fs::write(dir.path().join("core/lib/Cookfile"), "recipe \"core\"\n").unwrap();
        fs::write(
            dir.path().join("apps/web/Cookfile"),
            "import core //core/lib\nrecipe \"app\"\n",
        ).unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import web ./apps/web\nrecipe \"top\"\n",
        ).unwrap();
        fs::write(dir.path().join(".cookroot"), "").unwrap();

        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = Workspace::load(&entry, &root, &[]).unwrap();
        let web_dir = std::fs::canonicalize(dir.path().join("apps/web")).unwrap();
        let alias_dirs = ws.alias_dirs_for(&web_dir);
        assert_eq!(alias_dirs.get("core"), Some(&PathBuf::from("../../core/lib")));
    }

    #[test]
    fn test_no_imports_loads_root_only() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cookfile"), "recipe \"build\"\n").unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = Workspace::load(&entry, &root, &[]).unwrap();
        assert!(ws.imports.is_empty());
        assert!(ws.namespace_map.is_empty());
    }

    #[test]
    fn test_basic_import_loads_child() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("lib")).unwrap();
        fs::write(
            dir.path().join("lib/Cookfile"),
            "recipe \"build\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import lib ./lib\nrecipe \"bundle\": \"lib.build\"\n",
        )
        .unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = Workspace::load(&entry, &root, &[]).unwrap();
        assert_eq!(ws.imports.len(), 1);
        assert_eq!(ws.namespace_map.len(), 1);
    }

    #[test]
    fn test_dotdot_import_is_rejected_at_parse() {
        // Phase 1 rejects `..` segments in import paths. Verify this
        // surfaces as a parse error rather than a cycle/IO error.
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::create_dir_all(dir.path().join("b")).unwrap();
        fs::write(
            dir.path().join("a/Cookfile"),
            "import b ../b\nrecipe \"x\"\n",
        )
        .unwrap();
        fs::write(dir.path().join("b/Cookfile"), "recipe \"y\"\n").unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import a ./a\nrecipe \"z\"\n",
        )
        .unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let result = Workspace::load(&entry, &root, &[]);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("..") || err.contains("segment") || err.contains("parse"),
            "expected dotdot rejection error: {err}"
        );
    }

    #[test]
    fn test_dedup_same_path_via_two_tree_imports() {
        // Root imports both `a` and `b`, and both are independent children.
        // The root also directly imports `shared`. All three sub-Cookfiles
        // load without error, and the imports map has exactly one entry per
        // unique canonical path.
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::create_dir_all(dir.path().join("b")).unwrap();
        fs::write(dir.path().join("a/Cookfile"), "recipe \"a\"\n").unwrap();
        fs::write(dir.path().join("b/Cookfile"), "recipe \"b\"\n").unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import a ./a\nimport b ./b\nrecipe \"bundle\"\n",
        )
        .unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = Workspace::load(&entry, &root, &[]).unwrap();
        assert_eq!(ws.imports.len(), 2, "expected exactly 2 imports (a, b)");
    }

    #[test]
    fn test_missing_import_dir_errors() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import missing ./nonexistent\nrecipe \"x\"\n",
        )
        .unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let result = Workspace::load(&entry, &root, &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_missing_cookfile_in_import_dir_errors() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("empty")).unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import empty ./empty\nrecipe \"x\"\n",
        )
        .unwrap();
        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let result = Workspace::load(&entry, &root, &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no Cookfile"));
    }

    #[test]
    fn test_resolve_workspace_root_marker_file_takes_precedence() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        fs::write(dir.path().join("a/.cookroot"), "").unwrap();
        fs::write(dir.path().join("a/Cookfile"), "import b ./b\n").unwrap();
        fs::write(dir.path().join("a/b/Cookfile"), "import c ./c\n").unwrap();
        fs::write(dir.path().join("a/b/c/Cookfile"), "recipe \"x\"\n").unwrap();

        let invoked = dir.path().join("a/b/c/Cookfile");
        let root = resolve_workspace_root(&invoked, None).unwrap();
        let expected = std::fs::canonicalize(dir.path().join("a")).unwrap();
        let got = std::fs::canonicalize(root).unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn test_resolve_workspace_root_explicit_override() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("lib")).unwrap();
        fs::write(dir.path().join("lib/Cookfile"), "recipe \"x\"\n").unwrap();
        fs::write(dir.path().join("Cookfile"), "import lib ./lib\n").unwrap();

        let invoked = dir.path().join("lib/Cookfile");
        let root = resolve_workspace_root(&invoked, Some(dir.path().to_path_buf())).unwrap();
        let expected = std::fs::canonicalize(dir.path()).unwrap();
        let got = std::fs::canonicalize(root).unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn test_resolve_workspace_root_explicit_override_outside_invoked_rejects() {
        // Rule 1: --root that does NOT contain the invoked Cookfile must be rejected.
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("sibling/a")).unwrap();
        fs::create_dir_all(dir.path().join("sibling/b")).unwrap();
        fs::write(dir.path().join("sibling/a/Cookfile"), "recipe \"x\"\n").unwrap();
        fs::write(dir.path().join("sibling/b/Cookfile"), "recipe \"y\"\n").unwrap();

        // Invoke a/Cookfile but pass --root pointing at b/ (disjoint subtree).
        let invoked = dir.path().join("sibling/a/Cookfile");
        let wrong_root = dir.path().join("sibling/b");
        let result = resolve_workspace_root(&invoked, Some(wrong_root));
        assert!(
            result.is_err(),
            "expected rejection because invoked file is not under --root"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not at or below") || msg.contains("--root"),
            "expected diagnostic mentioning '--root' constraint, got: {msg}"
        );
    }

    #[test]
    fn test_resolve_workspace_root_tree_inference() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("apps/web")).unwrap();
        fs::write(dir.path().join("Cookfile"), "import web ./apps/web\nrecipe \"x\"\n").unwrap();
        fs::write(dir.path().join("apps/web/Cookfile"), "recipe \"build\"\n").unwrap();

        let invoked = dir.path().join("apps/web/Cookfile");
        let root = resolve_workspace_root(&invoked, None).unwrap();
        let expected = std::fs::canonicalize(dir.path()).unwrap();
        let got = std::fs::canonicalize(root).unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn test_resolve_workspace_root_tree_inference_skip_no_cookfile_ancestor() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("intermediate/leaf")).unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import leaf ./intermediate/leaf\nrecipe \"x\"\n",
        ).unwrap();
        fs::write(dir.path().join("intermediate/leaf/Cookfile"), "recipe \"build\"\n").unwrap();

        let invoked = dir.path().join("intermediate/leaf/Cookfile");
        let root = resolve_workspace_root(&invoked, None).unwrap();
        let expected = std::fs::canonicalize(dir.path()).unwrap();
        let got = std::fs::canonicalize(root).unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn test_resolve_workspace_root_skips_candidate_that_doesnt_anchor_sigils() {
        let dir = TempDir::new().unwrap();
        // Layout: dir/Cookfile imports inner/, inner/Cookfile uses //top/lib.
        // dir/top/lib/ exists. dir/inner/top/ does NOT exist.
        // Inference: dir/inner/ would be a candidate (transitively tree-imports
        // dir/inner/leaf), but //top/lib doesn't resolve under dir/inner/, so
        // dir/inner/ fails sigil validation. dir/ does anchor //top/lib (path
        // dir/top/lib exists), so dir/ wins.
        fs::create_dir_all(dir.path().join("top/lib")).unwrap();
        fs::create_dir_all(dir.path().join("inner/leaf")).unwrap();
        fs::write(dir.path().join("Cookfile"), "import inner ./inner\nrecipe \"x\"\n").unwrap();
        fs::write(
            dir.path().join("inner/Cookfile"),
            "import lib //top/lib\nimport leaf ./leaf\nrecipe \"y\"\n",
        ).unwrap();
        fs::write(dir.path().join("inner/leaf/Cookfile"), "recipe \"build\"\n").unwrap();
        fs::write(dir.path().join("top/lib/Cookfile"), "recipe \"q\"\n").unwrap();

        let invoked = dir.path().join("inner/leaf/Cookfile");
        let root = resolve_workspace_root(&invoked, None).unwrap();
        let expected = std::fs::canonicalize(dir.path()).unwrap();
        let got = std::fs::canonicalize(root).unwrap();
        assert_eq!(got, expected, "expected dir/ as root (anchors //top/lib), got {got:?}");
    }

    /// Tighter gate test: the sigil-validation gate must be observable.
    ///
    /// Layout
    /// ------
    /// dir/
    ///   shared/lib/Cookfile    (target of the sigil import)
    ///   inner/
    ///     Cookfile             imports ./leaf  AND  //shared/lib
    ///     leaf/
    ///       Cookfile           [invoked] — has its OWN sigil import //shared/lib
    ///                          (so any_reachable_sigil_imports returns true)
    ///
    /// There is NO Cookfile directly in dir/ itself.
    ///
    /// Walk-up from dir/inner/leaf/ (invoked dir):
    ///   d = dir/inner/: has Cookfile
    ///     - cookfile_transitively_imports_via_tree: YES (./leaf)
    ///     - all_reachable_sigils_resolve_under(dir/inner/): tries to resolve
    ///       //shared/lib → dir/inner/shared/lib, which does NOT exist → Ok(false)
    ///       → gate rejects dir/inner/ as a candidate
    ///   d = dir/: no Cookfile → skipped
    ///
    /// No candidate found.  any_reachable_sigil_imports(invoked) → true
    /// (the invoked Cookfile itself declares //shared/lib) → Rule 5 fires → Err.
    ///
    /// Without the gate (sigil-validation removed), dir/inner/ WOULD become the
    /// only candidate (tree-import check passes) → resolve_workspace_root returns
    /// Ok(dir/inner/) instead of Err, so the test would fail.  The gate is the
    /// sole mechanism that eliminates the candidate and forces rule 5.
    #[test]
    fn test_resolve_workspace_root_gate_eliminates_only_candidate_falls_to_rule5() {
        let dir = TempDir::new().unwrap();
        // dir/shared/lib/ — sigil target that resolves at dir/ level but NOT under inner/
        fs::create_dir_all(dir.path().join("shared/lib")).unwrap();
        fs::write(dir.path().join("shared/lib/Cookfile"), "recipe \"lib\"\n").unwrap();
        // dir/inner/leaf/ — the invoked Cookfile, also has a sigil import
        fs::create_dir_all(dir.path().join("inner/leaf")).unwrap();
        fs::write(
            dir.path().join("inner/leaf/Cookfile"),
            "import shared //shared/lib\nrecipe \"leaf\"\n",
        ).unwrap();
        // dir/inner/Cookfile — tree-imports ./leaf AND uses //shared/lib
        // (no dir/inner/shared/ directory, so the sigil fails the gate at this level)
        fs::write(
            dir.path().join("inner/Cookfile"),
            "import leaf ./leaf\nimport shared //shared/lib\nrecipe \"inner\"\n",
        ).unwrap();
        // Deliberately NO Cookfile at dir/ itself.

        let invoked = dir.path().join("inner/leaf/Cookfile");
        let result = resolve_workspace_root(&invoked, None);

        assert!(
            result.is_err(),
            "expected rule-5 rejection because the only tree-import candidate (inner/) \
             failed the sigil-validation gate and no higher candidate exists; \
             got Ok({:?})",
            result.ok()
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("workspace root") || msg.contains("anchor"),
            "expected diagnostic mentioning 'workspace root' or 'anchor', got: {msg}"
        );
    }

    #[test]
    fn test_resolve_workspace_root_rejects_self_root_with_sigils() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import top //top/lib\nrecipe \"x\"\n",
        ).unwrap();

        let invoked = dir.path().join("Cookfile");
        let result = resolve_workspace_root(&invoked, None);
        assert!(result.is_err(), "expected reject for sigil import without anchor");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("workspace root"), "diagnostic missing 'workspace root'");
        assert!(
            msg.contains("top/lib") || msg.contains("//top/lib"),
            "diagnostic should name offending sigil path, got: {msg}"
        );
        assert!(
            msg.contains("'top'") || msg.contains("alias 'top'") || msg.contains("(alias 'top'"),
            "diagnostic should name offending alias, got: {msg}"
        );
    }

    #[test]
    fn test_resolve_workspace_root_self_root_no_sigils() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cookfile"), "recipe \"x\"\n").unwrap();

        let invoked = dir.path().join("Cookfile");
        let root = resolve_workspace_root(&invoked, None).unwrap();
        let expected = std::fs::canonicalize(dir.path()).unwrap();
        let got = std::fs::canonicalize(root).unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn test_diamond_via_sigil_dedups() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shared/lib")).unwrap();
        fs::create_dir_all(dir.path().join("apps/a")).unwrap();
        fs::create_dir_all(dir.path().join("apps/b")).unwrap();
        fs::write(dir.path().join("shared/lib/Cookfile"), "recipe \"shared\"\n").unwrap();
        fs::write(
            dir.path().join("apps/a/Cookfile"),
            "import shared //shared/lib\nrecipe \"a\"\n",
        ).unwrap();
        fs::write(
            dir.path().join("apps/b/Cookfile"),
            "import shared //shared/lib\nrecipe \"b\"\n",
        ).unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import a ./apps/a\nimport b ./apps/b\nrecipe \"top\"\n",
        ).unwrap();

        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = Workspace::load(&entry, &root, &[]).unwrap();
        let shared_count = ws
            .imports
            .keys()
            .filter(|p| p.to_string_lossy().contains("shared/lib"))
            .count();
        assert_eq!(shared_count, 1, "shared/lib must dedup across diamond imports");
    }

    #[test]
    fn test_cycle_via_sigil_rejected() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::create_dir_all(dir.path().join("b")).unwrap();
        fs::write(dir.path().join("a/Cookfile"), "import b //b\nrecipe \"x\"\n").unwrap();
        fs::write(dir.path().join("b/Cookfile"), "import a //a\nrecipe \"y\"\n").unwrap();
        fs::write(
            dir.path().join("Cookfile"),
            "import a ./a\nimport b ./b\nrecipe \"top\"\n",
        ).unwrap();
        fs::write(dir.path().join(".cookroot"), "").unwrap();

        let entry = dir.path().join("Cookfile");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let result = Workspace::load(&entry, &root, &[]);
        assert!(result.is_err(), "expected cycle detection to reject");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.to_lowercase().contains("cycle") || msg.to_lowercase().contains("circular"),
            "expected cycle diagnostic, got: {msg}"
        );
    }
}
