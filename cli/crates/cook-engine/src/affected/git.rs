//! Git driver for `cook affected` — shells out to `git` to discover the set
//! of files changed since a given reference, including working tree state.
//!
//! Three-dot merge-base semantics (matching Turborepo's `--filter=[ref]`):
//!   1. resolve `<ref>` to a commit
//!   2. compute merge-base of `<ref>` and HEAD
//!   3. diff merge-base..HEAD
//!   4. union uncommitted (staged + unstaged) diff against HEAD
//!   5. union untracked-but-not-ignored files

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("not a git repository: {0}")]
    NotAGitRepo(PathBuf),
    #[error("git ref '{reference}' not found: {stderr}")]
    RefNotFound { reference: String, stderr: String },
    #[error("no merge-base between '{reference}' and HEAD (shallow clone? try `git fetch --deepen`)")]
    NoMergeBase { reference: String },
    #[error("git executable not found on PATH")]
    GitNotInstalled,
    #[error("failed to spawn git: {0}")]
    Spawn(#[from] io::Error),
}

/// Return the set of changed paths (repo-relative) since `since_ref`,
/// including working-tree changes (staged + unstaged + untracked-non-ignored).
pub fn changed_paths(
    project_root: &Path,
    since_ref: &str,
) -> Result<BTreeSet<PathBuf>, GitError> {
    ensure_inside_work_tree(project_root)?;
    let merge_base = resolve_merge_base(project_root, since_ref)?;

    let mut set = BTreeSet::new();
    set.extend(diff_name_only(project_root, &format!("{merge_base}..HEAD"))?);
    set.extend(diff_name_only(project_root, "HEAD")?);
    set.extend(ls_untracked(project_root)?);
    Ok(set)
}

fn run_git(project_root: &Path, args: &[&str]) -> Result<std::process::Output, GitError> {
    Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                GitError::GitNotInstalled
            } else {
                GitError::Spawn(e)
            }
        })
}

fn ensure_inside_work_tree(project_root: &Path) -> Result<(), GitError> {
    let out = run_git(project_root, &["rev-parse", "--is-inside-work-tree"])?;
    if !out.status.success()
        || String::from_utf8_lossy(&out.stdout).trim() != "true"
    {
        return Err(GitError::NotAGitRepo(project_root.to_path_buf()));
    }
    Ok(())
}

fn resolve_merge_base(project_root: &Path, since_ref: &str) -> Result<String, GitError> {
    // Validate the ref exists first; otherwise merge-base's error is opaque.
    let verify = run_git(
        project_root,
        &["rev-parse", "--verify", &format!("{since_ref}^{{commit}}")],
    )?;
    if !verify.status.success() {
        return Err(GitError::RefNotFound {
            reference: since_ref.to_string(),
            stderr: String::from_utf8_lossy(&verify.stderr).into_owned(),
        });
    }

    let mb = run_git(project_root, &["merge-base", since_ref, "HEAD"])?;
    if !mb.status.success() {
        return Err(GitError::NoMergeBase {
            reference: since_ref.to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&mb.stdout).trim().to_string())
}

fn diff_name_only(project_root: &Path, range: &str) -> Result<BTreeSet<PathBuf>, GitError> {
    let out = run_git(project_root, &["diff", "--name-only", "-z", range])?;
    if !out.status.success() {
        // diff against HEAD on a brand-new repo with no commits yet returns
        // non-zero; treat as empty rather than error.
        return Ok(BTreeSet::new());
    }
    Ok(parse_nul_separated(&out.stdout))
}

fn ls_untracked(project_root: &Path) -> Result<BTreeSet<PathBuf>, GitError> {
    let out = run_git(
        project_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    Ok(parse_nul_separated(&out.stdout))
}

fn parse_nul_separated(bytes: &[u8]) -> BTreeSet<PathBuf> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(String::from_utf8_lossy(s).into_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
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
}
