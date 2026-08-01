//! Fingerprint and cache-key computation for the Cook build system.
//!
//! This crate is the "what changed?" surface: pure functions that compute
//! content hashes, env contributions, probe fingerprints, and the
//! SHA-256 cache keys that address artifacts in any backend. It also defines
//! the `CacheBackend` trait — the seam the persistence layer (filesystem,
//! Cook Cloud, etc.) implements.
//!
//! `cook-cache` provides the v3 filesystem backend and the recipe-cache
//! manager built on top of these primitives.

pub mod backend;
pub mod check;
pub mod consumes;
pub mod context;
pub mod envkey;
pub mod evict;
pub mod probe;
pub mod record;
pub mod statmemo;

use std::collections::BTreeSet;
use std::path::Path;

use cook_contracts::cache::DeclaredInput;

pub use backend::{
    ArtifactMeta, BackendError, BackendResult, CacheBackend, CloudKey, CloudKeyInputs,
    DISCOVERED_INPUT_SETS_CAP, DISCOVERED_INPUT_SETS_INDEX, DISCOVERED_INPUT_SETS_PATH,
    DISCOVERED_INPUTS_MANIFEST_INDEX, DISCOVERED_INPUTS_MANIFEST_PATH, OBSERVATION_INDEX,
    OBSERVATION_PATH, artifact_key, cloud_key, recipe_namespace,
};
pub use check::{
    FetchOutcome, RebuildReason, RebuildResult, RestoreCtx, fetch_by_key, fetch_observation, shared_observation,
    hash_env, hash_file, hash_input_paths, hash_reader, needs_rebuild_cook,
    read_discovered_input_sets, stat_mtime,
};
pub use consumes::ConsumesFilter;
pub use context::{ProbeFingerprintInputs, compute_probe_fingerprint};
pub use envkey::{EnvDenylist, env_contribution};
pub use evict::{
    DEFAULT_LOW_WATER, EvictPlan, EvictPolicy, SIZE_SWEEP_EXEMPT_KINDS, is_size_sweep_exempt,
    plan_eviction,
};
pub use probe::{resolve_probe_inputs, resolve_tool_path, tool_identity};
pub use record::{CACHE_VERSION, FileRecord, StepEntry};
pub use statmemo::stat_mtime_memo;

