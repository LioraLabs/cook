//! Pure path and output-shape rules (COOK-418, COOK-414).
//!
//! These decide what a declared path *is* without asking the filesystem: is it
//! a glob pattern, does it name a directory output, does it escape its base.
//! Resolving any of them against a real tree is a different job and lives with
//! the IO, in `cook-cache`.
//!
//! `normalize` in particular was implemented twice inside one crate, as
//! `lexically_normalize` and `normalize_lexical`, both private, differently
//! named, byte-identical in behaviour, with no agreement test (COOK-414). No
//! dependency edge was being refused, so the deliberate-copy protocol did not
//! even offer cover: they were simply two spellings of one rule that nobody
//! had noticed were the same.

use std::path::{Component, Path, PathBuf};

/// True when `s` contains a glob metacharacter.
///
/// Deliberately not `{`: brace alternation is not glob syntax here, so
/// `out/{a,b}.txt` is a literal path.
pub fn has_glob_meta(s: &str) -> bool {
    s.bytes().any(|b| matches!(b, b'*' | b'?' | b'['))
}

/// A directory output (CS-0119): a trailing slash declares that Cook owns the
/// entire subtree rooted here. Its concrete file set is known only after the
/// command runs, so it is a terminal output like a glob.
pub fn is_dir_output(s: &str) -> bool {
    s.ends_with('/')
}

/// A non-literal output entry whose concrete file set is resolved only after
/// the command runs: a glob pattern (CS-0085) or a directory output (CS-0119).
pub fn is_terminal_output(s: &str) -> bool {
    has_glob_meta(s) || is_dir_output(s)
}

/// Resolve `.` and `..` lexically, without touching the filesystem.
///
/// Lexical rather than canonical on purpose: a declared input may not exist
/// yet when this runs, and `canonicalize` would fail on it. `..` pops the
/// previous component, so `a/b/../c` is `a/c` and a leading `..` pops nothing
/// and disappears; see [`escapes_base`] for the check that cares about that.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True when `path` leaves the tree it is declared relative to.
///
/// Two ways to leave it, and both count: walking above the base with `..`, and
/// being absolute in the first place (a `RootDir` or a Windows `Prefix`).
///
/// Counts depth as it goes rather than normalising first, because
/// [`normalize`] silently drops a leading `..` and would report the escape as
/// an ordinary relative path.
pub fn escapes_base(path: &Path) -> bool {
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return true,
        }
    }
    false
}

#[cfg(test)]
#[path = "tests/pathlaw_tests.rs"]
mod tests;
