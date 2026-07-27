use super::*;
use crate::store::{self, FileRecord, StepEntry};

fn make_step_entry(command_hash: u64) -> StepEntry {
    StepEntry {
        inputs: vec![FileRecord {
            path: "src/main.c".into(),
            mtime: 1700000000,
            hash: 0xaabbccdd,
        }],
        outputs: vec![FileRecord {
            path: "build/main.o".into(),
            mtime: 1700000100,
            hash: 0x11223344,
        }],
        command_hash,
        env_contribution: 0,
        seal_contribution: 0,
    }
}

fn make_cache_meta(input_paths: Vec<String>, output_paths: Vec<String>) -> cook_contracts::CacheMeta {
    cook_contracts::CacheMeta {
        recipe_name: "test_recipe".into(),
        project_id: String::new(),
        cookfile_path: String::new(),
        cache_key: "step_one".into(),
        input_paths,
        consumes: Vec::new(),
        output_paths,
        command_hash: 0xdeadbeef,
        env_contribution: 0,
        consulted_env: std::collections::BTreeMap::new(),
        discovered_inputs: None,
        seal_keys: Default::default(),
        sharing: Default::default(),
        record: false,
    }
}

#[test]
fn test_thread_safe_cache_write() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
        let manager = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        manager.update_step("my_recipe", "step_one", make_step_entry(0xdeadbeef));
    manager.flush_all().expect("flush_all failed");

    let loaded = store::RecipeCache::load(dir.path(), "my_recipe")
        .expect("cache not found on disk after flush");
    let step = loaded
        .steps
        .get("step_one")
        .expect("step_one not in loaded cache");
    assert_eq!(step.command_hash, 0xdeadbeef);
}

#[test]
fn test_thread_safe_cache_multi_recipe() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
        let manager = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        manager.update_step("recipe_a", "step_a1", make_step_entry(0x1111));
    manager.update_step("recipe_b", "step_b1", make_step_entry(0x2222));
    manager.flush_all().expect("flush_all failed");

    let loaded_a =
        store::RecipeCache::load(dir.path(), "recipe_a").expect("recipe_a not found on disk");
    let loaded_b =
        store::RecipeCache::load(dir.path(), "recipe_b").expect("recipe_b not found on disk");

    assert_eq!(
        loaded_a
            .steps
            .get("step_a1")
            .expect("step_a1 missing")
            .command_hash,
        0x1111
    );
    assert_eq!(
        loaded_b
            .steps
            .get("step_b1")
            .expect("step_b1 missing")
            .command_hash,
        0x2222
    );
}

#[test]
fn test_thread_safe_cache_idempotent_flush() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
        let manager = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        manager.update_step("recipe_x", "step_x1", make_step_entry(0xabcd));
    manager.flush_all().expect("first flush_all failed");
    manager.flush_all().expect("second flush_all failed");
}

#[test]
fn test_get_or_load_missing() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
        let manager = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        let cache = manager.get_or_load("nonexistent_recipe");
    assert!(cache.steps.is_empty());
    assert_eq!(cache.schema_version, store::CACHE_VERSION);
}

#[test]
fn record_completion_writes_full_step_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
    std::fs::write(wd.join("out.o"), b"binary").expect("write");

    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).expect("mkdir cache");
    let cm = ThreadSafeCacheManager::new(cache_dir.clone());

    let meta = make_cache_meta(vec!["in.c".into()], vec!["out.o".into()]);
    cm.record_completion("rec", "step_one", &meta, wd, 0).expect("record ok");
    cm.flush_all().expect("flush");

    let loaded = store::RecipeCache::load(&cache_dir, "rec").expect("load");
    let entry = loaded.steps.get("step_one").expect("step");
    assert_eq!(entry.command_hash, 0xdeadbeef);
    assert_eq!(entry.inputs.len(), 1);
    assert_eq!(entry.outputs.len(), 1);
}

#[test]
fn record_completion_skips_on_missing_input() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        // Do NOT create "in.c" — record_completion should skip.
    std::fs::write(wd.join("out.o"), b"binary").expect("write");

    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).expect("mkdir");
        let cm = ThreadSafeCacheManager::new(cache_dir.clone());

        let meta = make_cache_meta(vec!["in.c".into()], vec!["out.o".into()]);
    let err = cm.record_completion("rec", "step_one", &meta, wd, 0).unwrap_err();
    assert!(matches!(err, RecordError::MissingFile(_)));

    // Verify nothing was written.
    cm.flush_all().expect("flush");
    let loaded = store::RecipeCache::load(&cache_dir, "rec");
    assert!(loaded.is_none() || loaded.unwrap().steps.is_empty());
}

