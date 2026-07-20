use std::collections::BTreeMap;
use std::path::Path;

use crate::{
    hash_str,
    record::{FileRecord, StepEntry, CACHE_VERSION},
};

/// Hash of an empty file. Empty files are treated as signals (marker files)
/// where mtime changes always trigger rebuilds, even if content is unchanged.
fn empty_hash() -> u64 {
    xxhash_rust::xxh3::xxh3_64(b"")
}

/// Get mtime as epoch milliseconds. Returns None if file doesn't exist.
/// Uses millisecond resolution to catch rapid modifications.
pub fn stat_mtime(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    // TOML integers are i64; clamp absurd future mtimes so save() never
    // fails on a file with an astronomically large mtime (COOK-92).
    Some(
        (mtime
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64)
            .min(i64::MAX as u64),
    )
}

/// Hash file contents with xxh3_64. Returns None if file can't be read.
pub fn hash_file(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    Some(xxhash_rust::xxh3::xxh3_64(&bytes))
}

/// Hash a sorted env var map into a single u64.
pub fn hash_env(env: &BTreeMap<String, String>) -> u64 {
    let combined: String = env
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");
    hash_str(&combined)
}

#[derive(Debug, PartialEq)]
pub enum RebuildResult {
    Skip,
    Rebuild(RebuildReason),
}

#[derive(Debug, PartialEq)]
pub enum RebuildReason {
    NoCacheEntry,
    CommandHashChanged,
    EnvChanged,
    SealChanged,
    OutputMissing,
    OutputChanged,
    /// COOK-276: the full recorded-vs-current input diff, computed at miss
    /// time (both sides are already in hand — no extra hashing on hits).
    /// All three lists empty means the sets are equal but ordered
    /// differently (a reorder still misses the key).
    InputsChanged {
        /// Paths present in both sets whose content differs (or that can no
        /// longer be read).
        changed: Vec<String>,
        /// Paths in the current set but not the recorded one.
        added: Vec<String>,
        /// Paths in the recorded set but not the current one.
        removed: Vec<String>,
    },
}

impl RebuildReason {
    /// One-line human cause fragment for build-output attribution (COOK-276):
    /// `input changed: <path> (+N more)`, `env changed`, … Returns `None` for
    /// `NoCacheEntry` — a cold unit is not a *re*-run and gets no attribution.
    pub fn cause_summary(&self) -> Option<String> {
        fn capped(kind: &str, paths: &[String], extra: usize) -> String {
            let more = paths.len() + extra - 1;
            if more > 0 {
                format!("input {kind}: {} (+{more} more)", paths[0])
            } else {
                format!("input {kind}: {}", paths[0])
            }
        }
        match self {
            RebuildReason::NoCacheEntry => None,
            RebuildReason::CommandHashChanged => Some("command changed".into()),
            RebuildReason::EnvChanged => Some("env changed".into()),
            RebuildReason::SealChanged => Some("seal changed".into()),
            RebuildReason::OutputMissing => Some("output missing (not restorable)".into()),
            RebuildReason::OutputChanged => Some("output drifted (not restorable)".into()),
            RebuildReason::InputsChanged { changed, added, removed } => {
                let rest = |skip: &[String]| {
                    changed.len() + added.len() + removed.len() - skip.len()
                };
                if !changed.is_empty() {
                    Some(capped("changed", changed, rest(changed)))
                } else if !added.is_empty() {
                    Some(capped("added", added, rest(added)))
                } else if !removed.is_empty() {
                    Some(capped("removed", removed, rest(removed)))
                } else {
                    Some("input set reordered".into())
                }
            }
        }
    }
}

/// Shared input-checking logic for cook and plate layers.
///
/// On a mismatch, returns the FULL diff (every changed path, every
/// added/removed path) rather than short-circuiting at the first — the walk
/// only hashes files whose mtime moved, and a miss is followed by a rebuild
/// that dwarfs the cost, so completeness here is effectively free (COOK-276).
fn check_inputs(
    cached_inputs: &[FileRecord],
    current_input_paths: &[&str],
    working_dir: &Path,
) -> Result<Vec<FileRecord>, RebuildReason> {
    let cached_paths: Vec<&str> = cached_inputs.iter().map(|f| f.path.as_str()).collect();
    if cached_paths != current_input_paths {
        let cached_set: std::collections::BTreeSet<&str> = cached_paths.iter().copied().collect();
        let current_set: std::collections::BTreeSet<&str> =
            current_input_paths.iter().copied().collect();
        let added = current_set.difference(&cached_set).map(|s| s.to_string()).collect();
        let removed = cached_set.difference(&current_set).map(|s| s.to_string()).collect();
        return Err(RebuildReason::InputsChanged { changed: Vec::new(), added, removed });
    }

    let mut updated = cached_inputs.to_vec();
    let mut changed: Vec<String> = Vec::new();
    for (i, (cached, rel_path)) in cached_inputs
        .iter()
        .zip(current_input_paths.iter())
        .enumerate()
    {
        let abs_path = working_dir.join(rel_path);
        let disk_mtime = match stat_mtime(&abs_path) {
            Some(m) => m,
            None => {
                changed.push(cached.path.clone());
                continue;
            }
        };
        if disk_mtime != cached.mtime {
            let disk_hash = hash_file(&abs_path);
            // Unreadable, content differs, or an empty marker file (mtime is
            // authoritative for those) → changed.
            if disk_hash != Some(cached.hash) || disk_hash == Some(empty_hash()) {
                changed.push(cached.path.clone());
                continue;
            }
            updated[i].mtime = disk_mtime;
        }
    }
    if !changed.is_empty() {
        return Err(RebuildReason::InputsChanged {
            changed,
            added: Vec::new(),
            removed: Vec::new(),
        });
    }
    Ok(updated)
}

mod __depfile_call {
    use std::path::Path;

    /// Function pointer the engine installs at startup. `cook-fingerprint`
    /// does not depend on `cook-cache`; the engine wires the real parser
    /// before any check fires (see cook-engine::executor).
    static PARSER: std::sync::OnceLock<
        fn(&Path, &str, &Path, &str) -> Result<Vec<String>, ()>,
    > = std::sync::OnceLock::new();

    pub fn install(parser: fn(&Path, &str, &Path, &str) -> Result<Vec<String>, ()>) {
        let _ = PARSER.set(parser);
    }

    pub fn parse(
        depfile_path: &Path,
        source_path: &str,
        working_dir: &Path,
        format: &str,
    ) -> Result<Vec<String>, ()> {
        match PARSER.get() {
            Some(p) => p(depfile_path, source_path, working_dir, format),
            None => Err(()),
        }
    }
}

pub use __depfile_call::install as install_depfile_parser;

/// Context for restore-on-hit attempts (2026-05-02 addendum spec §5.2).
///
/// When `Some(&RestoreCtx)` is passed to `needs_rebuild_cook`, a cache entry
/// whose command/context/env hashes match but whose on-disk output content
/// has drifted (or whose outputs are missing) will first attempt to fetch
/// each output's bytes from the backend and write them to disk. Only if the
/// restore fails does the function fall back to the rebuild path.
///
/// `recipe_namespace` is the same string formed at upload time:
/// `format!("{project_id}/{cookfile_path}::{recipe_name}")`. Both sides MUST
/// agree on this format or the recomposed `cloud_key` will differ.
pub struct RestoreCtx<'a> {
    pub backend: &'a dyn crate::backend::CacheBackend,
    pub recipe_namespace: &'a str,
}

