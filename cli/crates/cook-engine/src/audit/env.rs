//! The audit's environment half: which programs never loaded the getenv
//! recorder, and which queried names a unit's key does not carry.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cook_contracts::pathlaw::normalize;

use super::{trace_calls, OPEN_CALLS};

/// Programs that ran in `text` without ever opening `recorder`: each pid's
/// trace is cut into segments at its successful `execve`s (a reused pid's new program starts with its own), and
/// a segment with no successful open of the recorder never loaded it.
pub fn unobserved_programs(text: &str, start_cwd: &Path, recorder: &Path) -> BTreeSet<PathBuf> {
    let recorder = normalize(recorder);
    let mut segments: BTreeMap<u32, (PathBuf, bool)> = BTreeMap::new();
    let mut out = BTreeSet::new();
    for (pid, name, a) in trace_calls(text, start_cwd) {
        let exec = matches!(name.as_str(), "execve" | "execveat") && !a.absent;
        if exec {
            if let Some((prog, false)) = segments.remove(&pid) {
                out.insert(prog);
            }
        }
        if exec {
            segments.insert(pid, (a.path, false));
        } else if OPEN_CALLS.contains(&name.as_str()) && !a.absent && a.path == recorder {
            if let Some(seg) = segments.get_mut(&pid) {
                seg.1 = true;
            }
        }
    }
    out.extend(segments.into_values().filter(|(_, seen)| !seen).map(|(prog, _)| prog));
    out
}

/// Names the audit never reports as unkeyed, on top of the key computation's
/// own denylist: locale (`LANG`, `LANGUAGE`, `LC_*`) and the dynamic loader
/// (`LD_*`) are queried by every process and say nothing about the step. They are
/// globs, matched by the key denylist's own matcher. The recorder's own variables are
/// ignored too (see [`env_ignored`]).
pub const AUDIT_IGNORED_ENV: &[&str] = &["LANG", "LANGUAGE", "LC_*", "LD_*"];

fn env_ignored(name: &str, denylist: &cook_cache::envkey::EnvDenylist) -> bool {
    denylist.is_ignored(name)
        || name == cook_envtrace::OUT_VAR
        || cook_envtrace::REFERENCES.iter().any(|(r, _)| *r == name)
}

/// The process-environment names a unit keys: every literal `getenv("NAME")`
/// of the probes it seals, whose results enter the key. Nothing else: a
/// declared variable keys its value, not the environment name it may have
/// been read from, which a text scan of config cannot attribute soundly.
pub(crate) fn keyed_env(meta: &cook_contracts::CacheMeta, units: &cook_contracts::RecipeUnits) -> BTreeSet<String> {
    let mut keyed = BTreeSet::new();
    for k in &meta.seal_keys {
        if let Some(p) = units.probes.iter().find(|p| &p.key == k) {
            keyed.extend(cook_contracts::lua_scan::scan_getenv_reads(&p.produce_source));
        }
    }
    keyed
}

/// The queried names a report should carry: neither keyed nor ignored.
pub(crate) fn unkeyed_env(
    queried: &BTreeSet<String>,
    keyed: &BTreeSet<String>,
    denylist: &cook_cache::envkey::EnvDenylist,
) -> BTreeSet<String> {
    let mut deny = denylist.clone();
    deny.extend_with(&AUDIT_IGNORED_ENV.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    queried.iter().filter(|n| !keyed.contains(*n) && !env_ignored(n, &deny)).cloned().collect()
}

/// One recorder file: a line per lookup, `pid tid index long count NAME` (index in
/// `environ`, `-` when unset; long: `1` when the value is 256 bytes or more, else `0`;
/// count: environ's length). A missing file means no
/// traced process loaded the recorder (see [`unobserved_programs`]): nothing queried.
/// Lookups that walk the whole environment in order are dropped (see [`walk_lookups`]).
pub(super) fn read_queried(path: &Path) -> BTreeSet<String> {
    let text = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let _ = std::fs::remove_file(path);
    let lookups: Vec<Lookup> = text.lines().filter_map(Lookup::parse).collect();
    let walked = walk_lookups(&lookups);
    lookups.iter().enumerate().filter(|(i, _)| !walked.contains(i)).map(|(_, l)| l.name.to_string()).collect()
}

/// One recorded lookup.
struct Lookup<'a> {
    thread: (&'a str, &'a str),
    index: Option<usize>,
    long: bool,
    count: usize,
    name: &'a str,
}

