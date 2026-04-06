use std::collections::BTreeMap;
use std::path::Path;

use crate::{
    hash_str,
    store::{FileRecord, StepEntry},
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
    Some(
        mtime
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64,
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

/// Hash all files matching secondary ingredient globs (index > 0).
/// Sorts paths for deterministic ordering.
pub fn hash_secondary_inputs(working_dir: &Path, ingredients: &[String]) -> u64 {
    if ingredients.len() <= 1 {
        return 0;
    }
    let mut entries: Vec<(String, u64)> = Vec::new();
    for pattern in &ingredients[1..] {
        let full = working_dir.join(pattern).to_string_lossy().to_string();
        if let Ok(paths) = glob::glob(&full) {
            let mut sorted_paths: Vec<_> = paths.filter_map(|p| p.ok()).collect();
            sorted_paths.sort();
            for entry in sorted_paths {
                if let Some(h) = hash_file(&entry) {
                    entries.push((entry.to_string_lossy().to_string(), h));
                }
            }
        }
    }
    let bytes: Vec<u8> = entries
        .iter()
        .flat_map(|(path, h)| path.as_bytes().iter().copied().chain(h.to_le_bytes()))
        .collect();
    xxhash_rust::xxh3::xxh3_64(&bytes)
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

/// Check if a cook layer (with output) needs to rebuild.
/// INVARIANT: cook.layer() calls must NOT be nested.
pub fn needs_rebuild_cook(
    entry: Option<&StepEntry>,
    current_inputs: &[&str],
    current_output: &str,
    command_hash: u64,
    working_dir: &Path,
) -> (RebuildResult, Option<StepEntry>) {
    let entry = match entry {
        None => return (RebuildResult::Rebuild(RebuildReason::NoCacheEntry), None),
        Some(e) => e,
    };
    if entry.command_hash != command_hash {
        return (
            RebuildResult::Rebuild(RebuildReason::CommandHashChanged),
            None,
        );
    }
    let abs_output = working_dir.join(current_output);
    if !abs_output.exists() {
        return (RebuildResult::Rebuild(RebuildReason::OutputMissing), None);
    }
    // Check output not tampered
    if let (Some(cached_out), Some(disk_mtime)) = (&entry.output, stat_mtime(&abs_output))
        && disk_mtime != cached_out.mtime
        && let Some(disk_hash) = hash_file(&abs_output)
        && disk_hash != cached_out.hash
    {
        return (RebuildResult::Rebuild(RebuildReason::OutputChanged), None);
    }

    match check_inputs(&entry.inputs, current_inputs, working_dir) {
        Err(reason) => (RebuildResult::Rebuild(reason), None),
        Ok(updated_inputs) => {
            let updated = StepEntry {
                inputs: updated_inputs,
                output: entry.output.clone(),
                command_hash: entry.command_hash,
            };
            (RebuildResult::Skip, Some(updated))
        }
    }
}

/// Check if a plate layer (no output) needs to re-run.
pub fn needs_rebuild_plate(
    entry: Option<&StepEntry>,
    current_inputs: &[&str],
    command_hash: u64,
    working_dir: &Path,
) -> (RebuildResult, Option<StepEntry>) {
    let entry = match entry {
        None => return (RebuildResult::Rebuild(RebuildReason::NoCacheEntry), None),
        Some(e) => e,
    };
    if entry.command_hash != command_hash {
        return (
            RebuildResult::Rebuild(RebuildReason::CommandHashChanged),
            None,
        );
    }
    match check_inputs(&entry.inputs, current_inputs, working_dir) {
        Err(reason) => (RebuildResult::Rebuild(reason), None),
        Ok(updated_inputs) => {
            let updated = StepEntry {
                inputs: updated_inputs,
                output: None,
                command_hash: entry.command_hash,
            };
            (RebuildResult::Skip, Some(updated))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FileRecord;

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
        let (result, updated) = needs_rebuild_cook(None, &["in.c"], "out.o", 0xdead, dir.path());
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
            output: Some(out_record),
            command_hash: 0x1111,
        };

        let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], "out.o", 0x2222, wd);
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
            output: None,
            command_hash: 0xbeef,
        };

        let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], "out.o", 0xbeef, wd);
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
            output: Some(out_record),
            command_hash: 0xbeef,
        };

        let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], "out.o", 0xbeef, wd);
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
            output: Some(out_record),
            command_hash: 0xbeef,
        };

        let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], "out.o", 0xbeef, wd);
        assert_eq!(
            result,
            RebuildResult::Rebuild(RebuildReason::InputChanged("in.c".to_string()))
        );
        assert!(updated.is_none());
    }

    #[test]
    fn test_plate_no_cache_entry_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (result, updated) = needs_rebuild_plate(None, &["in.c"], 0xdead, dir.path());
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
            output: None,
            command_hash: 0xbeef,
        };

        let (result, updated) = needs_rebuild_plate(Some(&entry), &["in.c"], 0xbeef, wd);
        assert_eq!(result, RebuildResult::Skip);
        let updated = updated.expect("should have updated entry");
        assert!(updated.output.is_none());
    }

    // -------------------------------------------------------------------------
    // Task 8: hash_env and hash_secondary_inputs
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

    #[test]
    fn test_hash_secondary_no_secondary() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Single ingredient → returns 0
        let ingredients = vec!["src/*.c".to_string()];
        let result = hash_secondary_inputs(dir.path(), &ingredients);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_hash_secondary_no_ingredients() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Empty slice → returns 0
        let ingredients: Vec<String> = vec![];
        let result = hash_secondary_inputs(dir.path(), &ingredients);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_hash_secondary_detects_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::create_dir_all(wd.join("include")).expect("mkdir");
        std::fs::write(wd.join("include/foo.h"), b"original").expect("write");

        let ingredients = vec!["src/*.c".to_string(), "include/*.h".to_string()];

        let h1 = hash_secondary_inputs(wd, &ingredients);

        // Modify the secondary ingredient file
        std::fs::write(wd.join("include/foo.h"), b"modified").expect("write");
        let h2 = hash_secondary_inputs(wd, &ingredients);

        assert_ne!(h1, h2);
    }
}
