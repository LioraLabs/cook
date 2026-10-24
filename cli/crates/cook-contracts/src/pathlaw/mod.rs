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

/// True when the first component of an output declaration is itself a pattern
/// (`**`, `*`, `*.env`, `.*`), so it can name files directly in the unit's
/// working directory (CS-0271). A leading `./` is ignored.
pub fn is_root_wildcard_output(s: &str) -> bool {
    strip_dot_slash(s).split('/').next().is_some_and(has_glob_meta)
}

/// A directory output (CS-0119): a trailing slash declares that Cook owns the
/// entire subtree rooted here. Its concrete file set is known only after the
/// command runs, so it is a terminal output like a glob.
pub fn is_dir_output(s: &str) -> bool {
    s.ends_with('/')
}

/// The one lexical "does this output stay inside the unit" rule (CS-0274),
/// read by registration, the build-owned sweep and the verify re-run.
///
/// True when output declaration `p` names the unit's working directory `wd`
/// itself, one of its ancestors, or anywhere outside it: `./`, `x/../`, `../`,
/// `/x`, `<wd>`, `<wd>/../x`. Lexical: `p` is read relative to `wd`, an absolute
/// `p` lying under `wd` is first rebased onto it (`<wd>/./dist/` is `./dist/`,
/// the form cook_pnpm declares), and any other absolute `p` leaves. Symlinks are
/// not followed, and glob metacharacters mean nothing here: `**/` is inside by
/// this rule, and refusing a pattern as a directory output is a separate check.
pub fn output_leaves_unit(p: &str, wd: &str) -> bool {
    let rel = unit_relative(p, wd);
    let rel = Path::new(&rel);
    escapes_base(rel) || normalize(rel).as_os_str().is_empty()
}

/// Why a directory output `p` cannot be swept as build-owned (CS-0274).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirOutputRefusal {
    /// A glob metacharacter anywhere in the path as written: Cook globs the
    /// whole string when it records and sweeps, so a `[` in the working dir's
    /// own name counts too.
    Pattern,
    /// It names the working directory, an ancestor, or somewhere outside it.
    LeavesUnit,
}

/// The one decision on whether directory output `p` may be swept, read by
/// registration and by the sweep (CS-0274). Lexical: a glob metacharacter
/// anywhere in `p` as written is a pattern (an absolute `p` under a `wd` named
/// with `[` included — the record and the sweep would read that name as a
/// glob); otherwise [`output_leaves_unit`] decides. `None` means it may be swept.
pub fn dir_output_refusal(p: &str, wd: &str) -> Option<DirOutputRefusal> {
    if has_glob_meta(p) {
        Some(DirOutputRefusal::Pattern)
    } else if output_leaves_unit(p, wd) {
        Some(DirOutputRefusal::LeavesUnit)
    } else {
        None
    }
}

/// `p` relative to `wd` when it is an absolute path under `wd`, else `p`
/// unchanged. The rebasing step of [`output_leaves_unit`].
pub fn unit_relative(p: &str, wd: &str) -> String {
    match rebase(p, wd.trim_end_matches('/'), "") {
        Some(rest) => rest.trim_start_matches('/').to_string(),
        None => p.to_string(),
    }
}

/// Rebase `p` from under `from` to under `to` by string prefix (not
/// `Path::strip_prefix`, which would drop the `./` segments recorded output
/// names keep). `None` when `p` is not `from` or inside it.
pub fn rebase(p: &str, from: &str, to: &str) -> Option<String> {
    match p.strip_prefix(from) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => Some(format!("{to}{rest}")),
        _ => None,
    }
}

/// A non-literal output entry whose concrete file set is resolved only after
/// the command runs: a glob pattern (CS-0085) or a directory output (CS-0119).
pub fn is_terminal_output(s: &str) -> bool {
    has_glob_meta(s) || is_dir_output(s)
}

