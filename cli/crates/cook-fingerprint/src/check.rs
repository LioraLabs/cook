use std::collections::BTreeMap;
use std::path::Path;

use cook_contracts::cache::record::{determinant_drift, DeterminantDrift, Determinants};

use crate::{
    hash_str,
    record::{FileRecord, StepEntry, CACHE_VERSION},
};

/// COOK-360: the judge half of observe / judge / repair reports which
/// determinant moved; this crate's callers speak in rebuild reasons.
fn drift_reason(drift: DeterminantDrift) -> RebuildReason {
    match drift {
        DeterminantDrift::CommandHash => RebuildReason::CommandHashChanged,
        DeterminantDrift::Env => RebuildReason::EnvChanged,
        DeterminantDrift::Seal => RebuildReason::SealChanged,
    }
}

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

/// Streaming twin of [`hash_file`]: same algorithm over bytes arriving from a
/// reader rather than a path. Returns None if the stream errors mid-read.
///
/// CS-0173: `cook why` predicts a not-yet-restored output's content hash by
/// hashing the shared store's artifact stream as it drains it. That prediction
/// is only sound while this function and `hash_file` agree byte-for-byte on
/// the same content, which is why they live together: a change to one that
/// missed the other would not fail to compile, it would silently mispredict
/// every downstream key.
pub fn hash_reader(r: &mut dyn std::io::Read) -> Option<u64> {
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match r.read(&mut buf) {
            Ok(0) => return Some(hasher.digest()),
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
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
    let cached_paths: Vec<&str> = cached_inputs.iter().map(|f| &*f.path).collect();
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
        // COOK-306: memoised by path for the duration of a run that has not
        // written anything. The same header appears in hundreds of translation
        // units' input sets, so this is where a large graph's redundant stat
        // traffic lives. Deliberately avoids building `abs_path` until a stat
        // actually has to happen.
        let disk_mtime = match crate::statmemo::stat_mtime_memo(working_dir, rel_path) {
            Some(m) => m,
            None => {
                changed.push(cached.path.to_string());
                continue;
            }
        };
        if disk_mtime != cached.mtime {
            let abs_path = working_dir.join(rel_path);
            let disk_hash = hash_file(&abs_path);
            // Unreadable, content differs, or an empty marker file (mtime is
            // authoritative for those) → changed.
            if disk_hash != Some(cached.hash) || disk_hash == Some(empty_hash()) {
                changed.push(cached.path.to_string());
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

    // ---- JUDGE (COOK-360) -------------------------------------------------
    // A pure comparison of recorded determinants against current ones. No
    // filesystem access, no backend, no repair — and therefore shared with
    // every other store rather than hand-rolled per store. Everything below
    // this point OBSERVES the world (stat, hash) and then REPAIRS it
    // (restore), which is why those halves stay in this crate and this rule
    // does not.
    if let Some(drift) = determinant_drift(
        &Determinants::new(entry.command_hash, entry.env_contribution, entry.seal_contribution),
        &Determinants::new(command_hash, env_contribution, seal_contribution),
    ) {
        return (RebuildResult::Rebuild(drift_reason(drift)), None);
    }

    // ---- OBSERVE ----------------------------------------------------------
    // Discovered inputs: absorb-and-forget (COOK-313).
    //
    // A unit with `discovered_inputs` records a FAT entry — declared inputs
    // plus the paths its depfile named — at execution time. The check does
    // NOT re-read the depfile: the stored entry already IS the discovered
    // set, so re-parsing the `.d` recovers information we are holding.
    //
    // Until COOK-313 this re-parsed every unit's depfile on every check,
    // including a fully settled no-op that executed nothing. On DuckDB that
    // was 1,705 files and 17.15 MB of text yielding 330,415 tokens against
    // 5,128 distinct paths, each token costing a String allocation and a
    // HashSet probe, every run.
    //
    // Soundness is unchanged, because the two candidate sources say the same
    // thing. Both the `.d` on disk and `entry.inputs` describe the LAST
    // execution; neither predicts the next one. Each recorded input is still
    // stat'd, and hashed whenever its mtime moves, so:
    //
    //   - a changed header is caught by the content walk below;
    //   - a DELETED header is caught by the same walk (`stat_mtime_memo`
    //     returns None → `changed`), where it previously surfaced as the path
    //     vanishing from the re-parsed depfile — same rebuild, different
    //     reason string;
    //   - a header set that changes because the SOURCE changed is caught
    //     earlier still: the source is a declared input.
    //
    // The depfile remains an implicit OUTPUT (appended by record_completion,
    // uploaded and restored by the engine). A missing `.d` is therefore
    // caught by the output walk below as `OutputMissing` and self-heals via
    // restore or rebuild — which is the right place for it, the `.d` being an
    // output rather than an input.
    let entry_inputs_refs: Vec<&str>;
    let current_inputs_for_check: &[&str] = if discovered_inputs.is_some() {
        entry_inputs_refs = entry.inputs.iter().map(|f| &*f.path).collect();
        &entry_inputs_refs
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

    // ---- REPAIR -----------------------------------------------------------
    // Observation found outputs missing or drifted. This half does not decide
    // anything: it tries to make the world match the record, and reports
    // failure as a rebuild.
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
        observed: entry.observed.clone(),
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
///
/// COOK-233 CONSTRAINT — file restore MUST stay a byte copy (`read_to_end` ->
/// write tmp -> rename), never a hardlink or reflink of the CAS blob. The
/// restored workspace file must be a *different inode* from the blob, because
/// `LocalBackend::get_with_meta` restamps the blob's mtime on every cache hit
/// as the last-access signal for LRU eviction (`cook cache gc`). Share the
/// inode and that per-hit touch would land on the workspace file too,
/// corrupting input-freshness detection. If you want hardlink/reflink restore
/// for the disk savings, the mtime touch in `cook_cache::backend` has to go
/// first.
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
    // COOK-306: a restore writes artifacts into the working tree, so no
    // memoised mtime can be trusted afterwards. Disarmed before the attempt,
    // not after: a partial restore writes files too.
    crate::statmemo::disarm();
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
    /// Scalar observation metadata carried by the determinant manifest.
    /// Older cache entries do not have it.
    pub observation: Option<cook_contracts::cache::observation::Observation>,
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

/// Would the shared tier serve this OBSERVING unit, and with what?
///
/// An observing unit declares no outputs, so its shared hit is a *replay* of a
/// recorded observation rather than a restore of artifacts. A manifest alone
/// is not enough: the scalar half lives on the manifest and the stream half is
/// a separate artifact, and both must be present and decodable before the unit
/// can be served without running it.
///
/// # Why this is a function (COOK-401)
///
/// This question had two implementations that disagreed. `cook-engine`'s
/// executor asked it and served hits from the answer; `cook why` did not ask
/// it at all, returning "the question does not apply" for every observing unit
/// on the stated ground that such a unit "publishes nothing to the store".
///
/// That was true when CS-0186 wrote it and stopped being true at CS-0189,
/// which taught the executor to serve observing units from the shared tier and
/// updated neither the reasoning in `why.rs` nor the comment above
/// `check_observing_node` that still says "No backend is consulted". So `cook
/// why` went silent about a tier that would in fact serve the unit, on exactly
/// the unit kind CS-0189 had just taught to use it.
///
/// One function now. The executor takes the returned `Observation` and records
/// a step entry from it; `cook why` reports its presence. Neither re-derives
/// the condition, so a third CS cannot move one without the other.
///
/// Note the deliberate asymmetry with [`fetch_by_key`]: that path refuses an
/// empty output list at its own door, which is why an observing unit could
/// never be served through it and why this one exists separately.
pub fn shared_observation(
    backend: &dyn crate::backend::CacheBackend,
    key: &crate::backend::CloudKey,
) -> Option<cook_contracts::cache::observation::Observation> {
    let manifest = backend.get_manifest(key).ok().flatten()?;
    // A manifest carrying outputs belongs to a producing unit; serving it here
    // would replay an observation while leaving its artifacts unrestored.
    if !manifest.output_paths.is_empty() {
        return None;
    }
    let observation = manifest.observation?;
    // The scalar half without the stream half is not a hit.
    fetch_observation(backend, key)?;
    Some(observation)
}

/// Fetch and verify the stream half of a recorded observation.
///
/// This is deliberately separate from [`fetch_by_key`]: an observing unit has
/// no output paths to restore, and its hit is a replay rather than a restore.
pub fn fetch_observation(
    backend: &dyn crate::backend::CacheBackend,
    key: &crate::backend::CloudKey,
) -> Option<cook_contracts::cache::observation::OutputLog> {
    let artifact_k = crate::backend::artifact_key(
        key,
        crate::backend::OBSERVATION_INDEX,
        crate::backend::OBSERVATION_PATH,
    );
    let (mut reader, meta) = backend.get_with_meta(&artifact_k).ok().flatten()?;
    if meta.kind.as_deref() != Some("observation") {
        return None;
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut bytes).ok()?;
    cook_contracts::cache::observation::OutputLog::decode(&bytes).ok()
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
        let (restore_list, observation): (Vec<String>, Option<_>) =
            match ctx.backend.get_manifest(&cloud_k) {
            Ok(Some(m)) => {
                let mut list = m.output_paths;
                if let Some(di) = discovered_inputs {
                    list.push(di.from.clone());
                }
                list.extend(m.empty_dir_outputs);
                    (list, m.observation)
            }
                _ => (
                    output_paths.iter().map(|s| (*s).to_string()).collect(),
                    None,
                ),
        };
        if restore_all(ctx, &cloud_k, &restore_list, working_dir) {
            return Some(FetchOutcome {
                restored_outputs: restore_list,
                discovered_paths: set,
                observation,
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

// COOK-360: `needs_rebuild_plate` lived here — the output-less rebuild check.
// It was `needs_rebuild_cook` with the output half deleted: the same three
// early-return predicates in the same order, the same `check_inputs` call,
// then a `StepEntry` with `outputs: vec![]`. It had no production caller,
// because at the time test units were cached in a separate store and nothing
// else declared no outputs. CS-0186 folded that store in here, and an
// observing unit is judged by `needs_rebuild_cook` with an empty output slice
// — exactly the path the copy hard-coded: the discovered-inputs block is
// skipped, output augmentation is skipped, the output-count check passes
// `0 == 0`, and the output walk is empty. The copy is deleted rather than
// rewired, and the one function judges both kinds.

#[cfg(test)]
#[path = "tests/check_tests.rs"]
mod tests;
