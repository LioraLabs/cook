//! Invocation anchoring: which Cookfile is the entry point, and which
//! directory is the workspace root.
//!
//! Two concerns, both answering "where does this invocation anchor":
//!
//! * [`discover_entry_cookfile`] — upward entry-point selection (§20.2 /
//!   CS-0120): from the invocation cwd, walk up to the nearest `Cookfile`,
//!   bounded by the workspace boundary (`.cookroot` or `--root`).
//! * [`resolve_workspace_root`] — workspace-root resolution (§7.6): explicit
//!   `--root` override, `.cookroot` marker walk-up, tree-import inference,
//!   then self-root / reject.
//!
//! Extracted from `pipeline::workspace` (which owns multi-Cookfile loading);
//! both remain re-exported from `pipeline`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::error::PipelineError;

/// Returns true if `candidate_cookfile` transitively imports `target_dir` via
/// tree-relative imports only. Sigil-anchored imports are skipped (their
/// resolution presupposes a workspace root, which is what we are computing).
fn cookfile_transitively_imports_via_tree(
    candidate_cookfile: &Path,
    target_dir: &Path,
) -> Result<bool, PipelineError> {
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
fn all_reachable_sigils_resolve_under(candidate_root: &Path) -> Result<bool, PipelineError> {
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
) -> Result<Option<(PathBuf, String, String)>, PipelineError> {
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

/// Entry-point selection (§20.2 / CS-0120): walk upward from `start_dir` to
/// the nearest directory containing a `Cookfile` and return that Cookfile's
/// canonicalised path.
///
/// The walk is bounded at the workspace boundary: a directory containing a
/// `.cookroot` marker is the last directory examined (it is itself still
/// checked for a `Cookfile`), as is `stop_at` (the `--root` override). The
/// filesystem root bounds the walk unconditionally. The walk never selects a
/// Cookfile above the boundary — it must not escape into an unrelated
/// enclosing project.
pub fn discover_entry_cookfile(
    start_dir: &Path,
    stop_at: Option<&Path>,
) -> Result<PathBuf, PipelineError> {
    let start = std::fs::canonicalize(start_dir).unwrap_or_else(|_| start_dir.to_path_buf());
    let stop_canon =
        stop_at.map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    if let Some(stop) = &stop_canon {
        if !start.starts_with(stop) {
            return Err(PipelineError::Workspace(format!(
                "invocation directory {} is not at or below --root {}",
                start.display(),
                stop.display()
            )));
        }
    }
    let mut cur = start.clone();
    loop {
        let candidate = cur.join("Cookfile");
        if candidate.is_file() {
            return Ok(candidate);
        }
        let at_boundary =
            cur.join(".cookroot").exists() || stop_canon.as_deref() == Some(cur.as_path());
        if at_boundary {
            return Err(PipelineError::Workspace(format!(
                "no Cookfile found from {} up to the workspace boundary {}",
                start.display(),
                cur.display()
            )));
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => {
                return Err(PipelineError::Workspace(format!(
                    "no Cookfile found from {} up to the filesystem root",
                    start.display()
                )));
            }
        }
    }
}

/// Resolve the workspace root for an invocation per §7.6.
pub fn resolve_workspace_root(
    invoked_cookfile: &Path,
    override_root: Option<PathBuf>,
) -> Result<PathBuf, PipelineError> {
    // Rule 1: explicit override.
    if let Some(root) = override_root {
        let root = std::fs::canonicalize(&root).map_err(|e| {
            PipelineError::Workspace(format!("--root '{}': {e}", root.display()))
        })?;
        let invoked_canon = std::fs::canonicalize(invoked_cookfile).map_err(|e| {
            PipelineError::Workspace(format!(
                "cannot resolve {}: {e}",
                invoked_cookfile.display()
            ))
        })?;
        if !invoked_canon.starts_with(&root) {
            return Err(PipelineError::Workspace(format!(
                "invoked Cookfile {} is not at or below --root {}",
                invoked_canon.display(),
                root.display()
            )));
        }
        return Ok(root);
    }

    // Rule 2: .cookroot marker walk-up.
    let invoked_dir = {
        let p = invoked_cookfile
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        // An empty path (parent of a bare "Cookfile") is treated as ".".
        if p.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            p
        }
    };
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
    let invoked_dir_canon = std::fs::canonicalize(&invoked_dir)
        .or_else(|_| std::fs::canonicalize("."))
        .unwrap_or(invoked_dir.clone());
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
        return Err(PipelineError::Workspace(format!(
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

    #[test]
    fn test_resolve_workspace_root_gate_eliminates_only_candidate_falls_to_rule5() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shared/lib")).unwrap();
        fs::write(dir.path().join("shared/lib/Cookfile"), "recipe \"lib\"\n").unwrap();
        fs::create_dir_all(dir.path().join("inner/leaf")).unwrap();
        fs::write(
            dir.path().join("inner/leaf/Cookfile"),
            "import shared //shared/lib\nrecipe \"leaf\"\n",
        ).unwrap();
        fs::write(
            dir.path().join("inner/Cookfile"),
            "import leaf ./leaf\nimport shared //shared/lib\nrecipe \"inner\"\n",
        ).unwrap();

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
    fn test_discover_entry_nearest_cookfile_wins() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cookfile"), "recipe build\n    echo root\n").unwrap();
        fs::create_dir_all(dir.path().join("apps/rust/src")).unwrap();
        fs::write(dir.path().join("apps/rust/Cookfile"), "recipe build\n    echo member\n").unwrap();
        let found = discover_entry_cookfile(&dir.path().join("apps/rust/src"), None).unwrap();
        assert_eq!(
            found,
            std::fs::canonicalize(dir.path().join("apps/rust/Cookfile")).unwrap()
        );
    }

    #[test]
    fn test_discover_entry_falls_through_to_root_cookfile() {
        // cwd deep in a dir with no Cookfile until the root — root is the entry.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cookfile"), "recipe build\n    echo root\n").unwrap();
        fs::create_dir_all(dir.path().join("tools/scripts")).unwrap();
        let found = discover_entry_cookfile(&dir.path().join("tools/scripts"), None).unwrap();
        assert_eq!(found, std::fs::canonicalize(dir.path().join("Cookfile")).unwrap());
    }

    #[test]
    fn test_discover_entry_stops_at_cookroot_boundary() {
        // A decoy Cookfile ABOVE the .cookroot boundary must not be selected.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cookfile"), "recipe build\n    echo decoy\n").unwrap();
        fs::create_dir_all(dir.path().join("proj/sub")).unwrap();
        fs::write(dir.path().join("proj/.cookroot"), "").unwrap();
        let err = discover_entry_cookfile(&dir.path().join("proj/sub"), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no Cookfile found"), "msg: {msg}");
    }

    #[test]
    fn test_discover_entry_boundary_dir_itself_is_checked() {
        // .cookroot dir with a Cookfile at the same level: found, not an error.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("proj/sub")).unwrap();
        fs::write(dir.path().join("proj/.cookroot"), "").unwrap();
        fs::write(dir.path().join("proj/Cookfile"), "recipe build\n    echo x\n").unwrap();
        let found = discover_entry_cookfile(&dir.path().join("proj/sub"), None).unwrap();
        assert_eq!(found, std::fs::canonicalize(dir.path().join("proj/Cookfile")).unwrap());
    }

    #[test]
    fn test_discover_entry_stop_at_explicit_root() {
        // --root bounds the walk like .cookroot does.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cookfile"), "recipe build\n    echo decoy\n").unwrap();
        fs::create_dir_all(dir.path().join("proj/sub")).unwrap();
        let err =
            discover_entry_cookfile(&dir.path().join("proj/sub"), Some(&dir.path().join("proj")))
                .unwrap_err();
        assert!(err.to_string().contains("no Cookfile found"));
    }

    #[test]
    fn test_discover_entry_non_ancestor_stop_at_errors_instead_of_unbounding() {
        // --root that is NOT an ancestor of the start dir must not silently
        // unbound the walk (and select a Cookfile above the intended boundary).
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cookfile"), "recipe build\n    echo decoy\n").unwrap();
        fs::create_dir_all(dir.path().join("proj/sub")).unwrap();
        fs::create_dir_all(dir.path().join("elsewhere")).unwrap();
        let err = discover_entry_cookfile(
            &dir.path().join("proj/sub"),
            Some(&dir.path().join("elsewhere")),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not at or below"), "msg: {msg}");
    }

    #[test]
    fn test_discover_entry_skips_directory_named_cookfile() {
        // A DIRECTORY named "Cookfile" is not a Cookfile; the walk continues up.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cookfile"), "recipe build\n    echo root\n").unwrap();
        fs::create_dir_all(dir.path().join("sub/Cookfile")).unwrap();
        let found = discover_entry_cookfile(&dir.path().join("sub"), None).unwrap();
        assert_eq!(found, std::fs::canonicalize(dir.path().join("Cookfile")).unwrap());
    }
}