/// Strip a leading `./` from a declared path string, so two spellings of one
/// path compare equal.
///
/// Deliberately weaker than [`normalize`], and string-shaped rather than
/// `Path`-shaped, because the declarations this compares include directory
/// outputs (CS-0119) and glob patterns, and `normalize` would drop the trailing
/// slash that IS the directory-output declaration and would rewrite a `..`
/// inside a pattern. The only equivalence it claims is the one every one of
/// those spellings shares.
///
/// It is here, rather than private to a caller, for the reason recorded in this
/// module's doc: the register phase's member-source check and the `after`
/// resolution of §22.1.3 are two readers of one rule, and a rule with two homes
/// is a rule that drifts.
pub fn strip_dot_slash(s: &str) -> &str {
    s.strip_prefix("./").unwrap_or(s)
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

/// A `covers` entry without its trailing `/`, `*` and `**` components, which
/// add no narrowing: `/usr/**` is `/usr`.
pub fn strip_wildcard_tail(entry: &str) -> String {
    let mut parts: Vec<&str> = entry.split('/').collect();
    while matches!(parts.last(), Some(&("" | "*" | "**"))) {
        parts.pop();
    }
    // an absolute entry never strips to nothing: `/**` is `/`
    if parts.is_empty() && entry.starts_with('/') { "/".into() } else { parts.join("/") }
}

/// True when `root` (`..`/`.` resolved lexically) is too broad for a `covers`
/// root: the filesystem root, a single top-level directory, or `home` or one of
/// its ancestors (`/var/lib` when home is `/var/lib/jenkins`).
/// A relative path is never judged here; resolve it first.
pub fn too_broad_root(root: &Path, home: Option<&Path>) -> bool {
    let r = normalize(root);
    r.has_root()
        && (r.components().filter(|c| matches!(c, Component::Normal(_))).count() <= 1
            || home.is_some_and(|h| normalize(h).starts_with(&r)))
}

fn covers_glob(s: &str) -> bool {
    s.contains(['*', '?', '[', '{'])
}

/// The literal root a `covers` entry stands for: its wildcard tail stripped,
/// then every component from the first glob one on, which can match anything
/// under the root — `/h*` stands for `/`, `~/.nvm/*/bin` for `~/.nvm`.
pub fn covers_root(entry: &str) -> String {
    let t = strip_wildcard_tail(entry);
    let comps: Vec<&str> = t.split('/').collect();
    let n = comps.iter().position(|c| covers_glob(c)).unwrap_or(comps.len());
    let root = comps[..n].join("/");
    if root.is_empty() && t.starts_with('/') { "/".into() } else { root }
}

/// The one breadth rule for a `covers` entry (§22.5.2.2). `at` is `None` at
/// registration, where only what is too broad on every host is judged: a root
/// of `/`, a top-level directory, `~`, or the whole declaring directory unless
/// a literal component below a glob narrows it (`**/node_modules`, not `?*` or
/// `**/*.ts`). At audit, `Some((cookfile_dir, home))` adds what needs the
/// host: a root that is the home directory or its ancestor, and a relative root climbing
/// out of the Cookfile's directory, judged as resolved.
pub fn covers_too_broad(entry: &str, at: Option<(&Path, Option<&Path>)>) -> bool {
    let root = covers_root(entry);
    let home_rest = root.strip_prefix('~').filter(|r| r.is_empty() || r.starts_with('/')).map(|r| r.trim_start_matches('/'));
    let absolute = Path::new(&root).has_root();
    // Only `*`, `?` and `**` may precede the narrowing literal: a `/` inside
    // `{...}` or `[...]` is not a component separator, and `[}]`/`\}` hide
    // where one ends.
    let t = strip_wildcard_tail(entry);
    let narrowed = !t.contains(['{', '[', '\\'])
        && t.split('/').skip_while(|c| !covers_glob(c)).any(|c| !matches!(c, "" | "." | "..") && !covers_glob(c));
    let lexical = match home_rest {
        Some(r) => normalize(Path::new(r)).as_os_str().is_empty(),
        None if absolute => too_broad_root(Path::new(&root), None),
        None => normalize(Path::new(&root)).as_os_str().is_empty() && !narrowed,
    };
    let Some((dir, home)) = at else { return lexical };
    lexical
        || match home_rest {
            Some(r) => home.is_some_and(|h| too_broad_root(&h.join(r), Some(h))),
            None if absolute => too_broad_root(Path::new(&root), home),
            // one that stays inside the Cookfile's directory is never broader than the project
            // one climbing out is judged as resolved, and may not come back to the
            // Cookfile's directory or an ancestor of it at all: below such a root
            // a glob can name the project itself (`../**/proj`)
            None => {
                let r = normalize(&dir.join(&root));
                escapes_base(Path::new(&root)) && (too_broad_root(&r, home) || normalize(dir).starts_with(&r))
            }
        }
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