/// Check if a cook layer (with output) needs to rebuild.
/// INVARIANT: cook.layer() calls must NOT be nested.
///
/// When `restore_ctx` is `Some`, an entry whose command/context/env hashes
/// match but whose on-disk outputs have drifted (or are missing) will attempt
/// to restore each output's bytes from the backend before falling back to
/// `OutputChanged`/`OutputMissing` rebuild. See spec §5.2.
///
/// When `record` is `true` (the `record` disposition — an intrinsically
/// non-reproducible artifact such as LLM/image generation), byte-equivalence is
/// waived (§17.1.3): a present-but-content-drifted output is treated as
/// authoritative and is NOT scheduled for restore/rebuild. A genuinely missing
/// output still falls through to restore/rebuild as normal — `record` cannot
/// conjure bytes without a backend.
pub fn needs_rebuild_cook(
    entry: Option<&StepEntry>,
    current_inputs: &[&str],
    current_outputs: &[&str],
    command_hash: u64,
    env_contribution: u64,
    seal_contribution: u64,
    working_dir: &Path,
    restore_ctx: Option<&RestoreCtx>,
    discovered_inputs: Option<&cook_contracts::DiscoveredInputs>,
    record: bool,
) -> (RebuildResult, Option<StepEntry>) {
    let entry = match entry {
        None => return (RebuildResult::Rebuild(RebuildReason::NoCacheEntry), None),
        Some(e) => e,
    };
    if entry.command_hash != command_hash {
        return (RebuildResult::Rebuild(RebuildReason::CommandHashChanged), None);
    }
    if entry.env_contribution != env_contribution {
        return (RebuildResult::Rebuild(RebuildReason::EnvChanged), None);
    }
    if entry.seal_contribution != seal_contribution {
        return (RebuildResult::Rebuild(RebuildReason::SealChanged), None);
    }

    // Pre-check augmentation: when the unit declares discovered_inputs and
    // a prior depfile is on disk, fatten current_inputs by the discovered
    // paths so the entry's input set matches.
    //
    // §10 refinement: when the depfile is missing or malformed but a
    // restore_ctx is available, fall back to the stored entry's input list
    // rather than forcing a rebuild. The depfile itself is an implicit output
    // (appended by record_completion) and will be restored by try_restore if
    // the outputs check finds it missing. Without this fallback, a partial
    // disk wipe that removes only the depfile causes an InputSetChanged
    // rebuild even though the backend can restore both the .d and .o.
    //
    // Without restore_ctx: a missing or malformed depfile is no-augmentation
    // (fallthrough to InputSetChanged → rebuild → self-heal).
    let augmented_storage: Vec<String>;
    let augmented_refs: Vec<&str>;
    let entry_inputs_refs: Vec<&str>;
    let current_inputs_for_check: &[&str] = if let Some(di) = discovered_inputs {
        let source_for_skip = current_inputs.first().copied().unwrap_or("");
        match __depfile_call::parse(
            &working_dir.join(&di.from),
            source_for_skip,
            working_dir,
            &di.format,
        ) {
            Ok(discovered_paths) => {
                augmented_storage = current_inputs
                    .iter()
                    .map(|s| (*s).to_string())
                    .chain(discovered_paths)
                    .collect();
                augmented_refs = augmented_storage.iter().map(String::as_str).collect();
                &augmented_refs
            }
            Err(_) => {
                // Depfile missing or malformed. If we have a restore_ctx,
                // use the stored entry's fat input list so the check can
                // proceed to the outputs walk where try_restore will fetch
                // the depfile back. Without a restore_ctx there's nothing
                // to recover from, so fall back to rebuild (self-heal).
                if restore_ctx.is_some() {
                    entry_inputs_refs = entry.inputs.iter().map(|f| f.path.as_str()).collect();
                    &entry_inputs_refs
                } else {
                    current_inputs
                }
            }
        }
    } else {
        current_inputs
    };

    // Inputs first (spec §5.3): we need the input content hashes to recompose
    // cloud_key for the restore attempt below. InputChanged/InputSetChanged
    // still short-circuits to rebuild before any restore work happens.
    let updated_inputs = match check_inputs(&entry.inputs, current_inputs_for_check, working_dir) {
        Err(reason) => return (RebuildResult::Rebuild(reason), None),
        Ok(u) => u,
    };

    // Output augmentation: when discovered_inputs is set, record_completion
    // appends the depfile as an implicit output. Augment current_outputs to
    // include the depfile path so the output count and content checks below
    // work correctly against the stored fat entry.
    let augmented_outputs_storage: Vec<String>;
    let augmented_outputs_refs: Vec<&str>;
    let current_outputs_for_check: &[&str] = if let Some(di) = discovered_inputs {
        if entry.outputs.len() == current_outputs.len() + 1 {
            // Entry has one extra output — assume it's the implicit depfile.
            augmented_outputs_storage = current_outputs
                .iter()
                .map(|s| (*s).to_string())
                .chain(std::iter::once(di.from.clone()))
                .collect();
            augmented_outputs_refs = augmented_outputs_storage.iter().map(String::as_str).collect();
            &augmented_outputs_refs
        } else {
            current_outputs
        }
    } else {
        current_outputs
    };

    // Output count must match.
    if entry.outputs.len() != current_outputs_for_check.len() {
        return (RebuildResult::Rebuild(RebuildReason::OutputMissing), None);
    }

    // Walk outputs; collect indices that need restore.
    let mut needs_restore: Vec<usize> = Vec::new();
    let mut output_missing_seen = false;
    for (i, (cached_out, rel_path)) in entry
        .outputs
        .iter()
        .zip(current_outputs_for_check.iter())
        .enumerate()
    {
        let abs = working_dir.join(rel_path);
        if !abs.exists() {
            needs_restore.push(i);
            output_missing_seen = true;
            continue;
        }
        // §17.1.3 record disposition: a present-but-drifted output is
        // authoritative for a record unit — byte-equivalence is waived, so the
        // drift check is suppressed. (The missing-output push above stays
        // unguarded: record cannot conjure bytes without a backend.)
        if !record {
            if let Some(disk_mtime) = stat_mtime(&abs) {
                if disk_mtime != cached_out.mtime {
                    if let Some(disk_hash) = hash_file(&abs) {
                        if disk_hash != cached_out.hash {
                            needs_restore.push(i);
                        }
                    }
                }
            }
        }
    }

    if !needs_restore.is_empty() {
        let restored = match restore_ctx {
            Some(ctx) => try_restore(
                ctx,
                entry,
                current_outputs_for_check,
                &needs_restore,
                &updated_inputs,
                working_dir,
            ),
            None => false,
        };
        if !restored {
            let reason = if output_missing_seen {
                RebuildReason::OutputMissing
            } else {
                RebuildReason::OutputChanged
            };
            return (RebuildResult::Rebuild(reason), None);
        }
    }

    let updated = StepEntry {
        inputs: updated_inputs,
        outputs: entry.outputs.clone(),
        command_hash: entry.command_hash,
        env_contribution: entry.env_contribution,
        seal_contribution: entry.seal_contribution,
    };
    (RebuildResult::Skip, Some(updated))
}

