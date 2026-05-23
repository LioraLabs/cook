//! End-to-end CLI tests for `cook affected` and `--affected` (COOK-58).
//!
//! Each test creates a fresh tempdir, initialises a git repo with local
//! user config (never touches global), writes a Cookfile + sources, and
//! runs the `cook` binary against it.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

/// Init a git repo + write Cookfile and tracked files, commit them.
fn init_workspace(cookfile: &str, files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    git(&dir, &["init", "-b", "main"]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "Test"]);
    write(&dir, "Cookfile", cookfile);
    for (rel, body) in files {
        write(&dir, rel, body);
    }
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-m", "initial"]);
    dir
}

fn git(dir: &TempDir, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(dir: &TempDir, rel: &str, body: &str) {
    let p = dir.path().join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

fn run_cook(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_bin())
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

/// Standard single-recipe fixture: `recipe build` declaring `src/main.c` as input.
const SINGLE_RECIPE_COOKFILE: &str = r#"
recipe build
    >>{
        cook.add_unit({
            name    = "build-step",
            inputs  = {"src/main.c"},
            outputs = {"out.bin"},
            command = "cp src/main.c out.bin",
        })
    }
"#;

#[test]
fn introspection_lists_affected_recipe() {
    let dir = init_workspace(
        SINGLE_RECIPE_COOKFILE,
        &[("src/main.c", "int main(){return 0;}")],
    );
    // Modify the tracked file post-commit so it's in the diff.
    write(&dir, "src/main.c", "int main(){return 1;}");
    let out = run_cook(dir.path(), &["affected", "--since=HEAD"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "build");
}

#[test]
fn introspection_json_output_has_expected_keys() {
    let dir = init_workspace(
        SINGLE_RECIPE_COOKFILE,
        &[("src/main.c", "int main(){return 0;}")],
    );
    write(&dir, "src/main.c", "int main(){return 1;}");
    let out = run_cook(dir.path(), &["affected", "--since=HEAD", "--json"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"affected_recipes\""), "json: {s}");
    assert!(s.contains("\"changed_files\""), "json: {s}");
    assert!(s.contains("\"since_ref\""), "json: {s}");
}

#[test]
fn drive_scheduler_runs_only_affected_recipe() {
    // Two recipes; touch only the input of `a`. Run aggregate `c` with
    // --affected and assert `a` ran but `b` didn't (by checking output files).
    let cookfile = r#"
recipe a
    >>{
        cook.add_unit({
            name    = "a-step",
            inputs  = {"src/a.txt"},
            outputs = {"a.stamp"},
            command = "touch a.stamp",
        })
    }

recipe b
    >>{
        cook.add_unit({
            name    = "b-step",
            inputs  = {"src/b.txt"},
            outputs = {"b.stamp"},
            command = "touch b.stamp",
        })
    }

recipe c : a b
"#;
    let dir = init_workspace(
        cookfile,
        &[("src/a.txt", "a-v1"), ("src/b.txt", "b-v1")],
    );
    write(&dir, "src/a.txt", "a-v2");
    let out = run_cook(dir.path(), &["c", "--affected", "--since=HEAD"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.path().join("a.stamp").exists(), "a should have run");
    assert!(!dir.path().join("b.stamp").exists(), "b should NOT have run");
}

#[test]
fn empty_affected_exits_zero_with_message() {
    let dir = init_workspace(
        SINGLE_RECIPE_COOKFILE,
        &[("src/main.c", "int main(){return 0;}")],
    );
    // No file changes since HEAD.
    let out = run_cook(dir.path(), &["build", "--affected", "--since=HEAD"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nothing affected"), "stderr: {stderr}");
}

#[test]
fn bad_ref_exits_nonzero() {
    let dir = init_workspace(
        SINGLE_RECIPE_COOKFILE,
        &[("src/main.c", "int main(){return 0;}")],
    );
    let out = run_cook(
        dir.path(),
        &["build", "--affected", "--since=nonexistent-ref"],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nonexistent-ref"),
        "stderr: {stderr}"
    );
}

#[test]
fn outside_git_repo_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    write(&dir, "Cookfile", SINGLE_RECIPE_COOKFILE);
    write(&dir, "src/main.c", "int main(){return 0;}");
    let out = run_cook(dir.path(), &["build", "--affected", "--since=main"]);
    assert!(!out.status.success());
}

#[test]
fn affected_without_since_errors() {
    let dir = init_workspace(
        SINGLE_RECIPE_COOKFILE,
        &[("src/main.c", "int main(){return 0;}")],
    );
    let out = run_cook(dir.path(), &["build", "--affected"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--since"),
        "stderr should mention --since: {stderr}"
    );
}