/// Hash a string (for command templates, env vars, etc.)
pub fn hash_str(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

// A test-unit fingerprint used to live here: `FingerprintInputs` and
// `compute_test_fingerprint`, a second hash function over a second input
// struct, for the one unit kind that had its own store. CS-0186 gave every unit
// one record under one key, and both went with the store — `needs_rebuild_cook`
// judges a test unit now, on the same determinants as any other.

/// Returns true if the string contains any glob metacharacter recognised by
/// the reference implementation's `glob = "0.3"` matcher: `*`, `?`, `[`.
///
/// CS-0085 specifies these three characters as the glob metacharacter set.
/// `{` is intentionally excluded — `glob` 0.3 does not support brace
/// alternation, so a string like "out/{a,b}.txt" is treated as a literal
/// path.
pub fn has_glob_meta(s: &str) -> bool {
    s.bytes().any(|b| matches!(b, b'*' | b'?' | b'['))
}

/// A directory output (CS-0119): a trailing slash declares that Cook owns the
/// entire subtree rooted here. Its concrete file set is known only after the
/// command runs, so it is a terminal output like a glob.
pub fn is_dir_output(s: &str) -> bool {
    s.ends_with('/')
}

/// A non-literal output entry whose concrete file set is resolved only after the
/// command runs: a glob pattern (CS-0085) or a directory output (CS-0119).
pub fn is_terminal_output(s: &str) -> bool {
    has_glob_meta(s) || is_dir_output(s)
}

/// A unit's declared inputs, with every pattern entry expanded against the
/// tree and the expansion narrowed by an optional `consumes` allowlist
/// (§17.1.1.2, CS-0186).
///
/// **One function, every unit.** Nothing here asks what kind of step produced
/// the unit. A `cook` unit whose inputs are all paths gets its declared list
/// back unchanged; a unit that declared an input its producer had not yet
/// written — which is how any unit declares a consumed output — gets that entry
/// expanded here instead.
///
/// **Nothing here re-reads a string to decide what it is.** Whether an entry is
/// a path or a pattern was settled where the entry entered the declaration
/// (`cook.add_unit`), by the phase that knew where it came from. Deciding it
/// here, by scanning for `*`, `?` and `[`, is what §17.1.1.2 forbids and why:
/// those are ordinary filename characters, so a real `pages/[id].tsx` would be
/// expanded, match nothing, and vanish from the key — on this side and on the
/// recording side alike, so the two agree and the unit hits over a file that
/// changed.
///
/// Called when the unit is READY, so a declared `dist/**` names the files its
/// producer has by then written (§18). Pattern entries are normalised exactly
/// as the producing unit normalises them when capturing the same declaration
/// (CS-0085): one declaration MUST NOT resolve to two different file sets
/// depending on which side is looking at it, and a bare trailing `**` silently
/// resolving to nothing is the under-keying direction.
///
/// **Order is preserved and duplicates are dropped.** `check_inputs` compares
/// the recorded and current lists element-wise, so sorting the result would
/// read as an input-set change on every unit in the project.
pub fn resolve_declared_inputs(
    inputs: &[DeclaredInput],
    consumes: &[String],
    working_dir: &Path,
) -> Vec<String> {
    // Pass 1: expand, keeping the pattern-derived paths apart from the ones
    // the unit named outright. Only the former are `consumes`'s business
    // (§17.1.1.2 rule 1): a filter able to delete a declared path would drop a
    // determinant the declaration states in so many words.
    let mut resolved: Vec<(String, bool)> = Vec::with_capacity(inputs.len());
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut from_pattern: BTreeSet<String> = BTreeSet::new();
    for entry in inputs {
        if entry.is_pattern() {
            let pattern = normalize_glob_pattern(&entry.path);
            for rel in resolve_glob(working_dir, &pattern) {
                from_pattern.insert(rel.clone());
                if seen.insert(rel.clone()) {
                    resolved.push((rel, true));
                }
            }
        } else if seen.insert(entry.path.clone()) {
            resolved.push((entry.path.clone(), false));
        }
    }
    if consumes.is_empty() {
        return resolved.into_iter().map(|(p, _)| p).collect();
    }
    // A path reached BOTH ways is pattern-derived for narrowing purposes: an
    // artifact excluded from one pattern's expansion must not re-enter through
    // another pattern that also covers it. It is only safe from the filter when
    // the unit named it as a path.
    let candidates: Vec<String> = resolved
        .iter()
        .filter(|(p, by_pattern)| *by_pattern || from_pattern.contains(p))
        .map(|(p, _)| p.clone())
        .collect();
    if candidates.is_empty() {
        return resolved.into_iter().map(|(p, _)| p).collect();
    }
    // CS-0175: narrowing errs toward the under-keyed direction, so a filter
    // matching none of the candidates is inert rather than emptying the set.
    // `ConsumesFilter::select` holds that rule; a pattern that fails to compile
    // (rejected at register phase, so unreachable here) keeps the full set for
    // the same reason.
    let admitted: BTreeSet<String> = match ConsumesFilter::compile(consumes) {
        Ok(filter) => filter
            .select(&candidates, |p| p.clone())
            .into_iter()
            .cloned()
            .collect(),
        Err(_) => candidates.iter().cloned().collect(),
    };
    resolved
        .into_iter()
        .filter(|(p, by_pattern)| {
            if *by_pattern || from_pattern.contains(p) {
                admitted.contains(p)
            } else {
                true
            }
        })
        .map(|(p, _)| p)
        .collect()
}

pub fn normalize_glob_pattern(pattern: &str) -> std::borrow::Cow<'_, str> {
    if pattern == "**" {
        std::borrow::Cow::Borrowed("**/*")
    } else if pattern.ends_with("/**") {
        std::borrow::Cow::Owned(format!("{pattern}/*"))
    } else if pattern.ends_with('/') {
        std::borrow::Cow::Owned(format!("{pattern}**/*"))
    } else {
        std::borrow::Cow::Borrowed(pattern)
    }
}

pub fn resolve_ingredient_glob(
    member_root: &Path,
    workspace_root: &Path,
    raw: &str,
) -> Result<BTreeSet<String>, String> {
    let anchored = raw.strip_prefix("//");
    let anchored_escapes = anchored.is_some_and(|pattern| {
        Path::new(pattern).components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    });
    if (raw.starts_with('/') && anchored.is_none()) || raw.starts_with("///")
        || matches!(anchored, Some("") | Some(".."))
        || anchored_escapes
    {
        return Err(format!("malformed workspace anchor in ingredient pattern {raw:?}: use //"));
    }
    if anchored.is_none() && lexically_escapes_base(Path::new(raw)) {
        return Err(format!(
            "ingredient pattern {raw:?} escapes member root"
        ));
    }
    let (root, pattern) = anchored.map_or((member_root, raw), |p| (workspace_root, p));
    let full_pattern = root.join(normalize_glob_pattern(pattern).as_ref());
    let paths = glob::glob(&full_pattern.to_string_lossy())
        .map_err(|e| format!("invalid ingredient glob {raw:?}: {e}"))?;
    let resolved = paths
        .map(|entry| entry.map_err(|e| format!("failed to resolve ingredient glob {raw:?}: {e}")))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|p| !matches!(std::fs::metadata(p), Ok(m) if m.is_dir()))
        .map(|p| relative_path(member_root, &lexically_normalize(&p)))
        .collect();
    Ok(resolved)
}

fn lexically_escapes_base(path: &Path) -> bool {
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if depth > 0 => depth -= 1,
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return true,
        }
    }
    false
}

