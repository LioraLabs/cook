use super::*;

fn make_populated_cache() -> RecipeCache {
    let mut cache = RecipeCache::new();

    let mut globs = BTreeMap::new();
    globs.insert(
        "src/*.c".to_string(),
        BTreeSet::from(["src/main.c".to_string(), "src/util.c".to_string()]),
    );
    cache.globs = globs;

    let step = StepEntry {
        inputs: vec![
            FileRecord {
                path: "src/main.c".into(),
                mtime: 1700000000,
                hash: 0x1234567890abcdef,
            },
            FileRecord {
                path: "src/util.c".into(),
                mtime: 1700000001,
                hash: 0xfedcba9876543210,
            },
        ],
        outputs: vec![FileRecord {
            path: "build/main.o".into(),
            mtime: 1700000100,
            hash: 0xabcdef1234567890,
        }],
        command_hash: 0x0102030405060708,
        env_contribution: 0x2222222222222222,
        seal_contribution: 0,
    };
    cache.steps.insert("compile_main".to_string(), step);

    cache
}

#[test]
fn version_is_seven() {
    assert_eq!(CACHE_VERSION, 7);
}

#[test]
fn no_output_step_entry_round_trips() {
    let mut cache = RecipeCache::new();
    cache.steps.insert(
        "no_outputs".to_string(),
        StepEntry {
            inputs: vec![FileRecord {
                path: "src/main.c".into(),
                mtime: 1700000000,
                hash: 0x1234567890abcdef,
            }],
            outputs: vec![],
            command_hash: 0xdeadbeefcafe,
            env_contribution: 0xe0e0e0e0,
            seal_contribution: 0,
        },
    );
    let restored = crate::index_bin::decode(&crate::index_bin::encode(&cache)).expect("decode");
    assert_eq!(cache, restored);
}

#[test]
fn saved_index_is_a_binary_idx_file() {
    // CS-0166: readability moved to `cook why` / `cook cache dump`. What the
    // file must still guarantee is the magic and version in its header, so a
    // foreign file is never mistaken for an index.
    let dir = tempfile::tempdir().expect("tempdir");
    make_populated_cache().save(dir.path(), "my_recipe").expect("save");
    let path = dir.path().join("my_recipe.idx");
    let bytes = std::fs::read(&path).expect("read");
    assert_eq!(&bytes[0..8], b"COOKIDX\0");
    assert_eq!(
        u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        CACHE_VERSION
    );
    assert!(!dir.path().join("my_recipe.idx.tmp").exists(), "tmp renamed away");
    assert!(!dir.path().join("my_recipe.toml").exists(), "no TOML written");
}

#[test]
fn save_and_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    let original = make_populated_cache();
    original.save(dir.path(), "my_recipe").expect("save");
    let loaded = RecipeCache::load(dir.path(), "my_recipe").expect("load");
    assert_eq!(original, loaded);
}

#[test]
fn load_missing_returns_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(RecipeCache::load(dir.path(), "nonexistent").is_none());
}

#[test]
fn save_and_load_scoped_recipe_name() {
    // npm-scoped names ("@cap/env:build") embed a `/`; a raw path join
    // aimed the write at a directory that never exists, so the recipe
    // silently never cached.
    let dir = tempfile::tempdir().expect("tempdir");
    let original = make_populated_cache();
    original.save(dir.path(), "@cap/env:build").expect("save");
    let loaded = RecipeCache::load(dir.path(), "@cap/env:build").expect("load");
    assert_eq!(original, loaded);
    // Flat file, percent-encoded — no subdirectory materialised.
    assert!(dir.path().join("@cap%2Fenv:build.idx").is_file());
    assert!(!dir.path().join("@cap").exists());
}

#[test]
fn cache_file_basename_is_injective_for_escape_byte() {
    // A literal "%2F" in a recipe name must not collide with an
    // encoded "/".
    assert_eq!(cache_file_basename("a/b"), "a%2Fb");
    assert_eq!(cache_file_basename("a%2Fb"), "a%252Fb");
    assert_eq!(cache_file_basename("plain:name"), "plain:name");
}