/// Apply a Unix file `mode` to `abs`. On non-Unix this is a no-op that
/// reports success (Windows mode parity — the rest of the codebase treats
/// mode as advisory there). Returns false only on a real set-permissions
/// failure.
#[cfg(unix)]
fn set_mode(abs: &Path, mode: u32) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(abs, std::fs::Permissions::from_mode(mode)).is_ok()
}

#[cfg(not(unix))]
fn set_mode(abs: &Path, mode: u32) -> bool {
    let _ = (abs, mode);
    true
}

/// Fold `.` and `..` components lexically WITHOUT touching the filesystem.
/// `..` pops the last normal component (or is dropped at the root).
fn normalize_lexical(p: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => { out.pop(); }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Walk up `p`'s ancestors and return the longest one that exists on disk.
fn longest_existing_prefix(p: &Path) -> Option<std::path::PathBuf> {
    let mut cur = Some(p);
    while let Some(c) = cur {
        if c.exists() { return Some(c.to_path_buf()); }
        cur = c.parent();
    }
    None
}

/// Create a symlink at `link` pointing to `target`, only if `target` cannot
/// escape `anchor`. Ports Turborepo's four restore_symlink checks:
///   (1) reject absolute targets;
///   (2) lexically resolve `target` against the link's parent and require the
///       result stays within `anchor`;
///   (3) realpath the longest existing prefix of the resolved target and
///       require it stays within the real anchor (defends symlink-then-write
///       escapes);
///   (4) only then create the link.
/// Returns false on any rejection or OS error.
#[cfg(unix)]
fn restore_symlink_checked(anchor: &Path, link: &Path, target: &str) -> bool {
    let t = Path::new(target);
    // (1) Reject absolute targets.
    if t.is_absolute() {
        tracing::warn!(
            "cache restore: symlink target {:?} escapes output anchor (absolute path); treating as miss",
            target
        );
        return false;
    }
    let parent = match link.parent() {
        Some(p) => p,
        None => return false,
    };
    // (2) Lexically resolve target against link's parent directory and verify
    // the result stays within the anchor.
    let lexical = normalize_lexical(&parent.join(t));
    let real_anchor = anchor.canonicalize().unwrap_or_else(|_| normalize_lexical(anchor));
    let lexical_anchor = normalize_lexical(anchor);
    if !(lexical.starts_with(&real_anchor) || lexical.starts_with(&lexical_anchor)) {
        tracing::warn!(
            "cache restore: symlink target {:?} escapes output anchor (lexical escape); treating as miss",
            target
        );
        return false;
    }
    // (3) Realpath the longest existing prefix of the resolved target.
    if let Some(existing) = longest_existing_prefix(&lexical) {
        match existing.canonicalize() {
            Ok(real) if real.starts_with(&real_anchor) => {}
            _ => {
                tracing::warn!(
                    "cache restore: symlink target {:?} escapes output anchor (realpath escape); treating as miss",
                    target
                );
                return false;
            }
        }
    }
    // (4) Create the link.
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(not(unix))]
fn restore_symlink_checked(anchor: &Path, link: &Path, target: &str) -> bool {
    let _ = (anchor, link, target);
    false
}

/// Materialise one cached artifact at `abs`, faithful to its recorded kind +
/// mode. `anchor` is the restore boundary for symlink hardening (enforced by
/// `restore_symlink_checked`). `expected_content_hash`, when `Some`, pins the restored *file*
/// bytes to the locally-trusted `StepEntry` content hash (xxh3) BEFORE they
/// touch the workspace — this is the warm-path defence against a shared backend
/// that rewrites BOTH the artifact bytes and its sidecar (the both-rewritten
/// case the sidecar `VerifyingReader` deliberately leaves out of scope, see
/// `ArtifactMeta::content_hash` / CS-0054 §2; cf. spec §8.6 cache integrity).
/// The cold `fetch_by_key` path has no local record to pin against and passes
/// `None`, relying solely on `get_with_meta`'s `VerifyingReader` — unchanged
/// from prior behaviour. Returns false on any miss/error (caller falls back to
/// rebuild).
fn restore_one(
    backend: &dyn crate::backend::CacheBackend,
    artifact_k: &crate::backend::CloudKey,
    abs: &Path,
    anchor: &Path,
    expected_content_hash: Option<u64>,
) -> bool {
    // `get_with_meta` returns a `VerifyingReader`: draining it surfaces any
    // bytes-vs-sidecar tampering as an `io::Error` at EOF (treated as a miss).
    let (mut reader, meta) = match backend.get_with_meta(artifact_k) {
        Ok(Some(t)) => t,
        _ => return false,
    };
    if let Some(parent) = abs.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    match meta.kind.as_deref() {
        Some("dir") => {
            if std::fs::create_dir_all(abs).is_err() {
                return false;
            }
            set_mode(abs, meta.mode)
        }
        Some("symlink") => {
            let target = match meta.target.as_deref() {
                Some(t) => t,
                None => return false,
            };
            restore_symlink_checked(anchor, abs, target)
        }
        _ => {
            let mut bytes = Vec::new();
            if std::io::Read::read_to_end(&mut reader, &mut bytes).is_err() {
                return false;
            }
            // Warm-path integrity anchor (see fn docs): verify against the
            // locally-trusted record BEFORE the atomic write so tampered
            // bytes never reach the workspace, even transiently.
            if let Some(expected) = expected_content_hash {
                if xxhash_rust::xxh3::xxh3_64(&bytes) != expected {
                    return false;
                }
            }
            // Atomic write via tmp + rename.
            let tmp = abs.with_extension("cook.tmp");
            if std::fs::write(&tmp, &bytes).is_err() {
                return false;
            }
            if std::fs::rename(&tmp, abs).is_err() {
                return false;
            }
            set_mode(abs, meta.mode)
        }
    }
}

/// Attempt to restore output bytes from the backend. Returns true if every
/// index in `needs_restore` was fetched and written to disk; any miss aborts
/// the attempt and the caller falls back to rebuild.
fn try_restore(
    ctx: &RestoreCtx,
    entry: &StepEntry,
    current_outputs: &[&str],
    needs_restore: &[usize],
    updated_inputs: &[FileRecord],
    working_dir: &Path,
) -> bool {
    let mut sorted: Vec<u64> = updated_inputs.iter().map(|r| r.hash).collect();
    sorted.sort();
    let key_inputs = crate::backend::CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: ctx.recipe_namespace,
        command_hash: entry.command_hash,
        env_contribution: entry.env_contribution,
        seal_contribution: entry.seal_contribution,
        sorted_input_content_hashes: &sorted,
    };
    let cloud_k = crate::backend::cloud_key(&key_inputs);

    // Two-pass restore (symlink-last). Pass 1 attempts every needed index;
    // misses are retried in pass 2 so a symlink whose target was materialised
    // earlier in pass 1 now resolves. Integrity for each restored file is
    // pinned to the locally-trusted `StepEntry` hash via `restore_one`'s
    // `expected_content_hash` (spec §8.6) — the warm-path defence the cold
    // `fetch_by_key` path cannot provide.
    let mut pending: Vec<usize> = Vec::new();
    for &idx in needs_restore {
        let path = current_outputs[idx];
        let artifact_k = crate::backend::artifact_key(&cloud_k, idx as u32, path);
        let abs = working_dir.join(path);
        if !restore_one(
            ctx.backend,
            &artifact_k,
            &abs,
            working_dir,
            Some(entry.outputs[idx].hash),
        ) {
            pending.push(idx);
        }
    }
    for idx in pending {
        let path = current_outputs[idx];
        let artifact_k = crate::backend::artifact_key(&cloud_k, idx as u32, path);
        let abs = working_dir.join(path);
        if !restore_one(
            ctx.backend,
            &artifact_k,
            &abs,
            working_dir,
            Some(entry.outputs[idx].hash),
        ) {
            return false;
        }
    }
    true
}