fn lexically_normalize(path: &Path) -> std::path::PathBuf {
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn relative_path(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut relative = std::path::PathBuf::new();
    for _ in common..from.len() {
        relative.push("..");
    }
    for component in &to[common..] {
        relative.push(component.as_os_str());
    }
    relative.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
#[path = "tests/ingredient_glob_tests.rs"]
mod ingredient_glob_tests;

/// Reconcile a build-owned directory output (CS-0119) so the subtree rooted at
/// `working_dir/root` contains exactly `kept` (paths relative to `working_dir`,
/// in the same form `resolve_glob` returns). Deletes every regular file under the
/// subtree not in `kept`, then prunes directories left empty. Deletion is bounded
/// strictly to the subtree; the root directory itself is preserved.
pub fn reconcile_dir_output(working_dir: &Path, root: &str, kept: &BTreeSet<String>) {
    // COOK-306: sweeping strays writes to the tree, so no memoised mtime can
    // be trusted afterwards.
    statmemo::disarm();
    let root = root.trim_end_matches('/');
    let present = resolve_glob(working_dir, &format!("{root}/**/*"));
    for rel in &present {
        if !kept.contains(rel) {
            let _ = std::fs::remove_file(working_dir.join(rel));
        }
    }
    prune_empty_dirs_keeping(&working_dir.join(root), working_dir, kept);
}

/// Workspace-relative paths of every EMPTY directory at or under `root`
/// (which is itself workspace-relative, no trailing slash). Returns paths with
/// forward slashes, relative to `working_dir`. An empty `root` dir is itself
/// reported. Used so directory outputs round-trip empty subdirs through the
/// cache. Returns an empty vec if `root` doesn't exist or isn't a dir.
pub fn empty_dirs_under(working_dir: &Path, root: &str) -> Vec<String> {
    let base = working_dir.join(root);
    let mut out = Vec::new();
    fn walk(dir: &Path, working_dir: &Path, out: &mut Vec<String>) {
        let entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(Result::ok).collect(),
            Err(_) => return,
        };
        let mut has_child = false;
        for e in &entries {
            let p = e.path();
            // Use symlink_metadata so a symlink-to-dir is NOT recursed (it's a
            // symlink output, not a dir to walk).
            match std::fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_dir() => {
                    has_child = true;
                    walk(&p, working_dir, out);
                }
                Ok(_) => {
                    has_child = true;
                }
                Err(_) => {}
            }
        }
        if !has_child {
            if let Ok(rel) = dir.strip_prefix(working_dir) {
                // forward-slash normalize
                let s = rel.to_string_lossy().replace('\\', "/");
                if !s.is_empty() {
                    out.push(s);
                }
            }
        }
    }
    if base.is_dir() {
        walk(&base, working_dir, &mut out);
    }
    out
}

/// Recursively remove empty subdirectories of `dir`, but never remove a
/// directory whose workspace-relative (forward-slash) path is in `kept` — these
/// are recorded empty-dir outputs (CS-0119) restored on a cache hit, so pruning
/// them on the same hit would defeat the round-trip (COOK-180). A kept child
/// also marks its parent non-empty so the parent survives too. Returns true if
/// `dir` is empty after the sweep. `dir` itself is not removed by this call (its
/// parent decides), so the directory-output root is preserved. Symbolic links
/// are never followed (`symlink_metadata`): a symlinked directory is treated as
/// a leaf entry, so reconciliation cannot recurse outside the subtree
/// (COOK-109).
fn prune_empty_dirs_keeping(dir: &Path, working_dir: &Path, kept: &BTreeSet<String>) -> bool {
    let mut empty = true;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            // symlink_metadata: do NOT follow links when classifying.
            let is_real_dir = matches!(std::fs::symlink_metadata(&p), Ok(m) if m.is_dir());
            if is_real_dir {
                let child_empty = prune_empty_dirs_keeping(&p, working_dir, kept);
                let rel = p
                    .strip_prefix(working_dir)
                    .ok()
                    .map(|r| r.to_string_lossy().replace('\\', "/"));
                let is_kept = rel.as_deref().map(|r| kept.contains(r)).unwrap_or(false);
                if child_empty && !is_kept {
                    let _ = std::fs::remove_dir(&p);
                } else {
                    empty = false;
                }
            } else {
                empty = false;
            }
        }
    }
    empty
}

/// Helper to resolve a glob pattern into a set of files.
///
/// Sub-directory matches are dropped (CS-0064): every consumer of this
/// helper feeds the results into cook's file-hashing path, where a
/// directory entry has no hashable bytes.
pub fn resolve_glob(root: &Path, pattern: &str) -> BTreeSet<String> {
    let full_pattern = root.join(pattern);
    let prefix = root.to_string_lossy().to_string();

    let paths = match glob::glob(&full_pattern.to_string_lossy()) {
        Ok(p) => p,
        Err(_) => return BTreeSet::new(),
    };

    paths
        .filter_map(Result::ok)
        .filter(|p| !matches!(std::fs::metadata(p), Ok(m) if m.is_dir()))
        .map(|p| {
            let path_str = p.to_string_lossy().to_string();
            path_str
                .strip_prefix(&prefix)
                .unwrap_or(&path_str)
                .trim_start_matches('/')
                .to_string()
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/fingerprint_tests.rs"]
mod tests;
