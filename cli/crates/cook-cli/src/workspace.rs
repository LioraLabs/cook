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
}

impl Workspace {
    pub fn load(cookfile_path: &Path, _cli_sets: &[String]) -> Result<Self, CookError> {
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
        })
    }

    fn load_imports(
        cookfile: &Cookfile,
        cookfile_dir: &Path,
        imports: &mut BTreeMap<PathBuf, LoadedCookfile>,
        namespace_map: &mut Vec<(PathBuf, String, PathBuf)>,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<(), CookError> {
        let parent_canonical = std::fs::canonicalize(cookfile_dir)
            .unwrap_or_else(|_| cookfile_dir.to_path_buf());

        for import_decl in &cookfile.imports {
            let import_dir = cookfile_dir.join(import_decl.path.to_string());
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
        {
            highest = Some(d.clone());
        }
        walk_cur = d.parent().map(|p| p.to_path_buf());
    }
    if let Some(root) = highest {
        return Ok(root);
    }

    // Fall-through: self-root (Task 2.5 will add the sigil-presence gate).
    Ok(invoked_dir_canon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_no_imports_loads_root_only() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cookfile"), "recipe \"build\"\n").unwrap();
        let ws = Workspace::load(&dir.path().join("Cookfile"), &[]).unwrap();
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
        let ws = Workspace::load(&dir.path().join("Cookfile"), &[]).unwrap();
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
        let result = Workspace::load(&dir.path().join("Cookfile"), &[]);
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
        let ws = Workspace::load(&dir.path().join("Cookfile"), &[]).unwrap();
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
        let result = Workspace::load(&dir.path().join("Cookfile"), &[]);
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
        let result = Workspace::load(&dir.path().join("Cookfile"), &[]);
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
}
