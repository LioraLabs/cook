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
        "import lib ./lib\nrecipe top\n    cook \"build/top\" { echo $<lib.lib_build> }\n",
        &[("lib", "recipe lib_build\n    cook \"lib.o\" { echo $<out> }\n")],
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
        "recipe core_lib\n    cook \"core.o\" { echo $<out> }\n",
    )
    .unwrap();
    // apps/web Cookfile — imports core via sigil, refs $<core.core_lib>
    fs::create_dir_all(dir.path().join("apps/web")).unwrap();
    fs::write(
        dir.path().join("apps/web/Cookfile"),
        "import core //core/lib\nrecipe web_app\n    cook \"web.o\" { echo $<core.core_lib> }\n",
    )
    .unwrap();
    // root Cookfile: imports BOTH web (tree) AND core (sigil) directly.
    fs::write(
        dir.path().join("Cookfile"),
        "import web ./apps/web\nimport core //core/lib\nrecipe top\n    cook \"build/top\" { echo $<web.web_app> $<core.core_lib> }\n",
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
        "import lib ./lib\nrecipe top\n",
        &[("lib", "recipe lib_build\n")],
    );
    let deps = compute_workspace_inferred_deps(&ws);
    assert!(
        deps.is_empty(),
        "expected empty inferred_deps when no body refs, got: {deps:?}"
    );
}

/// Diagnostic text guard (ported from the deleted single-path test):
/// the explicit+inferred conflict warning must spell the inferred form
/// with the `$<...>` sigil syntax, not legacy `{...}`.
#[test]
fn workspace_dep_conflict_uses_sigil_placeholder_syntax() {
    let (_dir, ws) = make_workspace(
        "recipe compile\n    cook \"compile.out\" { echo hi > $<out> }\nrecipe link: compile\n    cook \"link.out\" { echo $<compile> > $<out> }\n",
        &[],
    );
    let inferred = compute_workspace_inferred_deps(&ws);
    let warnings = workspace_dep_conflicts(&ws, &inferred);
    assert_eq!(warnings.len(), 1, "expected exactly one warning, got: {warnings:?}");
    assert_eq!(
        warnings[0],
        "recipe 'link' has both explicit ': compile' and inferred '$<compile>' dependency — conflicting scheduling intent"
    );
}