#[test]
fn record_completion_appends_depfile_to_outputs() {
    use cook_contracts::DiscoveredInputs;

    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("a.c"), b"src").expect("a.c");
    std::fs::write(wd.join("a.o"), b"obj").expect("a.o");
    std::fs::create_dir_all(wd.join(".cook/deps")).expect("mkdir");
        std::fs::write(wd.join(".cook/deps/a.d"), b"a.o: a.c\n").expect("dep");

    let cache_dir = wd.join(".cook/cache");
    std::fs::create_dir_all(&cache_dir).expect("cachedir");
        let mgr = ThreadSafeCacheManager::new(cache_dir.clone());

        let mut meta = make_cache_meta(vec!["a.c".into()], vec!["a.o".into()]);
    meta.discovered_inputs = Some(DiscoveredInputs {
        from: ".cook/deps/a.d".into(),
        format: "make".into(),
    });

    let entry = mgr.record_completion("rec", "k", &meta, wd, 0).expect("rec");

    let output_paths: Vec<&str> =
        entry.outputs.iter().map(|fr| fr.path.as_ref()).collect();
    assert!(output_paths.contains(&"a.o"), "user output present");
    assert!(output_paths.contains(&".cook/deps/a.d"),
        "depfile appended to outputs when discovered_inputs is set");
}

#[test]
fn record_completion_preserves_prior_entry_on_skip() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
    std::fs::write(wd.join("out.o"), b"binary").expect("write");

    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).expect("mkdir");
        let cm = ThreadSafeCacheManager::new(cache_dir.clone());

        // First successful record.
        let meta = make_cache_meta(vec!["in.c".into()], vec!["out.o".into()]);
    cm.record_completion("rec", "step_one", &meta, wd, 0).expect("record 1");
    cm.flush_all().expect("flush 1");

    // Now remove the input and try again — must err and leave prior entry intact.
    std::fs::remove_file(wd.join("in.c")).expect("rm");
    let err = cm.record_completion("rec", "step_one", &meta, wd, 0).unwrap_err();
    assert!(matches!(err, RecordError::MissingFile(_)));
    cm.flush_all().expect("flush 2");

    let loaded = store::RecipeCache::load(&cache_dir, "rec").expect("load");
    let entry = loaded.steps.get("step_one").expect("prior entry survives");
    assert_eq!(entry.command_hash, 0xdeadbeef);
}

#[test]
fn retain_steps_drops_and_persists() {
    let dir = tempfile::tempdir().expect("tempdir");
        let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());
        cm.update_step("rec", "keep", make_step_entry(0x1));
    cm.update_step("rec", "drop", make_step_entry(0x2));
    cm.flush_all().expect("flush 1");

    // Reload into a fresh manager, then retain only "keep".
    let cm2 = ThreadSafeCacheManager::new(dir.path().to_path_buf());
    cm2.get_or_load("rec");
    cm2.retain_steps("rec", |k, _| k == "keep");
    cm2.flush_all().expect("flush 2");

    let loaded = store::RecipeCache::load(dir.path(), "rec").expect("load");
    assert!(loaded.steps.contains_key("keep"));
    assert!(!loaded.steps.contains_key("drop"), "stale step pruned");
}

#[test]
fn manager_construction_sweeps_superseded_indexes() {
    let dir = tempfile::tempdir().expect("tempdir");
        // Legacy bincode index + torn tmp from an interrupted pre-v4 write.
        std::fs::write(dir.path().join("old_recipe.bin"), b"\x03legacy").expect("bin");
    std::fs::write(dir.path().join("old_recipe.bin.tmp"), b"torn").expect("tmp");
    // Things the sweep must NOT touch: the live `.idx` index, and subdirs —
    // except `tests/`, the removed test-result store, which it now takes
    // (CS-0186).
    store::RecipeCache::new().save(dir.path(), "current").expect("save");
    std::fs::create_dir_all(dir.path().join("tests/ab")).expect("mkdir");
    std::fs::write(dir.path().join("tests/ab/abcd1234.json"), b"{}").expect("json");
    std::fs::create_dir_all(dir.path().join("cc-state")).expect("mkdir");
    std::fs::write(dir.path().join("cc-state/probe.json"), b"{}").expect("json");
    let _mgr = ThreadSafeCacheManager::new(dir.path().to_path_buf());
    assert!(!dir.path().join("old_recipe.bin").exists(), ".bin swept");
    assert!(!dir.path().join("old_recipe.bin.tmp").exists(), ".bin.tmp swept");
    assert!(dir.path().join("current.idx").exists(), "live index untouched");
    assert!(!dir.path().join("tests").exists(), "removed test-result store swept");
    assert!(
        dir.path().join("cc-state/probe.json").exists(),
        "every other subdirectory untouched"
    );
}

