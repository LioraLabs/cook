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
    InputSetChanged,
    InputChanged(String),
}

/// Shared input-checking logic for cook and plate layers.
fn check_inputs(
    cached_inputs: &[FileRecord],
    current_input_paths: &[&str],
    working_dir: &Path,
) -> Result<Vec<FileRecord>, RebuildReason> {
    let cached_paths: Vec<&str> = cached_inputs.iter().map(|f| f.path.as_str()).collect();
    if cached_paths != current_input_paths {
        return Err(RebuildReason::InputSetChanged);
    }

    let mut updated = cached_inputs.to_vec();
    for (i, (cached, rel_path)) in cached_inputs
        .iter()
        .zip(current_input_paths.iter())
        .enumerate()
    {
        let abs_path = working_dir.join(rel_path);
        let disk_mtime = match stat_mtime(&abs_path) {
            Some(m) => m,
            None => return Err(RebuildReason::InputChanged(cached.path.clone())),
        };
        if disk_mtime != cached.mtime {
            let disk_hash = match hash_file(&abs_path) {
                Some(h) => h,
                None => return Err(RebuildReason::InputChanged(cached.path.clone())),
            };
            if disk_hash != cached.hash {
                return Err(RebuildReason::InputChanged(cached.path.clone()));
            }
            // Empty files are signals (marker files) — mtime is authoritative.
            if disk_hash == empty_hash() {
                return Err(RebuildReason::InputChanged(cached.path.clone()));
            }
            updated[i].mtime = disk_mtime;
        }
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

    for &idx in needs_restore {
        let path = current_outputs[idx];
        let artifact_k = crate::backend::artifact_key(&cloud_k, idx as u32, path);
        let mut reader = match ctx.backend.get(&artifact_k) {
            Ok(Some(r)) => r,
            _ => return false,
        };
        // Drain the streaming reader. The CS-0054 / CS-0056 streaming
        // verifier raises `io::Error(InvalidData)` at EOF on hash
        // mismatch; we treat any read failure (including the verifier's
        // mismatch) as a miss and fall through to rebuild.
        let mut bytes = Vec::new();
        if std::io::Read::read_to_end(&mut reader, &mut bytes).is_err() {
            return false;
        }
        // Spec §8.6 cache integrity: a restore MUST verify backend bytes
        // against the recorded fingerprint and treat any mismatch as a
        // miss. Without this, a remote / shared backend could feed
        // attacker-controlled bytes into the workspace under a cache hit.
        // For LocalBackend the check is redundant; for any networked
        // backend (Cook Cloud, CI artifact stores, etc.) it is the only
        // line of defence between `cook` and a supply-chain compromise.
        let bytes_hash = xxhash_rust::xxh3::xxh3_64(&bytes);
        if bytes_hash != entry.outputs[idx].hash {
            return false;
        }
        let abs = working_dir.join(path);
        if let Some(parent) = abs.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        // Atomic write via tmp + rename.
        let tmp = abs.with_extension("cook.tmp");
        if std::fs::write(&tmp, &bytes).is_err() {
            return false;
        }
        if std::fs::rename(&tmp, &abs).is_err() {
            return false;
        }
    }
    true
}

/// Cold fetch-by-key (COOK-162 §3 sharing): with no local StepEntry, attempt to
/// serve a unit's declared outputs straight from the shared backend by
/// recomputing its one key. Returns true iff every output was fetched, verified,
/// and written. `sorted_input_content_hashes` MUST already be sorted.
///
/// `output_paths` are the unit's declared output paths. An empty `output_paths`
/// slice returns false (no artifacts to serve is not a hit).
///
/// Two unit shapes intrinsically cannot cold-fetch and so fall through to
/// rebuild (and, for a `pinned` unit, to a hard cold-miss error):
///   * **Glob outputs** — on the cold path the declared outputs are still raw
///     patterns (e.g. `*.o`), not the concrete paths the publish path keyed its
///     artifacts under.
///   * **`discovered_inputs` (depfile) units** — the publish path folds the
///     depfile-discovered inputs into the key, but a cold consumer has no
///     depfile yet, so the `sorted_input_content_hashes` passed here (derived
///     from the *declared* inputs only) recompute a different key. This affects
///     the cc/cook_cc compile path; a fuller fix (deferred follow-up) would
///     surface the discovered inputs on the cold path before keying.
/// Both degrade safely: a non-pinned unit rebuilds; a `pinned` unit cold-misses.
#[allow(clippy::too_many_arguments)]
pub fn fetch_by_key(
    ctx: &RestoreCtx,
    command_hash: u64,
    env_contribution: u64,
    seal_contribution: u64,
    sorted_input_content_hashes: &[u64],
    output_paths: &[&str],
    working_dir: &std::path::Path,
) -> bool {
    if output_paths.is_empty() {
        return false;
    }
    let key_inputs = crate::backend::CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: ctx.recipe_namespace,
        command_hash,
        env_contribution,
        seal_contribution,
        sorted_input_content_hashes,
    };
    let cloud_k = crate::backend::cloud_key(&key_inputs);
    for (idx, path) in output_paths.iter().enumerate() {
        let artifact_k = crate::backend::artifact_key(&cloud_k, idx as u32, path);
        let mut reader = match ctx.backend.get(&artifact_k) {
            Ok(Some(r)) => r,
            _ => return false,
        };
        // Drain the streaming reader (CS-0054 verify-on-restore): the streaming
        // verifier raises io::Error(InvalidData) at EOF on hash mismatch; treat
        // any read failure as a miss.
        let mut bytes = Vec::new();
        if std::io::Read::read_to_end(&mut reader, &mut bytes).is_err() {
            return false;
        }
        let abs = working_dir.join(path);
        if let Some(parent) = abs.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        // Atomic write via tmp + rename (mirrors try_restore): never expose a
        // torn output to a concurrent reader.
        let tmp = abs.with_extension("cook.tmp");
        if std::fs::write(&tmp, &bytes).is_err() {
            return false;
        }
        if std::fs::rename(&tmp, &abs).is_err() {
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
            needs_rebuild_cook(None, &["in.c"], &["out.o"], 0xdead, 0, 0, dir.path(), None, None);
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
            needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0x2222, 0, 0, wd, None, None);
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
            needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0, wd, None, None);
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
            needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0, wd, None, None);
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
            needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0, wd, None, None);
        assert_eq!(
            result,
            RebuildResult::Rebuild(RebuildReason::InputChanged("in.c".to_string()))
        );
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

        let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0x9999, 0, wd, None, None);
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
            Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0x9999, wd, None, None,
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
        );

        // Augmentation no-ops; current=[src.c] vs entry=[src.c, hdr.h] → InputSetChanged.
        assert!(matches!(result, RebuildResult::Rebuild(RebuildReason::InputSetChanged)));
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
