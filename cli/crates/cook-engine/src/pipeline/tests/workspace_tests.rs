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
fn test_workspace_codegen_emits_dep_output_for_alias_recipe() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("lib")).unwrap();
    fs::write(
        dir.path().join("lib/Cookfile"),
        "recipe lib_build\n    cook \"build/lib.o\" { echo $<out> }\n",
    ).unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        "import lib ./lib\nrecipe demo\n    cook \"build/demo\" { echo $<lib.lib_build> }\n",
    ).unwrap();
    fs::write(dir.path().join(".cookroot"), "").unwrap();

    let entry = dir.path().join("Cookfile");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let ws = Workspace::load(&entry, &root, &[]).unwrap();

    // The root cookfile's lua_source should now contain `cook.dep_output("lib.lib_build")`.
    assert!(
        ws.root.lua_source.contains("cook.dep_output(\"lib.lib_build\")"),
        "expected dep_output(lib.lib_build) emission, got:\n{}",
        ws.root.lua_source
    );
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