#[test]
fn manager_construction_tolerates_missing_cache_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");
    let _mgr = ThreadSafeCacheManager::new(missing); // must not panic or create the dir
    assert!(!dir.path().join("does-not-exist").exists());
}

#[test]
fn retain_steps_keeps_all_is_not_dirty() {
    let dir = tempfile::tempdir().expect("tempdir");
        let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());
        cm.update_step("rec", "a", make_step_entry(0x1));
    cm.retain_steps("rec", |_, _| true);
    // Nothing removed; flush still succeeds and the step survives.
    cm.flush_all().expect("flush");
    let loaded = store::RecipeCache::load(dir.path(), "rec").expect("load");
    assert!(loaded.steps.contains_key("a"));
}

// --- COOK-306 -------------------------------------------------------------

#[test]
fn lookup_step_returns_the_keyed_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());
    cm.update_step("rec", "build/main.o", make_step_entry(0xfeed));

    let hit = cm.lookup_step("rec", "build/main.o", "build/main.o");
    assert_eq!(hit.entry.expect("entry").command_hash, 0xfeed);
    assert!(!hit.env_moved_key, "a keyed hit never reports env-moved");
}

#[test]
fn lookup_step_reports_a_cold_miss_as_cold() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());

    let miss = cm.lookup_step("rec", "build/main.o@abc", "build/main.o");
    assert!(miss.entry.is_none());
    assert!(
        !miss.env_moved_key,
        "no sibling entry exists, so the miss is genuinely cold"
    );
}

/// COOK-276 attribution, preserved through the COOK-306 rewrite: history
/// parked under a different env suffix makes the miss env-attributable.
#[test]
fn lookup_step_attributes_a_moved_env_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());
    cm.update_step("rec", "build/main.o@OLDENV", make_step_entry(0x1));

    let miss = cm.lookup_step("rec", "build/main.o@NEWENV", "build/main.o");
    assert!(miss.entry.is_none());
    assert!(miss.env_moved_key, "sibling under the same output identity");
}

#[test]
fn lookup_step_attributes_a_bare_output_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());
    cm.update_step("rec", "build/main.o", make_step_entry(0x1));

    let miss = cm.lookup_step("rec", "build/main.o@NEWENV", "build/main.o");
    assert!(miss.entry.is_none());
    assert!(miss.env_moved_key, "bare-key history counts as history");
}

/// A prefix match must not leak across output identities: `build/main.o2` is a
/// different artifact from `build/main.o`, not an env variant of it.
#[test]
fn lookup_step_does_not_attribute_a_neighbouring_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());
    cm.update_step("rec", "build/main.o2@ENV", make_step_entry(0x1));

    let miss = cm.lookup_step("rec", "build/main.o@ENV", "build/main.o");
    assert!(!miss.env_moved_key, "'@' boundary must be respected");
}

/// The COOK-306 perf invariant, which nothing else can observe: re-writing an
/// identical entry must NOT mark the recipe dirty, or a settled run pays a
/// full index re-serialisation (seconds, on a large graph) to write bytes it
/// already has.
#[test]
fn rewriting_an_identical_entry_does_not_dirty_the_recipe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());
    cm.update_step("rec", "step", make_step_entry(0xabc));
    cm.flush_all().expect("flush 1");

    let index = dir.path().join("rec.idx");
    let stamp = std::fs::metadata(&index).expect("stat").modified().expect("mtime");

    // Same entry again, then flush: the file must not be rewritten.
    cm.update_step("rec", "step", make_step_entry(0xabc));
    cm.flush_all().expect("flush 2");
    assert_eq!(
        stamp,
        std::fs::metadata(&index).expect("stat").modified().expect("mtime"),
        "an identical write must leave the index untouched"
    );

    // A genuinely different entry still persists.
    cm.update_step("rec", "step", make_step_entry(0xdef));
    cm.flush_all().expect("flush 3");
    let loaded = store::RecipeCache::load(dir.path(), "rec").expect("load");
    assert_eq!(loaded.steps.get("step").expect("step").command_hash, 0xdef);
}