#[test]
fn load_corrupted_returns_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("bad.idx"), b"not an index at all").expect("write");
    assert!(RecipeCache::load(dir.path(), "bad").is_none());
}

#[test]
fn load_truncated_returns_none() {
    // A crash mid-write must degrade to a cache miss, not a build failure.
    let dir = tempfile::tempdir().expect("tempdir");
    make_populated_cache().save(dir.path(), "torn").expect("save");
    let path = dir.path().join("torn.idx");
    let bytes = std::fs::read(&path).expect("read");
    std::fs::write(&path, &bytes[..bytes.len() - 12]).expect("truncate");
    assert!(RecipeCache::load(dir.path(), "torn").is_none());
}

#[test]
fn load_wrong_schema_version_returns_none() {
    // CS-0048 read policy in both directions: an older cook's index and a
    // future cook's index are equally unreadable.
    let dir = tempfile::tempdir().expect("tempdir");
    for version in [CACHE_VERSION - 1, CACHE_VERSION + 1] {
        make_populated_cache().save(dir.path(), "versioned").expect("save");
        let path = dir.path().join("versioned.idx");
        let mut bytes = std::fs::read(&path).expect("read");
        bytes[8..12].copy_from_slice(&version.to_le_bytes());
        std::fs::write(&path, &bytes).expect("write");
        assert!(
            RecipeCache::load(dir.path(), "versioned").is_none(),
            "schema_version {version} must be refused"
        );
    }
}

#[test]
fn load_ignores_superseded_formats() {
    // The loader only opens `.idx`; a v6 `.toml` or pre-v4 `.bin` sitting
    // under the same recipe name is invisible to it.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("legacy.bin"), b"\x00\x01junk bytes\xff").expect("write");
    std::fs::write(dir.path().join("legacy.toml"), "schema_version = 6\n").expect("write");
    assert!(RecipeCache::load(dir.path(), "legacy").is_none());
}

#[test]
fn sweep_removes_superseded_indexes_only() {
    // CS-0166: no migration — superseded indexes are deleted. The cache dir
    // is shared with module state (`cook_cc.json` lives in every cc-built
    // project's `.cook/cache/`), so the sweep must not be an `.idx` allowlist.
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    for name in [
        "old.toml",
        "old.toml.tmp",
        "ancient.bin",
        "ancient.bin.tmp",
        "torn.idx.tmp",
    ] {
        std::fs::write(d.join(name), b"x").expect("write");
    }
    std::fs::write(d.join("cook_cc.json"), b"{}").expect("write");
    make_populated_cache().save(d, "live").expect("save");
    std::fs::create_dir_all(d.join("tests")).expect("mkdir");
    std::fs::write(d.join("tests").join("a.json"), b"{}").expect("write");

    sweep_superseded_indexes(d);

    for gone in [
        "old.toml",
        "old.toml.tmp",
        "ancient.bin",
        "ancient.bin.tmp",
        "torn.idx.tmp",
    ] {
        assert!(!d.join(gone).exists(), "{gone} should have been swept");
    }
    assert!(d.join("cook_cc.json").is_file(), "module state must survive");
    assert!(d.join("live.idx").is_file(), "the live index must survive");
    assert!(
        d.join("tests").join("a.json").is_file(),
        "subdirectories are never touched"
    );
}

#[test]
fn sweep_on_missing_dir_is_a_noop() {
    let dir = tempfile::tempdir().expect("tempdir");
    sweep_superseded_indexes(&dir.path().join("does-not-exist"));
}

#[test]
fn manager_construction_sweeps_a_stale_toml_index() {
    // End-to-end: the upgrade path a real user hits is "cook runs, the old
    // index is gone, the graph rebuilds once".
    let dir = tempfile::tempdir().expect("tempdir");
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).expect("mkdir");
    std::fs::write(cache_dir.join("build.toml"), "schema_version = 6\n").expect("write");

    let _mgr = crate::ThreadSafeCacheManager::new(cache_dir.clone());

    assert!(!cache_dir.join("build.toml").exists());
    assert!(RecipeCache::load(&cache_dir, "build").is_none());
}