impl<'a> Lookup<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let mut f = line.splitn(6, ' ');
        let (pid, tid, index, long, count, name) = (f.next()?, f.next()?, f.next()?, f.next()?, f.next()?, f.next()?);
        let index = if index == "-" { None } else { Some(index.parse().ok()?) };
        let long = match long {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        (!name.is_empty()).then_some(Lookup { thread: (pid, tid), index, long, count: count.parse().ok()?, name })
    }
}

/// The positions of lookups that copy the environment rather than read it: on
/// one thread, a run that starts at `environ[0]` and steps through every entry
/// to the last in order. A name may repeat only back to back; other lookups may
/// fall between steps and are kept. This is how node enumerates `process.env`
/// (to spawn a child, spread or stringify it), once per name per access, and
/// would otherwise report every variable the caller happened to set. A copy
/// looks each name up k times, k being the shortest run among names of its
/// kind, long-valued (256 bytes or more: node looks those up again when the
/// value does not fit its buffer) or short; a name's lookups beyond its kind's
/// k are reads (a point read next to the copy, or a loop over the copy reading
/// the names it picks, as vite's `loadEnv` does), the rest dropped.
fn walk_lookups(lookups: &[Lookup]) -> BTreeSet<usize> {
    struct Walk {
        count: usize,
        at: usize,
        members: Vec<usize>,
    }
    let mut walks: BTreeMap<(&str, &str), Walk> = BTreeMap::new();
    let mut last: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    let mut walked = BTreeSet::new();
    // A finished copy's members are dropped once it can no longer grow.
    let settle = |w: &Walk, walked: &mut BTreeSet<usize>| {
        if w.at + 1 != w.count {
            return;
        }
        let runs: Vec<&[usize]> = w.members.chunk_by(|a, b| lookups[*a].index == lookups[*b].index).collect();
        for long in [false, true] {
            let kind: Vec<_> = runs.iter().filter(|r| lookups[r[0]].long == long).collect();
            let k = kind.iter().map(|r| r.len()).min().unwrap_or(0);
            walked.extend(kind.iter().flat_map(|r| r[..k].iter().copied()));
        }
    };
    for (i, l) in lookups.iter().enumerate() {
        let prev = last.insert(l.thread, i);
        let w = walks.get_mut(&l.thread);
        let done = w.as_ref().is_some_and(|w| w.at + 1 == w.count);
        let step = match (w, l.index) {
            // A repeat counts only straight after the walk's previous lookup.
            (Some(w), Some(x)) if w.count == l.count && x == w.at && w.members.last() == prev.as_ref() => Some(w),
            (Some(w), Some(x)) if w.count == l.count && x == w.at + 1 => Some(w),
            _ => None,
        };
        match step {
            Some(w) => {
                w.at = l.index.unwrap_or(w.at);
                w.members.push(i);
            }
            // In a one-entry environment a walk and a read look alike: it stays a read.
            None if l.index == Some(0) && l.count > 1 => {
                if let Some(old) = walks.insert(l.thread, Walk { count: l.count, at: 0, members: vec![i] }) {
                    settle(&old, &mut walked);
                }
            }
            // A finished walk ends at the first lookup that is not its last name again.
            None if done => {
                settle(&walks.remove(&l.thread).unwrap(), &mut walked);
            }
            None => {}
        }
    }
    walks.values().for_each(|w| settle(w, &mut walked));
    walked
}