/// COOK-278: the result of a successful [`fetch_by_key`] restore. The caller
/// needs both lists to leave the working tree and the local index in the same
/// state a fresh execution would have: `restored_outputs` seeds the recorded
/// `StepEntry.outputs` and the stray-output sweep; `discovered_paths` fattens
/// the recorded input set so the NEXT check is a plain local skip instead of
/// a perpetual `InputSetChanged` → refetch round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchOutcome {
    /// Index-ordered restored paths: resolved file outputs, then the implicit
    /// depfile (discovered-inputs units only), then recorded empty dirs.
    pub restored_outputs: Vec<String>,
    /// The discovered-input path set whose content hashes composed the winning
    /// full key. Empty for units without `discovered_inputs`.
    pub discovered_paths: Vec<String>,
}

/// Read the candidate discovered-input path sets recorded under a unit's
/// DECLARED-inputs-only key, newest first. Prefers the COOK-278 multi-set
/// manifest; falls back to the single-set COOK-177 manifest written by older
/// binaries. Returns `[]` when neither exists (or either is corrupt — a
/// corrupt manifest degrades to a safe cold miss, never a wrong hit).
pub fn read_discovered_input_sets(
    backend: &dyn crate::backend::CacheBackend,
    declared_key: &crate::backend::CloudKey,
) -> Vec<Vec<String>> {
    let read_json = |index: u32, path: &str| -> Option<Vec<u8>> {
        let k = crate::backend::artifact_key(declared_key, index, path);
        let mut reader = backend.get(&k).ok().flatten()?;
        let mut buf = Vec::new();
        // Drain fully so the VerifyingReader hashes on read (CS-0054).
        std::io::Read::read_to_end(&mut reader, &mut buf).ok()?;
        Some(buf)
    };
    if let Some(sets) = read_json(
        crate::backend::DISCOVERED_INPUT_SETS_INDEX,
        crate::backend::DISCOVERED_INPUT_SETS_PATH,
    )
    .and_then(|buf| serde_json::from_slice::<Vec<Vec<String>>>(&buf).ok())
    {
        return sets;
    }
    if let Some(set) = read_json(
        crate::backend::DISCOVERED_INPUTS_MANIFEST_INDEX,
        crate::backend::DISCOVERED_INPUTS_MANIFEST_PATH,
    )
    .and_then(|buf| serde_json::from_slice::<Vec<String>>(&buf).ok())
    {
        return vec![set];
    }
    Vec::new()
}

/// Fetch-by-key (COOK-162 §3 sharing): serve a unit's outputs straight from
/// the shared backend by recomputing its one key from on-disk input content.
/// Returns `Some(FetchOutcome)` iff every output of some candidate key was
/// fetched, verified, and written; `None` is a safe miss.
/// `sorted_input_content_hashes` MUST already be sorted and MUST cover the
/// DECLARED inputs only.
///
/// An empty `output_paths` slice returns `None` (no artifacts to serve is not
/// a hit).
///
/// **`discovered_inputs` (depfile) units** are handled via a two-level fetch
/// (COOK-177, extended by COOK-278). The publish path folds the
/// depfile-discovered inputs into the artifact key, but the consumer's
/// depfile (if any) describes the LAST build, not the one being looked up. So
/// the candidate discovered-path sets recorded under the DECLARED-only key
/// are tried newest-first: re-hash each set's paths from the consumer's own
/// `working_dir` at their CURRENT content, fold them into the hash set, and
/// probe the resulting FULL key. A set whose files hash differently composes
/// a different key and naturally misses — that is the manifest validation the
/// ccache pattern requires (a different input content can imply a different
/// input set, so a candidate hit validates against the store, never assumes
/// set stability). Any recovery failure (no manifest, a listed input absent
/// locally, a corrupt manifest) skips to the next candidate or returns a safe
/// miss.
///
/// **Concrete output recovery (COOK-278):** the caller's `output_paths` may
/// be stale for glob-output units — on a warm revert they are the LAST run's
/// concrete names, and content-dependent filenames (Next.js chunk hashes)
/// make those wrong for the candidate key being probed. When the candidate
/// key's determinant manifest is present, its recorded `output_paths` (plus
/// the implicit depfile and any recorded empty dirs) are restored instead;
/// the caller list is only a fallback for stores written before the
/// determinant manifest existed. This also lifts the old "glob outputs cannot
/// cold-fetch" limitation: raw patterns in `output_paths` simply never match
/// an artifact, but the manifest-driven list does.
#[allow(clippy::too_many_arguments)]
pub fn fetch_by_key(
    ctx: &RestoreCtx,
    command_hash: u64,
    env_contribution: u64,
    seal_contribution: u64,
    sorted_input_content_hashes: &[u64],
    output_paths: &[&str],
    working_dir: &std::path::Path,
    discovered_inputs: Option<&cook_contracts::DiscoveredInputs>,
) -> Option<FetchOutcome> {
    if output_paths.is_empty() {
        return None;
    }
    // Candidate discovered-input sets. A unit without discovered_inputs has
    // exactly one candidate: the empty set (full key == declared key).
    let candidates: Vec<Vec<String>> = match discovered_inputs {
        Some(_) => {
            let declared_key = crate::backend::cloud_key(&crate::backend::CloudKeyInputs {
                schema_version: CACHE_VERSION,
                recipe_namespace: ctx.recipe_namespace,
                command_hash,
                env_contribution,
                seal_contribution,
                sorted_input_content_hashes, // declared-only, exactly as passed
            });
            let sets = read_discovered_input_sets(ctx.backend, &declared_key);
            if sets.is_empty() {
                return None; // no manifest => cannot reconstruct => safe cold miss
            }
            sets
        }
        None => vec![Vec::new()],
    };
    for set in candidates {
        let mut full_hashes: Vec<u64> = sorted_input_content_hashes.to_vec();
        if !set.is_empty() {
            let refs: Vec<&str> = set.iter().map(|s| s.as_str()).collect();
            match hash_input_paths(&refs, working_dir) {
                Some(mut h) => full_hashes.append(&mut h),
                None => continue, // a listed discovered input is absent locally => try next set
            }
            full_hashes.sort();
        }
        let cloud_k = crate::backend::cloud_key(&crate::backend::CloudKeyInputs {
            schema_version: CACHE_VERSION,
            recipe_namespace: ctx.recipe_namespace,
            command_hash,
            env_contribution,
            seal_contribution,
            sorted_input_content_hashes: &full_hashes,
        });
        // COOK-278: prefer the candidate key's own recorded output list over
        // the caller's (possibly stale) one. Index order mirrors publish:
        // files, implicit depfile, empty dirs.
        let restore_list: Vec<String> = match ctx.backend.get_manifest(&cloud_k) {
            Ok(Some(m)) => {
                let mut list = m.output_paths;
                if let Some(di) = discovered_inputs {
                    list.push(di.from.clone());
                }
                list.extend(m.empty_dir_outputs);
                list
            }
            _ => output_paths.iter().map(|s| (*s).to_string()).collect(),
        };
        if restore_all(ctx, &cloud_k, &restore_list, working_dir) {
            return Some(FetchOutcome {
                restored_outputs: restore_list,
                discovered_paths: set,
            });
        }
    }
    None
}

