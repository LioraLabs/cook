//! CS-0196 (COOK-364): cache identity is portable across checkout locations.
//!
//! The key-side project segment is exactly the configured `[cloud] project`
//! or empty — never the checkout directory's name. Two clones of one
//! workspace under DIFFERENT directory names must therefore share cache
//! entries through a common store, and setting `[cloud] project` must
//! separate two projects that would otherwise collide.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("cook");
    assert!(path.exists(), "cook binary not found — cargo build --bin cook");
    path
}

const COOKFILE: &str = "recipe build\n    ingredients \"src.txt\"\n    cook \"out/app.txt\" { sed 's/^/[app] /' src.txt > $<out> }\n";

fn mk_checkout(root: &Path, name: &str, shared_store: &Path, project: Option<&str>) -> std::path::PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(dir.join(".cook")).unwrap();
    let project_line = project
        .map(|p| format!("[cloud]\nproject = \"{p}\"\n"))
        .unwrap_or_default();
    fs::write(
        dir.join(".cook/cloud.toml"),
        format!("{project_line}[cache]\ncache_dir = \"{}\"\n", shared_store.display()),
    )
    .unwrap();
    fs::write(dir.join("Cookfile"), COOKFILE).unwrap();
    fs::write(dir.join("src.txt"), "hello portable cache\n").unwrap();
    dir
}

fn run_build(dir: &Path) -> String {
    let out = Command::new(cook_binary())
        .arg("build")
        .current_dir(dir)
        .output()
        .expect("spawn cook");
    assert!(
        out.status.success(),
        "cook build failed in {}:\n{}{}",
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn different_checkout_names_share_cache_entries() {
    let tmp = tempdir().unwrap();
    let store = tmp.path().join("shared-store");
    fs::create_dir_all(&store).unwrap();

    let a = mk_checkout(tmp.path(), "cook", &store, None);
    let b = mk_checkout(tmp.path(), "cook-fork", &store, None);

    let first = run_build(&a);
    assert!(
        !first.contains("cached recipes, 0 done") || first.contains("1 done"),
        "first build should run: {first}"
    );

    // Different directory name, fresh local state, same store: the key must
    // match, so the unit is served without running.
    let second = run_build(&b);
    assert!(
        second.contains("(1/1 cached)") || second.contains("1 cached recipes"),
        "checkout under a different name must hit the shared store, got:\n{second}"
    );
    assert_eq!(
        fs::read_to_string(a.join("out/app.txt")).unwrap(),
        fs::read_to_string(b.join("out/app.txt")).unwrap(),
    );
}

#[test]
fn configured_project_separates_two_projects() {
    let tmp = tempdir().unwrap();
    let store = tmp.path().join("shared-store");
    fs::create_dir_all(&store).unwrap();

    let a = mk_checkout(tmp.path(), "one", &store, Some("acme"));
    let b = mk_checkout(tmp.path(), "two", &store, Some("globex"));

    run_build(&a);
    // Same content, same command — but a DIFFERENT configured identity.
    // The second project must not be served the first one's artifact.
    let second = run_build(&b);
    assert!(
        !second.contains("1 cached recipes"),
        "distinct [cloud] project identities must not share, got:\n{second}"
    );
}
