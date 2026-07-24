use super::*;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

/// Initialise a fresh git repo with local user.email/user.name so tests
/// never touch the developer's global config.
fn init_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    run(&dir, &["init", "-b", "main"]);
    run(&dir, &["config", "user.email", "test@example.com"]);
    run(&dir, &["config", "user.name", "Test"]);
    dir
}

fn run(dir: &TempDir, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

fn write(dir: &TempDir, rel: &str, body: &str) {
    let p = dir.path().join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

fn commit(dir: &TempDir, msg: &str) {
    run(dir, &["add", "-A"]);
    run(dir, &["commit", "-m", msg]);
}

fn assert_set(got: BTreeSet<PathBuf>, expected: &[&str]) {
    let want: BTreeSet<PathBuf> = expected.iter().map(PathBuf::from).collect();
    assert_eq!(got, want);
}

#[test]
fn linear_history_since_head_minus_one() {
    let dir = init_repo();
    write(&dir, "a.txt", "1");
    commit(&dir, "first");
    write(&dir, "b.txt", "2");
    commit(&dir, "second");
    let got = changed_paths(dir.path(), "HEAD~1").unwrap();
    assert_set(got, &["b.txt"]);
}

#[test]
fn three_dot_semantics_ignores_other_branch_advances() {
    let dir = init_repo();
    write(&dir, "base.txt", "0");
    commit(&dir, "base");

    run(&dir, &["checkout", "-b", "feature"]);
    write(&dir, "feature.txt", "f");
    commit(&dir, "on feature");

    run(&dir, &["checkout", "main"]);
    write(&dir, "main-extra.txt", "m");
    commit(&dir, "on main");

    run(&dir, &["checkout", "feature"]);
    let got = changed_paths(dir.path(), "main").unwrap();
    // three-dot from merge-base: only feature.txt, NOT main-extra.txt
    assert_set(got, &["feature.txt"]);
}

#[test]
fn includes_working_tree_modifications() {
    let dir = init_repo();
    write(&dir, "tracked.txt", "v1");
    commit(&dir, "initial");
    write(&dir, "tracked.txt", "v2");
    let got = changed_paths(dir.path(), "HEAD").unwrap();
    assert_set(got, &["tracked.txt"]);
}

#[test]
fn includes_untracked_non_ignored() {
    let dir = init_repo();
    write(&dir, "tracked.txt", "v1");
    commit(&dir, "initial");
    write(&dir, "new.txt", "fresh");
    let got = changed_paths(dir.path(), "HEAD").unwrap();
    assert_set(got, &["new.txt"]);
}

#[test]
fn excludes_gitignored_files() {
    let dir = init_repo();
    write(&dir, ".gitignore", "ignored.txt\n");
    commit(&dir, "ignore");
    write(&dir, "ignored.txt", "x");
    let got = changed_paths(dir.path(), "HEAD").unwrap();
    assert_set(got, &[]);
}

#[test]
fn bad_ref_returns_ref_not_found() {
    let dir = init_repo();
    write(&dir, "a.txt", "1");
    commit(&dir, "init");
    let err = changed_paths(dir.path(), "nonexistent-ref").unwrap_err();
    match err {
        GitError::RefNotFound { reference, .. } => assert_eq!(reference, "nonexistent-ref"),
        other => panic!("expected RefNotFound, got {other:?}"),
    }
}

#[test]
fn not_a_git_repo_returns_not_a_git_repo() {
    let dir = TempDir::new().unwrap();
    let err = changed_paths(dir.path(), "main").unwrap_err();
    assert!(matches!(err, GitError::NotAGitRepo(_)), "got {err:?}");
}

#[test]
fn shallow_clone_outside_depth_returns_no_merge_base() {
    let origin = init_repo();
    write(&origin, "a.txt", "1");
    commit(&origin, "c1");
    write(&origin, "b.txt", "2");
    commit(&origin, "c2");
    write(&origin, "c.txt", "3");
    commit(&origin, "c3");

    let shallow = TempDir::new().unwrap();
    // Use file:// prefix to prevent git from ignoring --depth on local clones.
    let origin_url = format!("file://{}", origin.path().display());
    let out = Command::new("git")
        .args(["clone", "--depth=1"])
        .arg(&origin_url)
        .arg(shallow.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "clone failed: {}", String::from_utf8_lossy(&out.stderr));
    run(&shallow, &["config", "user.email", "test@example.com"]);
    run(&shallow, &["config", "user.name", "Test"]);

    let c1_sha = String::from_utf8(
        Command::new("git")
            .arg("-C").arg(origin.path())
            .args(["rev-list", "--max-parents=0", "HEAD"])
            .output().unwrap().stdout
    ).unwrap().trim().to_string();

    let err = changed_paths(shallow.path(), &c1_sha).unwrap_err();
    // Either RefNotFound (shallow doesn't have it) or NoMergeBase — both
    // are acceptable "ref unreachable" signals.
    assert!(
        matches!(err, GitError::RefNotFound { .. } | GitError::NoMergeBase { .. }),
        "got {err:?}"
    );
}
