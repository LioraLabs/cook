//! Project-root sandbox for the Cook Lua I/O surface (CS-0045).
//!
//! `cook` step (and `test`/`chore`) Lua bodies are required to be hermetic
//! — their inputs and outputs must be derivable from the captured cache
//! fingerprint, and they must not silently read or write files outside
//! the project that owns the Cookfile. The sandbox enforces that
//! contract for the path-taking surfaces of `fs.*` and the Lua shell
//! escape hatches (`os.execute`, `io.popen`) that bypass `cook.sh`'s
//! working-directory rooting.
//!
//! `plate` step bodies are explicitly the user's "ship outside the
//! project" escape hatch (deploys, uploads, etc.) and run with
//! [`SandboxPolicy::Off`]. The execute-phase worker selects the policy
//! per work item; the register-phase VM is always [`Confined`].
//!
//! # Path resolution
//!
//! [`SandboxPolicy::resolve`] takes the user-supplied path and the
//! caller's working directory, resolves the final absolute target, and
//! checks that the target lies under the project root.
//!
//! - **Off**: returns `working_dir.join(path)` unchanged. Equivalent to
//!   the pre-CS-0045 behavior of `fs.*` for every step kind.
//! - **Confined**: rejects absolute paths whose canonical form escapes
//!   `project_root`, and rejects relative paths that would land outside
//!   `project_root` after `..` normalization.
//!
//! Canonicalization is done logically (lexical normalization) rather
//! than via `std::fs::canonicalize`, because `fs.write` and `fs.mkdir_p`
//! are required to succeed against paths that do not yet exist. A
//! caller that wants symlink-based escapes blocked must keep the
//! project root free of attacker-controlled symlinks; CS-0045 does not
//! attempt to defeat hostile symlinks already present in the project.

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Policy controlling whether `fs.*` and the Lua shell escape hatches
/// confine paths to the project root.
#[derive(Clone, Debug)]
pub enum SandboxPolicy {
    /// No confinement. Used by `plate` step Lua bodies, which are
    /// explicitly allowed to ship outputs outside the project root
    /// (deploys, uploads, etc.) per §{recipes.plate-step}.
    Off,
    /// Confine path arguments to `project_root`. Used by `cook`-,
    /// `test`-, and `chore`-step Lua bodies and by all register-phase
    /// Lua execution.
    Confined { project_root: PathBuf },
}

/// Live source of a sandbox policy.
///
/// Mirrors the [`crate::WorkingDirSource`] split: the register-phase
/// VM uses `Static` (one project root for the lifetime of the VM) and
/// the execute-phase worker pool uses `Live` so each work item can
/// install its own policy (CS-0017 multi-Cookfile imports + per-item
/// step-kind selection).
#[derive(Clone, Debug)]
pub enum SandboxSource {
    Static(SandboxPolicy),
    Live(Arc<Mutex<SandboxPolicy>>),
}

impl SandboxSource {
    /// Off-source convenience: `Static(SandboxPolicy::Off)`. The
    /// pre-CS-0045 behavior — fs.* never rejects.
    pub fn off() -> Self {
        SandboxSource::Static(SandboxPolicy::Off)
    }

    /// Confined-source convenience: `Static(SandboxPolicy::Confined { ... })`.
    pub fn confined(project_root: PathBuf) -> Self {
        SandboxSource::Static(SandboxPolicy::Confined { project_root })
    }

    /// Resolve the policy at call time. `Live`'s mutex is held only
    /// for the clone.
    pub fn resolve(&self) -> SandboxPolicy {
        match self {
            SandboxSource::Static(p) => p.clone(),
            SandboxSource::Live(slot) => slot.lock().expect("sandbox policy lock").clone(),
        }
    }
}

