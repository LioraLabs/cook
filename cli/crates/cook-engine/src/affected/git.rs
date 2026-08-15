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
    // `--relative` scopes and re-roots the listing to the cwd (`-C
    // project_root`): without it, paths come back repo-root-relative, which
    // diverges from workspace-root-relative whenever the workspace lives
    // inside a larger git repository (COOK-274). `ls_untracked` needs no
    // flag; `ls-files` is cwd-relative by default.
    let out = run_git(project_root, &["diff", "--relative", "--name-only", "-z", range])?;
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
#[path = "tests/git_tests.rs"]
mod tests;
