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
pub mod context;
pub mod envkey;
pub mod evict;
pub mod probe;
pub mod record;
pub mod statmemo;

use std::collections::BTreeSet;
use std::path::Path;

use sha2::{Digest, Sha256};

pub use backend::{
    artifact_key, cloud_key, recipe_namespace, ArtifactMeta, BackendError, BackendResult,
    CacheBackend, CloudKey, CloudKeyInputs, DISCOVERED_INPUTS_MANIFEST_INDEX,
    DISCOVERED_INPUTS_MANIFEST_PATH, DISCOVERED_INPUT_SETS_CAP, DISCOVERED_INPUT_SETS_INDEX,
    DISCOVERED_INPUT_SETS_PATH,
};
pub use check::{
    fetch_by_key, hash_env, hash_file, hash_input_paths, hash_reader,     needs_rebuild_cook, needs_rebuild_plate, read_discovered_input_sets, stat_mtime, FetchOutcome,
    RebuildReason, RebuildResult, RestoreCtx,
};
pub use context::{compute_probe_fingerprint, ProbeFingerprintInputs};
pub use evict::{
    is_size_sweep_exempt, plan_eviction, EvictPlan, EvictPolicy, DEFAULT_LOW_WATER,
    SIZE_SWEEP_EXEMPT_KINDS,
};
pub use probe::{resolve_probe_inputs, resolve_tool_path, tool_identity};
pub use envkey::{env_contribution, EnvDenylist};
pub use record::{FileRecord, StepEntry, CACHE_VERSION};
pub use statmemo::stat_mtime_memo;

/// Hash a string (for command templates, env vars, etc.)
pub fn hash_str(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

// ---------------------------------------------------------------------------
// Test-unit fingerprint (CS-0061 §3.3)
// ---------------------------------------------------------------------------

/// Environmental and file-system inputs that contribute to a test unit's
/// content-addressed fingerprint. Matches the analogous inputs used for
/// recipe-step fingerprints but is kept separate so the test cache can
/// evolve independently.
///
/// All four `Vec` fields are sorted before hashing, so insertion order is
/// irrelevant — callers should not pre-sort them.
#[derive(Debug, Default, Clone)]
pub struct FingerprintInputs {
    /// `(path, content_fingerprint)` for cook-step outputs consumed by the test.
    pub cook_outputs: Vec<(String, String)>,
    /// `(path, content_fingerprint)` for dep-step outputs consumed by the test.
    pub dep_outputs: Vec<(String, String)>,
    /// `(key, value)` for env-var contributions.
    pub env_keys: Vec<(String, String)>,
    /// CS-0159: `(probe_key, canonical_value)` for every probe in the test
    /// unit's effective seal set (§17.4 rule 1). Resolved from the execute-phase
    /// `ProbeValueStore` at ready time by the engine, using the same
    /// absent-key-folds-to-empty-string rule as a cook unit's
    /// `resolve_sealed_probes`, so producer and consumer agree on the digest.
    pub sealed_probes: Vec<(String, String)>,
}

/// Hash a sorted list of `(key, value)` pairs into `h`.
fn hash_pairs(h: &mut Sha256, v: &[(String, String)]) {
    let mut s: Vec<&(String, String)> = v.iter().collect();
    s.sort();
    for (k, val) in s {
        h.update(k.as_bytes());
        h.update(b"=");
        h.update(val.as_bytes());
        h.update(b"\0");
    }
}

/// Compute a content-addressed fingerprint for a test unit per CS-0061 §3.3.
///
/// Inputs (hashed in this stable order):
///   1. `cmd` — the substituted command text
///   2. `timeout` — big-endian u64 bytes
///   3. `should_fail` — 0x00 (false) or 0x01 (true)
///   4. `cook_outputs` — sorted by `(path, fingerprint)`
///   5. `dep_outputs`  — sorted by `(path, fingerprint)`
///   6. `env_keys`     — sorted by `(key, value)`
///
/// **Excluded:** `suite_name`, `test_name` — these are display metadata.
/// Renaming a test via `as STRING` MUST NOT bust its fingerprint (§3.3).
///
/// # Panics
/// Panics if `payload` is not `WorkPayload::Test { .. }`. This function is
/// intentionally test-only; callers must route non-Test payloads elsewhere.
pub fn compute_test_fingerprint(
    payload: &cook_contracts::WorkPayload,
    inputs: &FingerprintInputs,
) -> String {
    let (cmd, timeout, should_fail, lua_code) = match payload {
        cook_contracts::WorkPayload::Test {
            cmd,
            timeout,
            should_fail,
            lua_code,
            ..
        } => (cmd.as_str(), *timeout, *should_fail, lua_code.as_deref()),
        _ => panic!("compute_test_fingerprint: not a Test payload"),
    };

    let mut h = Sha256::new();

    // 1. cmd
    h.update(cmd.as_bytes());
    h.update(b"\0");

    // 1b. lua_code (CS-0127 §22.4): a lua-body test has an empty `cmd` by
    // construction, so its content is carried entirely by `lua_code`. Fold
    // it into the hash so two lua tests with different bodies get distinct
    // fingerprints, and editing a lua test's body busts its cache key
    // instead of colliding on the shared empty-`cmd` hash.
    h.update(lua_code.unwrap_or("").as_bytes());
    h.update(b"\0");

    // 2. timeout (big-endian u64)
    h.update(timeout.to_be_bytes());
    h.update(b"\0");

    // 3. should_fail (0 or 1)
    h.update([if should_fail { 1u8 } else { 0u8 }]);
    h.update(b"\0");

    // 4-6. sorted pair lists
    hash_pairs(&mut h, &inputs.cook_outputs);
    hash_pairs(&mut h, &inputs.dep_outputs);
    hash_pairs(&mut h, &inputs.env_keys);

    // 7. sealed probe values (CS-0159, §17.4 rule 1).
    //
    //    The domain tag is load-bearing, not decoration. `hash_pairs` uses one
    //    encoding for every pair list and the lists are hashed back-to-back,
    //    so without a separator a sealed probe named `K` with value `V` would
    //    hash byte-identically to an env-var contribution `K=V` — two
    //    materially different determinants colliding on one key, i.e. a false
    //    cache hit. NUL cannot occur in an env key or a probe key, so the tag
    //    is unambiguous.
    //
    //    The tag is emitted ONLY for a non-empty set, which keeps the surface
    //    purely additive: a test that seals nothing hashes exactly as it did
    //    pre-CS-0159, so no `CACHE_VERSION` bump is needed and existing
    //    test-cache entries stay valid. A newly-sealed test has no prior entry
    //    to collide with.
    if !inputs.sealed_probes.is_empty() {
        h.update(b"\0seal\0");
        hash_pairs(&mut h, &inputs.sealed_probes);
    }

    format!("sha256:{:x}", h.finalize())
}

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