/// Errors raised by the sandbox check. Surfaced to Lua as runtime
/// errors with diagnostics naming the offending path.
#[derive(Debug)]
pub enum SandboxError {
    /// The path resolves outside `project_root`.
    Escape {
        api: &'static str,
        path: String,
        project_root: PathBuf,
    },
    /// The Lua-side shell escape hatch (`os.execute` / `io.popen`) is
    /// disabled in this step-kind context.
    ShellDisabled { api: &'static str },
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxError::Escape {
                api,
                path,
                project_root,
            } => write!(
                f,
                "{api}: path {path:?} escapes project root {}. \
                 cook/test/chore step Lua bodies are confined to the \
                 project root; use a `plate` step to ship outside)",
                project_root.display()
            ),
            SandboxError::ShellDisabled { api } => write!(
                f,
                "{api}: Lua-side shell escape hatch is disabled in \
                 cook/test/chore step bodies; use cook.sh \
                 (which runs with the recipe's working_dir) or move \
                 the call to a `plate` step"
            ),
        }
    }
}

impl SandboxPolicy {
    /// Resolve a user-supplied path against `working_dir` and check it
    /// against the policy.
    ///
    /// On success, returns the absolute target the caller should pass
    /// to the OS. On failure (path escapes project root), returns a
    /// [`SandboxError::Escape`].
    ///
    /// `working_dir` MAY be relative. In that case the candidate is
    /// promoted to an absolute path against the process's current
    /// working directory before the prefix check runs, so a CLI that
    /// invokes `cook -f Cookfile.x` (whose `cli.file.parent()` is
    /// `"."`) does not produce false negatives against an absolute
    /// `project_root`.
    pub fn resolve(
        &self,
        api: &'static str,
        working_dir: &Path,
        user_path: &str,
    ) -> Result<PathBuf, SandboxError> {
        let candidate = working_dir.join(user_path);
        match self {
            SandboxPolicy::Off => Ok(candidate),
            SandboxPolicy::Confined { project_root } => {
                let normalized = absolutize(&candidate);
                let root_normalized = absolutize(project_root);
                if path_starts_with(&normalized, &root_normalized) {
                    Ok(candidate)
                } else {
                    Err(SandboxError::Escape {
                        api,
                        path: user_path.to_string(),
                        project_root: root_normalized,
                    })
                }
            }
        }
    }

    /// Returns true if Lua-side shell escape hatches (`os.execute`,
    /// `io.popen`) are permitted under this policy. They are always
    /// permitted under `Off`; under `Confined` they are denied because
    /// arbitrary shell text bypasses the path-confinement check.
    pub fn shell_escape_hatches_enabled(&self) -> bool {
        matches!(self, SandboxPolicy::Off)
    }
}

/// Promote a path to an absolute, lexically-normalized form. If `p`
/// is relative the result is `cwd.join(p)` after normalization, where
/// `cwd` is the process's current working directory at call time. If
/// the cwd is unavailable (a degenerate condition Cook should never
/// hit), falls back to the lexically-normalized relative path.
///
/// Used by [`SandboxPolicy::resolve`] so the prefix check is always
/// over absolute paths even when the caller's `working_dir` was
/// passed in relative form (e.g. `Path::new(".")` from the CLI's
/// `cli.file.parent()` shortcut).
fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        lexical_normalize(p)
    } else {
        match std::env::current_dir() {
            Ok(cwd) => lexical_normalize(&cwd.join(p)),
            Err(_) => lexical_normalize(p),
        }
    }
}

/// Lexically normalize a path: strip `.` components and apply `..`
/// components in-place. Does not touch the filesystem; works for paths
/// that do not yet exist (which `fs.write` / `fs.mkdir_p` rely on).
///
/// The result is absolute when the input is absolute; otherwise it is
/// the input with `.` and `..` resolved as far as possible.
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Prefix(pre) => {
                out.push(pre.as_os_str());
            }
            Component::RootDir => {
                out.push(comp.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop the trailing component if there is one and it is
                // not itself a parent-dir (the input was already
                // unresolvable above the root, e.g. `../..` on its own).
                let pops_ok = match out.components().next_back() {
                    Some(Component::Normal(_)) => true,
                    _ => false,
                };
                if pops_ok {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            Component::Normal(s) => {
                out.push(s);
            }
        }
    }
    out
}

