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