/// Two-pass restore (symlink-last), mirroring `try_restore`. The fetch path
/// has no local `StepEntry` to pin against, so `expected_content_hash` is
/// `None`: integrity rests solely on `get_with_meta`'s `VerifyingReader`
/// (CS-0054 verify-on-restore).
fn restore_all(
    ctx: &RestoreCtx,
    cloud_k: &crate::backend::CloudKey,
    paths: &[String],
    working_dir: &std::path::Path,
) -> bool {
    let mut pending: Vec<usize> = Vec::new();
    for (idx, path) in paths.iter().enumerate() {
        let artifact_k = crate::backend::artifact_key(cloud_k, idx as u32, path);
        let abs = working_dir.join(path);
        if !restore_one(ctx.backend, &artifact_k, &abs, working_dir, None) {
            pending.push(idx);
        }
    }
    for idx in pending {
        let path = &paths[idx];
        let artifact_k = crate::backend::artifact_key(cloud_k, idx as u32, path);
        let abs = working_dir.join(path);
        if !restore_one(ctx.backend, &artifact_k, &abs, working_dir, None) {
            return false;
        }
    }
    true
}

/// Hash each declared input path's on-disk content, sorted ascending. Returns
/// None if any declared input is missing (the unit cannot be a clean hit).
pub fn hash_input_paths(input_paths: &[&str], working_dir: &std::path::Path) -> Option<Vec<u64>> {
    let mut hashes = Vec::with_capacity(input_paths.len());
    for p in input_paths {
        let abs = working_dir.join(p);
        match hash_file(&abs) {
            Some(h) => hashes.push(h),
            None => return None,
        }
    }
    hashes.sort();
    Some(hashes)
}