/// Check that `child` is a prefix-equal descendant of `parent` after
/// lexical normalization. Equality is component-wise; a trailing
/// path-separator on either side is irrelevant.
fn path_starts_with(child: &Path, parent: &Path) -> bool {
    let mut child_iter = child.components();
    for parent_comp in parent.components() {
        match child_iter.next() {
            Some(c) if c == parent_comp => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/proj")
    }

    fn confined() -> SandboxPolicy {
        SandboxPolicy::Confined { project_root: root() }
    }

    #[test]
    fn off_passes_everything() {
        let p = SandboxPolicy::Off;
        assert!(p.resolve("fs.read", Path::new("/proj"), "../etc/passwd").is_ok());
        assert!(p.resolve("fs.read", Path::new("/proj"), "/etc/passwd").is_ok());
    }

    #[test]
    fn confined_allows_relative_inside() {
        let p = confined();
        assert!(p.resolve("fs.read", Path::new("/proj"), "src/main.rs").is_ok());
        assert!(p.resolve("fs.read", Path::new("/proj"), "./build/x").is_ok());
    }

    #[test]
    fn confined_allows_subdir_cwd() {
        let p = confined();
        // CS-0017: imported Cookfiles run with their own subdir as
        // working_dir, but the project_root is still /proj. A relative
        // path from the subdir cwd that stays inside /proj is fine.
        assert!(p.resolve("fs.read", Path::new("/proj/lib"), "data.txt").is_ok());
        assert!(p.resolve("fs.read", Path::new("/proj/lib"), "../data.txt").is_ok());
    }

    #[test]
    fn confined_rejects_absolute_outside() {
        let p = confined();
        let err = p.resolve("fs.read", Path::new("/proj"), "/etc/passwd").unwrap_err();
        assert!(matches!(err, SandboxError::Escape { .. }), "got {err}");
    }

    #[test]
    fn confined_rejects_relative_traversal() {
        let p = confined();
        let err = p.resolve("fs.read", Path::new("/proj/lib"), "../../etc/passwd").unwrap_err();
        assert!(matches!(err, SandboxError::Escape { .. }), "got {err}");
    }

    #[test]
    fn confined_rejects_dotdot_to_above_root() {
        let p = confined();
        // /proj/.. = /, not inside /proj
        let err = p.resolve("fs.read", Path::new("/proj"), "../somefile").unwrap_err();
        assert!(matches!(err, SandboxError::Escape { .. }));
    }

    #[test]
    fn confined_allows_absolute_inside() {
        // An absolute path that points into the project is fine.
        let p = confined();
        assert!(p.resolve("fs.read", Path::new("/proj"), "/proj/src/x.rs").is_ok());
        assert!(p.resolve("fs.read", Path::new("/proj"), "/proj").is_ok());
    }

    #[test]
    fn shell_escape_disabled_under_confined() {
        assert!(!confined().shell_escape_hatches_enabled());
        assert!(SandboxPolicy::Off.shell_escape_hatches_enabled());
    }

    #[test]
    fn lexical_normalize_basic() {
        assert_eq!(lexical_normalize(Path::new("/a/b/./c")), PathBuf::from("/a/b/c"));
        assert_eq!(lexical_normalize(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
        assert_eq!(lexical_normalize(Path::new("a/b/../c")), PathBuf::from("a/c"));
        assert_eq!(lexical_normalize(Path::new("../x")), PathBuf::from("../x"));
    }

    #[test]
    fn live_source_observes_post_install_changes() {
        let slot = Arc::new(Mutex::new(SandboxPolicy::Off));
        let src = SandboxSource::Live(Arc::clone(&slot));
        assert!(matches!(src.resolve(), SandboxPolicy::Off));

        *slot.lock().unwrap() = SandboxPolicy::Confined { project_root: root() };
        assert!(matches!(src.resolve(), SandboxPolicy::Confined { .. }));
    }
}
