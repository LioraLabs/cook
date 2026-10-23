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
/// (`LD_*`) are queried by every process and say nothing about the step. A
/// trailing `*` is a prefix glob. The recorder's own output variable is
/// ignored too (see [`env_ignored`]).
pub const AUDIT_IGNORED_ENV: &[&str] = &["LANG", "LANGUAGE", "LC_*", "LD_*"];

fn env_ignored(name: &str, denylist: &cook_cache::envkey::EnvDenylist) -> bool {
    denylist.is_ignored(name)
        || name == cook_envtrace::OUT_VAR
        || AUDIT_IGNORED_ENV.iter().any(|p| match p.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => name == *p,
        })
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
    queried.iter().filter(|n| !keyed.contains(*n) && !env_ignored(n, denylist)).cloned().collect()
}

/// One recorder file: names, one per line. A missing file means no traced
/// process loaded the recorder (see [`unobserved_programs`]): nothing queried.
pub(super) fn read_queried(path: &Path) -> BTreeSet<String> {
    let text = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let _ = std::fs::remove_file(path);
    text.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}
