//! CS-0120 §20.2 end-to-end: upward Cookfile discovery, cwd-scoped bare
//! names, root-anchored cache keys, and the reserved `//` target syntax.
//!
//! Acceptance: `cook build` in apps/rust/src == `cook build` in apps/rust ==
//! `cook rust.build` at the workspace root, with IDENTICAL cache keys;
//! `cook //x` is rejected with the reserved-syntax diagnostic; the upward
//! walk never escapes a `.cookroot` boundary.

use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("mkdir -p");
    }
    std::fs::write(p, body).expect("write file");
}

/// Isolate the shared artifact store so parallel test runs / developer
/// machines don't cross-pollinate via ~/.cache/cook/cloud. Written at the
/// WORKSPACE ROOT — that is where §20.2.3 anchors .cook/cloud.toml.
fn write_isolated_cache_config(root: &Path, cache_dir: &Path) {
    std::fs::create_dir_all(root.join(".cook")).unwrap();
    std::fs::write(
        root.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

fn run_cook(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_bin())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("invoke cook")
}

fn assert_ok(out: &std::process::Output, what: &str) {
    assert!(
        out.status.success(),
        "{what} failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `cook why <target> --json` in `dir` and return (unit key hex,
/// unit local cache_key, status) for the single unit of the target recipe.
fn why_unit(dir: &Path, target: &str) -> (String, String, String) {
    let out = run_cook(dir, &["why", target, "--json"]);
    assert_ok(&out, &format!("cook why {target} in {}", dir.display()));
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("why --json parses");
    let units = v["units"].as_array().expect("units array");
    assert_eq!(units.len(), 1, "expected exactly one unit, got: {v}");
    (
        units[0]["key"].as_str().expect("key").to_string(),
        units[0]["cache_key"].as_str().expect("cache_key").to_string(),
        units[0]["status"].as_str().expect("status").to_string(),
    )
}

const MEMBER_COOKFILE: &str = r#"recipe build
    >>{
        cook.add_unit({
            inputs  = { "src/main.txt" },
            outputs = { "build/out.txt" },
            command = "mkdir -p build && cp src/main.txt build/out.txt",
        })
    }
"#;

const ROOT_COOKFILE: &str = "import rust apps/rust\n\nrecipe check\n    echo root-check\n";

fn workspace_fixture() -> (TempDir, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let cache = TempDir::new().expect("cache tempdir");
    let root = tmp.path();
    write(root, "Cookfile", ROOT_COOKFILE);
    write(root, "apps/rust/Cookfile", MEMBER_COOKFILE);
    write(root, "apps/rust/src/main.txt", "payload-v1\n");
    write_isolated_cache_config(root, cache.path());
    (tmp, cache)
}

/// (i) Single-Cookfile project: invoke from a nested subdir; the nearest
/// (only) Cookfile is discovered, the build runs, outputs land relative to
/// the Cookfile's dir, and .cook state lands at the project dir.
#[test]
fn single_project_invoked_from_nested_subdir() {
    let tmp = TempDir::new().expect("tempdir");
    let cache = TempDir::new().expect("cache tempdir");
    let root = tmp.path();
    write(root, "Cookfile", MEMBER_COOKFILE);
    write(root, "src/main.txt", "hello\n");
    std::fs::create_dir_all(root.join("src/deeper")).unwrap();
    write_isolated_cache_config(root, cache.path());

    let out = run_cook(&root.join("src/deeper"), &["build"]);
    assert_ok(&out, "cook build from nested subdir");
    assert!(root.join("build/out.txt").exists(), "output at Cookfile dir");
    assert!(
        !root.join("src/deeper/build").exists(),
        "no stray output at cwd"
    );
}

/// (ii) THE acceptance criterion: identical cache keys from all three
/// invocation points, and a warm cache from one point is a hit from another.
#[test]
fn workspace_three_invocation_points_share_one_cache_key() {
    let (tmp, _cache) = workspace_fixture();
    let root = tmp.path();
    let member = root.join("apps/rust");
    let member_src = root.join("apps/rust/src");

    let (k_root, ck_root, _) = why_unit(root, "rust.build");
    let (k_member, ck_member, _) = why_unit(&member, "build");
    let (k_nested, ck_nested, _) = why_unit(&member_src, "build");

    assert_eq!(k_root, k_member, "root vs member key");
    assert_eq!(k_member, k_nested, "member vs nested key");
    assert_eq!(ck_root, ck_member, "root vs member local cache_key");
    assert_eq!(ck_member, ck_nested, "member vs nested local cache_key");

    // Warm the cache from the root...
    let out = run_cook(root, &["rust.build"]);
    assert_ok(&out, "cook rust.build at root");
    assert!(root.join("apps/rust/build/out.txt").exists());

    // ...and observe a local hit from inside the member subtree.
    let (_, _, status_nested) = why_unit(&member_src, "build");
    assert_eq!(status_nested, "local_hit", "warm cache visible from nested cwd");
}

/// (iii) Reserved `//` target syntax is rejected with a clear diagnostic.
#[test]
fn double_slash_target_rejected() {
    let (tmp, _cache) = workspace_fixture();
    let out = run_cook(tmp.path(), &["//check"]);
    assert!(!out.status.success(), "cook //check must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("reserved"), "stderr: {stderr}");
    assert!(stderr.contains("not yet supported"), "stderr: {stderr}");
}

/// (iv) The upward walk stops at a `.cookroot` boundary — it must not
/// select a decoy Cookfile in an unrelated enclosing project.
#[test]
fn walk_up_does_not_escape_cookroot_boundary() {
    let tmp = TempDir::new().expect("tempdir");
    let outer = tmp.path();
    write(outer, "Cookfile", "recipe build\n    echo DECOY\n");
    std::fs::create_dir_all(outer.join("proj/sub")).unwrap();
    write(outer, "proj/.cookroot", "");

    let out = run_cook(&outer.join("proj/sub"), &["build"]);
    assert!(!out.status.success(), "must not run the decoy build");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no Cookfile found"), "stderr: {stderr}");
}

/// Edge: cwd inside the workspace with no Cookfile until the root — the
/// root Cookfile is the entry and bare names are the root's recipes.
#[test]
fn bare_dir_falls_through_to_root_cookfile() {
    let (tmp, _cache) = workspace_fixture();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tools")).unwrap();

    let out = run_cook(&root.join("tools"), &["check"]);
    assert_ok(&out, "cook check from bare tools/ dir");
}

/// Explicit `-f` disables discovery: naming a missing file is an error even
/// when an ancestor Cookfile exists.
#[test]
fn explicit_file_flag_disables_discovery() {
    let (tmp, _cache) = workspace_fixture();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tools")).unwrap();

    let out = run_cook(&root.join("tools"), &["-f", "Cookfile", "check"]);
    assert!(!out.status.success(), "explicit -f Cookfile in a bare dir must error");
}

/// §20.2.2 negative: the enclosing workspace's alias namespace does not
/// leak into an invocation made inside the member — `rust.build` is not a
/// name the member knows.
#[test]
fn enclosing_alias_does_not_leak_into_member() {
    let (tmp, _cache) = workspace_fixture();
    let member = tmp.path().join("apps/rust");
    let out = run_cook(&member, &["rust.build"]);
    assert!(
        !out.status.success(),
        "rust.build must not resolve from inside the member"
    );
}
