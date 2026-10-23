//! Resolving a `covers` entry (relative to the declaring Cookfile's directory,
//! or `~/` to HOME) into what the audit counts as covered.

use std::path::{Path, PathBuf};

use cook_contracts::consumes::ConsumesFilter;
use cook_contracts::pathlaw::{covers_too_broad, normalize};

/// What one resolved `covers` entry covers.
pub(super) enum Cover {
    Subtree(PathBuf),
    Pattern(String),
}

/// `None`: `~/` with HOME unset, nothing to resolve against.
pub(super) fn resolve(entry: &str, dir: &Path, home: Option<&Path>) -> Result<Option<Cover>, String> {
    let (base, rest) = match entry.strip_prefix("~/") {
        Some(r) => match home {
            Some(h) => (h, r),
            None => return Ok(None),
        },
        None => (dir, entry),
    };
    if covers_too_broad(entry, Some((dir, home))) {
        return Err(format!("covers entry {entry:?} is too broad; name the install root, not the filesystem or home"));
    }
    if rest.contains(['*', '?', '[', '{']) {
        let p = anchor_pattern(base, rest);
        ConsumesFilter::compile(std::slice::from_ref(&p)).map_err(|(_, e)| format!("covers entry {entry:?}: {e}"))?;
        Ok(Some(Cover::Pattern(p)))
    } else if !rest.is_empty() {
        Ok(Some(Cover::Subtree(normalize(&base.join(rest)))))
    } else {
        Ok(None)
    }
}

/// A covers glob anchored at `base`: its literal leading components (`./`,
/// `../`, `a/`, or a leading `/`) resolve like a path, then the glob tail
/// follows as written, so `./**/node_modules` matches what `**/node_modules` does.
pub(super) fn anchor_pattern(base: &Path, rest: &str) -> String {
    let (base, rest) = match rest.strip_prefix('/') {
        Some(r) => (Path::new("/"), r),
        None => (base, rest),
    };
    let comps: Vec<&str> = rest.split('/').collect();
    let split = comps.iter().position(|c| c.contains(['*', '?', '[', '{'])).unwrap_or(comps.len());
    let mut s = escape_glob(&normalize(&base.join(comps[..split].join("/"))));
    if !s.ends_with('/') {
        s.push('/');
    }
    s + &comps[split..].join("/")
}

/// `dir` with every glob metacharacter made literal, so a project path
/// containing `[` cannot change the pattern it anchors.
pub(super) fn escape_glob(dir: &Path) -> String {
    dir.to_string_lossy().chars().fold(String::new(), |mut s, c| {
        match c {
            '*' | '?' | '[' | ']' | '{' | '}' => s.extend(['[', c, ']']),
            '\\' => s.push_str("\\\\"),
            _ => s.push(c),
        }
        s
    })
}