/// Check if a plate layer (no output) needs to re-run.
pub fn needs_rebuild_plate(
    entry: Option<&StepEntry>,
    current_inputs: &[&str],
    command_hash: u64,
    env_contribution: u64,
    seal_contribution: u64,
    working_dir: &Path,
) -> (RebuildResult, Option<StepEntry>) {
    let entry = match entry {
        None => return (RebuildResult::Rebuild(RebuildReason::NoCacheEntry), None),
        Some(e) => e,
    };
    if entry.command_hash != command_hash {
        return (RebuildResult::Rebuild(RebuildReason::CommandHashChanged), None);
    }
    if entry.env_contribution != env_contribution {
        return (RebuildResult::Rebuild(RebuildReason::EnvChanged), None);
    }
    if entry.seal_contribution != seal_contribution {
        return (RebuildResult::Rebuild(RebuildReason::SealChanged), None);
    }
    match check_inputs(&entry.inputs, current_inputs, working_dir) {
        Err(reason) => (RebuildResult::Rebuild(reason), None),
        Ok(updated_inputs) => {
            let updated = StepEntry {
                inputs: updated_inputs,
                outputs: vec![],
                command_hash: entry.command_hash,
                env_contribution: entry.env_contribution,
                seal_contribution: entry.seal_contribution,
            };
            (RebuildResult::Skip, Some(updated))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::FileRecord;

    // -------------------------------------------------------------------------
    // Task 4: hashing / mtime utilities
    // -------------------------------------------------------------------------

    #[test]
    fn test_hash_file_deterministic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"hello world").expect("write");

        let h1 = hash_file(&path).expect("hash");
        let h2 = hash_file(&path).expect("hash");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_file_differs_on_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p1 = dir.path().join("a.txt");
        let p2 = dir.path().join("b.txt");
        std::fs::write(&p1, b"hello").expect("write");
        std::fs::write(&p2, b"world").expect("write");

        let h1 = hash_file(&p1).expect("hash");
        let h2 = hash_file(&p2).expect("hash");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_file_missing_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent.txt");
        assert!(hash_file(&path).is_none());
    }

    #[test]
    fn test_stat_mtime_returns_positive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"data").expect("write");

        let mtime = stat_mtime(&path).expect("mtime");
        assert!(mtime > 0);
    }

    #[test]
    fn test_stat_mtime_missing_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent.txt");
        assert!(stat_mtime(&path).is_none());
    }

    #[test]
    fn test_hash_str_deterministic() {
        let h1 = hash_str("gcc -O2 -c $in -o $out");
        let h2 = hash_str("gcc -O2 -c $in -o $out");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_str_differs() {
        let h1 = hash_str("gcc -O2 -c $in -o $out");
        let h2 = hash_str("clang -O2 -c $in -o $out");
        assert_ne!(h1, h2);
    }

    // -------------------------------------------------------------------------
    // COOK-276: warm re-run attribution — full diff + cause summaries
    // -------------------------------------------------------------------------

    #[test]
    fn check_inputs_collects_every_changed_path_not_just_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        for f in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(wd.join(f), format!("old {f}")).expect("write");
        }
        let cached: Vec<FileRecord> = ["a.txt", "b.txt", "c.txt"]
            .iter()
            .map(|f| FileRecord {
                path: (*f).to_string(),
                mtime: 0, // force the hash comparison
                hash: hash_file(&wd.join(f)).expect("hash"),
            })
            .collect();
        // Change two of the three.
        std::fs::write(wd.join("a.txt"), b"new a").expect("write");
        std::fs::write(wd.join("c.txt"), b"new c").expect("write");

        let err = check_inputs(&cached, &["a.txt", "b.txt", "c.txt"], wd).unwrap_err();
        assert_eq!(
            err,
            RebuildReason::InputsChanged {
                changed: vec!["a.txt".into(), "c.txt".into()],
                added: vec![],
                removed: vec![],
            }
        );
    }

    #[test]
    fn check_inputs_names_added_and_removed_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("keep.txt"), b"x").expect("write");
        let cached = vec![
            make_file_record("keep.txt", wd),
            FileRecord { path: "gone.txt".into(), mtime: 0, hash: 1 },
        ];
        let err = check_inputs(&cached, &["keep.txt", "new.txt"], wd).unwrap_err();
        assert_eq!(
            err,
            RebuildReason::InputsChanged {
                changed: vec![],
                added: vec!["new.txt".into()],
                removed: vec!["gone.txt".into()],
            }
        );
    }

    #[test]
    fn cause_summary_formats_and_caps() {
        let r = RebuildReason::InputsChanged {
            changed: vec!["apps/web/manifest.json".into(), "b".into(), "c".into()],
            added: vec![],
            removed: vec![],
        };
        assert_eq!(
            r.cause_summary().unwrap(),
            "input changed: apps/web/manifest.json (+2 more)"
        );

        let single = RebuildReason::InputsChanged {
            changed: vec![],
            added: vec!["new.txt".into()],
            removed: vec![],
        };
        assert_eq!(single.cause_summary().unwrap(), "input added: new.txt");

        let mixed = RebuildReason::InputsChanged {
            changed: vec!["m.json".into()],
            added: vec!["a".into()],
            removed: vec!["r".into()],
        };
        assert_eq!(mixed.cause_summary().unwrap(), "input changed: m.json (+2 more)");

        assert_eq!(RebuildReason::EnvChanged.cause_summary().unwrap(), "env changed");
        assert_eq!(RebuildReason::CommandHashChanged.cause_summary().unwrap(), "command changed");
        assert_eq!(RebuildReason::NoCacheEntry.cause_summary(), None, "cold is not attributed");

        let reorder = RebuildReason::InputsChanged { changed: vec![], added: vec![], removed: vec![] };
        assert_eq!(reorder.cause_summary().unwrap(), "input set reordered");
    }

    // -------------------------------------------------------------------------
    // Task 5: rebuild-check algorithm
    // -------------------------------------------------------------------------

    fn make_file_record(rel_path: &str, working_dir: &Path) -> FileRecord {
        let abs = working_dir.join(rel_path);
        FileRecord {
            path: rel_path.to_string(),
            mtime: stat_mtime(&abs).expect("mtime"),
            hash: hash_file(&abs).expect("hash"),
        }
    }

    #[test]
    fn test_no_cache_entry_rebuilds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (result, updated) =
            needs_rebuild_cook(None, &["in.c"], &["out.o"], 0xdead, 0, 0, dir.path(), None, None, false);
        assert_eq!(result, RebuildResult::Rebuild(RebuildReason::NoCacheEntry));
        assert!(updated.is_none());
    }

    #[test]
    fn test_command_hash_changed_rebuilds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let in_record = make_file_record("in.c", wd);
        let out_record = make_file_record("out.o", wd);

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![out_record],
            command_hash: 0x1111,
            env_contribution: 0,
            seal_contribution: 0,
        };

        let (result, updated) =
            needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0x2222, 0, 0, wd, None, None, false);
        assert_eq!(
            result,
            RebuildResult::Rebuild(RebuildReason::CommandHashChanged)
        );
        assert!(updated.is_none());
    }

    #[test]
    fn test_output_missing_rebuilds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        // out.o is intentionally NOT created

        let in_record = make_file_record("in.c", wd);

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![],
            command_hash: 0xbeef,
            env_contribution: 0,
            seal_contribution: 0,
        };

        let (result, updated) =
            needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0, wd, None, None, false);
        assert_eq!(result, RebuildResult::Rebuild(RebuildReason::OutputMissing));
        assert!(updated.is_none());
    }

    #[test]
    fn test_nothing_changed_skips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let in_record = make_file_record("in.c", wd);
        let out_record = make_file_record("out.o", wd);

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![out_record],
            command_hash: 0xbeef,
            env_contribution: 0,
            seal_contribution: 0,
        };

        let (result, updated) =
            needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0, wd, None, None, false);
        assert_eq!(result, RebuildResult::Skip);
        assert!(updated.is_some());
    }

    #[test]
    fn test_input_content_changed_rebuilds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let out_record = make_file_record("out.o", wd);

        // Build a cache entry whose input mtime is stale (0) and whose hash
        // matches the OLD content.  The disk file already has different content
        // ("void foo(){}"), so when the mtime fast-path fires (0 != real mtime)
        // the hash comparison will also differ, triggering InputChanged.
        let old_hash = xxhash_rust::xxh3::xxh3_64(b"int main(){}");
        let in_record = FileRecord {
            path: "in.c".to_string(),
            mtime: 0, // guaranteed to differ from any real mtime
            hash: old_hash,
        };

        // Overwrite the input with different content.
        std::fs::write(wd.join("in.c"), b"void foo(){}").expect("write");

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![out_record],
            command_hash: 0xbeef,
            env_contribution: 0,
            seal_contribution: 0,
        };

        let (result, updated) =
            needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0, wd, None, None, false);
        assert_eq!(
            result,
            RebuildResult::Rebuild(RebuildReason::InputsChanged {
                changed: vec!["in.c".to_string()],
                added: vec![],
                removed: vec![],
            })
        );
        assert!(updated.is_none());
    }

    // -------------------------------------------------------------------------
    // COOK-163: record disposition waives output-drift rebuild
    // -------------------------------------------------------------------------

    #[test]
    fn record_unit_with_drifted_present_output_skips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let in_record = make_file_record("in.c", wd);

        // Recorded output hash deliberately does NOT match the on-disk content,
        // and the mtime is stale (0) so the drift fast-path fires.
        let out_record = FileRecord {
            path: "out.o".to_string(),
            mtime: 0, // guaranteed to differ from any real mtime
            hash: xxhash_rust::xxh3::xxh3_64(b"different recorded bytes"),
        };

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![out_record],
            command_hash: 0xbeef,
            env_contribution: 0,
            seal_contribution: 0,
        };

        // Control: a non-record unit with a drifted present output and no
        // restore_ctx falls through to OutputChanged rebuild.
        let (control, _) = needs_rebuild_cook(
            Some(&entry),
            &["in.c"],
            &["out.o"],
            0xbeef,
            0,
            0,
            wd,
            None,
            None,
            false,
        );
        assert!(matches!(control, RebuildResult::Rebuild(_)));

        // Waiver: a record unit treats the present-but-drifted output as
        // authoritative — Skip, with an updated entry.
        let (result, updated) = needs_rebuild_cook(
            Some(&entry),
            &["in.c"],
            &["out.o"],
            0xbeef,
            0,
            0,
            wd,
            None,
            None,
            true,
        );
        assert_eq!(result, RebuildResult::Skip);
        assert!(updated.is_some());
    }

    #[test]
    fn record_unit_with_missing_output_still_rebuilds_without_restore() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        // out.o is intentionally NOT created — genuinely missing.

        let in_record = make_file_record("in.c", wd);
        let out_record = FileRecord {
            path: "out.o".to_string(),
            mtime: 0,
            hash: xxhash_rust::xxh3::xxh3_64(b"recorded bytes"),
        };

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![out_record],
            command_hash: 0xbeef,
            env_contribution: 0,
            seal_contribution: 0,
        };

        // record cannot conjure bytes without a backend: a genuinely missing
        // output still restores/rebuilds.
        let (result, updated) = needs_rebuild_cook(
            Some(&entry),
            &["in.c"],
            &["out.o"],
            0xbeef,
            0,
            0,
            wd,
            None,
            None,
            true,
        );
        assert_eq!(result, RebuildResult::Rebuild(RebuildReason::OutputMissing));
        assert!(updated.is_none());
    }

    #[test]
    fn test_plate_no_cache_entry_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (result, updated) = needs_rebuild_plate(None, &["in.c"], 0xdead, 0, 0, dir.path());
        assert_eq!(result, RebuildResult::Rebuild(RebuildReason::NoCacheEntry));
        assert!(updated.is_none());
    }

    #[test]
    fn test_plate_nothing_changed_skips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");

        let in_record = make_file_record("in.c", wd);

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![],
            command_hash: 0xbeef,
            env_contribution: 0,
            seal_contribution: 0,
        };

        let (result, updated) = needs_rebuild_plate(Some(&entry), &["in.c"], 0xbeef, 0, 0, wd);
        assert_eq!(result, RebuildResult::Skip);
        let updated = updated.expect("should have updated entry");
        assert!(updated.outputs.is_empty());
    }

    // -------------------------------------------------------------------------
    // Task 8: hash_env
    // -------------------------------------------------------------------------

    #[test]
    fn test_hash_env_deterministic() {
        let mut env = BTreeMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        env.insert("BAZ".to_string(), "qux".to_string());

        let h1 = hash_env(&env);
        let h2 = hash_env(&env);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_env_order_independent() {
        let mut env1 = BTreeMap::new();
        env1.insert("A".to_string(), "1".to_string());
        env1.insert("B".to_string(), "2".to_string());

        let mut env2 = BTreeMap::new();
        env2.insert("B".to_string(), "2".to_string());
        env2.insert("A".to_string(), "1".to_string());

        assert_eq!(hash_env(&env1), hash_env(&env2));
    }

    #[test]
    fn test_hash_env_differs_on_value_change() {
        let mut env1 = BTreeMap::new();
        env1.insert("KEY".to_string(), "value1".to_string());

        let mut env2 = BTreeMap::new();
        env2.insert("KEY".to_string(), "value2".to_string());

        assert_ne!(hash_env(&env1), hash_env(&env2));
    }

    // -------------------------------------------------------------------------
    // New tests for env rebuild reason
    // -------------------------------------------------------------------------

    #[test]
    fn env_contribution_changed_rebuilds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let in_record = make_file_record("in.c", wd);
        let out_record = make_file_record("out.o", wd);

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![out_record],
            command_hash: 0xbeef,
            env_contribution: 0x1111,
            seal_contribution: 0,
        };

        let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0x9999, 0, wd, None, None, false);
        assert_eq!(result, RebuildResult::Rebuild(RebuildReason::EnvChanged));
        assert!(updated.is_none());
    }

    #[test]
    fn seal_contribution_changed_rebuilds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let in_record = make_file_record("in.c", wd);
        let out_record = make_file_record("out.o", wd);

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![out_record],
            command_hash: 0xbeef,
            env_contribution: 0,
            seal_contribution: 0x1111,
        };

        // Same command/env/inputs/outputs, different seal value -> SealChanged.
        let (result, updated) = needs_rebuild_cook(
            Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0x9999, wd, None, None, false,
        );
        assert_eq!(result, RebuildResult::Rebuild(RebuildReason::SealChanged));
        assert!(updated.is_none());
    }

    #[test]
    fn augments_current_inputs_from_depfile_and_skips() {
        use cook_contracts::DiscoveredInputs;

        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        // Lay out source, header, and a depfile that references both.
        std::fs::write(wd.join("src.c"), b"src").expect("src");
        std::fs::write(wd.join("hdr.h"), b"hdr").expect("hdr");
        std::fs::create_dir_all(wd.join(".cook/deps")).expect("mkdir");
        std::fs::write(
            wd.join(".cook/deps/src.d"),
            b"build/src.o: src.c hdr.h\n",
        ).expect("d");
        std::fs::write(wd.join("out.o"), b"obj").expect("out");

        // Build a stored entry that already has the fat input set.
        let src_hash = hash_file(&wd.join("src.c")).unwrap();
        let hdr_hash = hash_file(&wd.join("hdr.h")).unwrap();
        let out_hash = hash_file(&wd.join("out.o")).unwrap();

        let entry = StepEntry {
            inputs: vec![
                FileRecord { path: "src.c".into(), mtime: 0, hash: src_hash },
                FileRecord { path: "hdr.h".into(), mtime: 0, hash: hdr_hash },
            ],
            outputs: vec![FileRecord {
                path: "out.o".into(),
                mtime: stat_mtime(&wd.join("out.o")).unwrap_or(0),
                hash: out_hash,
            }],
            command_hash: 0xc0de,
            env_contribution: 0,
            seal_contribution: 0,
        };

        let di = DiscoveredInputs {
            from: ".cook/deps/src.d".into(),
            format: "make".into(),
        };

        install_real_parser_once();

        // Caller passes only the declared input.
        let (result, _updated) = needs_rebuild_cook(
            Some(&entry),
            &["src.c"],
            &["out.o"],
            0xc0de,
            0,
            0,
            wd,
            None,
            Some(&di),
            false,
        );

        assert!(matches!(result, RebuildResult::Skip),
            "augmented current_inputs (declared + discovered) should match the fat entry");
    }

    #[test]
    fn missing_depfile_falls_back_to_thin_inputs() {
        use cook_contracts::DiscoveredInputs;

        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("src.c"), b"src").expect("src");
        std::fs::write(wd.join("hdr.h"), b"hdr").expect("hdr");
        std::fs::write(wd.join("out.o"), b"obj").expect("out");

        let src_hash = hash_file(&wd.join("src.c")).unwrap();
        let hdr_hash = hash_file(&wd.join("hdr.h")).unwrap();
        let out_hash = hash_file(&wd.join("out.o")).unwrap();

        let entry = StepEntry {
            inputs: vec![
                FileRecord { path: "src.c".into(), mtime: 0, hash: src_hash },
                FileRecord { path: "hdr.h".into(), mtime: 0, hash: hdr_hash },
            ],
            outputs: vec![FileRecord {
                path: "out.o".into(),
                mtime: stat_mtime(&wd.join("out.o")).unwrap_or(0),
                hash: out_hash,
            }],
            command_hash: 0xc0de,
            env_contribution: 0,
            seal_contribution: 0,
        };

        let di = DiscoveredInputs {
            from: ".cook/deps/src.d".into(),  // does not exist
            format: "make".into(),
        };

        install_real_parser_once();

        let (result, _) = needs_rebuild_cook(
            Some(&entry),
            &["src.c"],
            &["out.o"],
            0xc0de,
            0,
            0,
            wd,
            None,
            Some(&di),
            false,
        );

        // Augmentation no-ops; current=[src.c] vs entry=[src.c, hdr.h] → the
        // set diff names hdr.h as removed from the current set.
        assert!(matches!(
            &result,
            RebuildResult::Rebuild(RebuildReason::InputsChanged { removed, .. })
                if removed == &vec!["hdr.h".to_string()]
        ), "got: {result:?}");
    }

    // -------------------------------------------------------------------------
    // COOK-180: restore_one kind-dispatch + symlink-last ordering
    // -------------------------------------------------------------------------

    /// In-crate fake `CacheBackend` for unit-testing `restore_one`. A real
    /// `LocalBackend` lives in `cook-cache`, but cook-cache dev-depends on
    /// cook-fingerprint which produces two distinct crate instances in the
    /// test dependency graph — so `cook_cache::LocalBackend` implements a
    /// *different* `CacheBackend` trait than `crate::backend::CacheBackend`.
    /// This minimal in-memory fake speaks the in-crate trait. Integrity
    /// (VerifyingReader) is exercised end-to-end by cook-cache's integration
    /// restore tests; here we only need faithful kind/mode/target dispatch.
    #[derive(Default)]
    struct FakeBackend {
        store: std::sync::Mutex<
            std::collections::HashMap<crate::backend::CloudKey, (Vec<u8>, crate::backend::ArtifactMeta)>,
        >,
    }

    impl FakeBackend {
        fn insert(&self, key: crate::backend::CloudKey, bytes: Vec<u8>, meta: crate::backend::ArtifactMeta) {
            self.store.lock().unwrap().insert(key, (bytes, meta));
        }
    }

    impl crate::backend::CacheBackend for FakeBackend {
        fn batch_query(
            &self,
            keys: &[crate::backend::CloudKey],
        ) -> crate::backend::BackendResult<std::collections::BTreeSet<crate::backend::CloudKey>> {
            let store = self.store.lock().unwrap();
            Ok(keys.iter().filter(|k| store.contains_key(*k)).copied().collect())
        }
        fn get(
            &self,
            key: &crate::backend::CloudKey,
        ) -> crate::backend::BackendResult<Option<Box<dyn std::io::Read + Send>>> {
            Ok(self.get_with_meta(key)?.map(|(r, _)| r))
        }
        fn get_with_meta(
            &self,
            key: &crate::backend::CloudKey,
        ) -> crate::backend::BackendResult<Option<(Box<dyn std::io::Read + Send>, crate::backend::ArtifactMeta)>>
        {
            Ok(self
                .store
                .lock()
                .unwrap()
                .get(key)
                .map(|(b, m)| {
                    let r: Box<dyn std::io::Read + Send> = Box::new(std::io::Cursor::new(b.clone()));
                    (r, m.clone())
                }))
        }
        fn put(
            &self,
            _key: &crate::backend::CloudKey,
            _reader: &mut dyn std::io::Read,
            _meta: &mut crate::backend::ArtifactMeta,
        ) -> crate::backend::BackendResult<()> {
            Ok(())
        }
        fn delete(&self, _key: &crate::backend::CloudKey) -> crate::backend::BackendResult<()> {
            Ok(())
        }
        fn health(&self) -> crate::backend::BackendResult<()> {
            Ok(())
        }
        fn put_manifest(
            &self,
            _key: &crate::backend::CloudKey,
            _manifest: &crate::backend::DeterminantManifest,
        ) -> crate::backend::BackendResult<()> {
            Ok(())
        }
        fn get_manifest(
            &self,
            _key: &crate::backend::CloudKey,
        ) -> crate::backend::BackendResult<Option<crate::backend::DeterminantManifest>> {
            Ok(None)
        }
    }

    fn fake_meta(
        kind: Option<String>,
        mode: u32,
        target: Option<String>,
        bytes: &[u8],
    ) -> crate::backend::ArtifactMeta {
        use crate::backend::ArtifactMeta;
        use std::collections::BTreeSet;
        let content_hash = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(bytes);
            h.finalize().into()
        };
        ArtifactMeta {
            recipe_namespace: "t".into(),
            command_hash: 0,
            env_contribution: 0,
            seal_contribution: 0,
            schema_version: CACHE_VERSION,
            size_bytes: bytes.len() as u64,
            tags: BTreeSet::new(),
            consulted_env_keys: BTreeSet::new(),
            output_index: 0,
            output_path: String::new(),
            content_hash,
            kind,
            mode,
            target,
        }
    }

    #[test]
    fn restore_one_materialises_file_and_symlink() {
        use crate::backend::ArtifactMeta;

        let backend = FakeBackend::default();

        let file_key: crate::backend::CloudKey = [1u8; 32];
        let symlink_key: crate::backend::CloudKey = [2u8; 32];

        let body = b"#!/bin/sh\n";
        backend.insert(file_key, body.to_vec(), fake_meta(None, 0o755, None, body));
        backend.insert(
            symlink_key,
            Vec::new(),
            fake_meta(Some("symlink".into()), ArtifactMeta::default_mode(), Some("run".into()), b""),
        );

        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path().join("wd");
        std::fs::create_dir_all(&wd).unwrap();

        assert!(restore_one(&backend, &file_key, &wd.join("bin/run"), &wd, None));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(wd.join("bin/run")).unwrap();
            assert_eq!(m.permissions().mode() & 0o777, 0o755);
        }

        #[cfg(unix)]
        {
            assert!(restore_one(&backend, &symlink_key, &wd.join("bin/run-link"), &wd, None));
            assert!(std::fs::symlink_metadata(wd.join("bin/run-link"))
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }

    // -------------------------------------------------------------------------
    // Task 5 (COOK-180): symlink restore-time path hardening
    // -------------------------------------------------------------------------

    #[test]
    #[cfg(unix)]
    fn symlink_hardening_rejects_escapes() {
        let tmp = tempfile::tempdir().unwrap();
        let anchor = tmp.path();
        let link = anchor.join("sub/link");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        // absolute target rejected
        assert!(!restore_symlink_checked(anchor, &link, "/etc/passwd"));
        assert!(!link.exists());
        // parent-escape rejected
        assert!(!restore_symlink_checked(anchor, &link, "../../etc/passwd"));
        assert!(!std::fs::symlink_metadata(&link).map(|m| m.file_type().is_symlink()).unwrap_or(false));
        // sibling within anchor accepted
        assert!(restore_symlink_checked(anchor, &link, "sib"));
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    }

    #[test]
    #[cfg(unix)]
    fn symlink_hardening_allows_reentrant_within_anchor() {
        let tmp = tempfile::tempdir().unwrap();
        let anchor = tmp.path();
        let link = anchor.join("sub/link");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::fs::create_dir_all(anchor.join("sub2")).unwrap();
        // target `../sub2/x` from link-parent `sub/` resolves to `sub2/x` under anchor
        assert!(restore_symlink_checked(anchor, &link, "../sub2/x"));
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    }

    fn install_real_parser_once() {
        use std::sync::OnceLock;
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| {
            crate::install_depfile_parser(|p, s, wd, fmt| {
                if fmt != "make" { return Err(()); }
                cook_cache::parse_make_depfile(p, s, wd).map_err(|_| ())
            });
        });
    }
}
