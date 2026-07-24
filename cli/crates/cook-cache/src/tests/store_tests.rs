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
                path: "src/main.c".to_string(),
                mtime: 1700000000,
                hash: 0x1234567890abcdef,
            },
            FileRecord {
                path: "src/util.c".to_string(),
                mtime: 1700000001,
                hash: 0xfedcba9876543210,
            },
        ],
        outputs: vec![FileRecord {
            path: "build/main.o".to_string(),
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
fn version_is_six() {
    assert_eq!(CACHE_VERSION, 6);
}

#[test]
fn round_trip_with_new_fields() {
    let original = make_populated_cache();
    let text = toml::to_string(&original).expect("serialize");
    let restored: RecipeCache = toml::from_str(&text).expect("deserialize");
    assert_eq!(original, restored);
    assert_eq!(restored.schema_version, CACHE_VERSION);
    let step = restored.steps.get("compile_main").unwrap();
    assert_eq!(step.command_hash, 0x0102030405060708);
    assert_eq!(step.env_contribution, 0x2222222222222222);
}

#[test]
fn empty_cache_round_trip() {
    let original = RecipeCache::new();
    let text = toml::to_string(&original).expect("serialize");
    let restored: RecipeCache = toml::from_str(&text).expect("deserialize");
    assert_eq!(original, restored);
}

#[test]
fn no_output_step_entry() {
    let step = StepEntry {
        inputs: vec![FileRecord {
            path: "src/main.c".to_string(),
            mtime: 1700000000,
            hash: 0x1234567890abcdef,
        }],
        outputs: vec![],
        command_hash: 0xdeadbeefcafe,
        env_contribution: 0xe0e0e0e0,
        seal_contribution: 0,
    };
    let text = toml::to_string(&step).expect("serialize");
    let restored: StepEntry = toml::from_str(&text).expect("deserialize");
    assert_eq!(step, restored);
}

#[test]
fn saved_index_is_human_readable_toml() {
    let dir = tempfile::tempdir().expect("tempdir");
        make_populated_cache().save(dir.path(), "my_recipe").expect("save");
    let path = dir.path().join("my_recipe.toml");
    let text = std::fs::read_to_string(&path).expect("read");
    assert!(text.contains("schema_version = 6"), "got: {text}");
    assert!(text.contains(r#"command_hash = "0102030405060708""#), "got: {text}");
    assert!(text.contains(r#"hash = "1234567890abcdef""#), "got: {text}");
    assert!(!dir.path().join("my_recipe.toml.tmp").exists(), "tmp renamed away");
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
    assert!(dir.path().join("@cap%2Fenv:build.toml").is_file());
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
        std::fs::write(dir.path().join("bad.toml"), b"= not toml [").expect("write");
    assert!(RecipeCache::load(dir.path(), "bad").is_none());
}

#[test]
fn load_old_schema_version_returns_none() {
    // CS-0048: a cache saved with schema_version=3 (bincode era) must be
    // refused — we save via current API then hand-patch the version field.
    let dir = tempfile::tempdir().expect("tempdir");
        let mut old = RecipeCache::new();
        old.schema_version = 3;
        old.save(dir.path(), "old_v3").expect("save");
    assert!(RecipeCache::load(dir.path(), "old_v3").is_none());
}

#[test]
fn load_future_schema_version_returns_none() {
    // CS-0048 read policy: a cache written by a future cook (schema_version
    // > CACHE_VERSION) is refused — the layout is unknown to this binary.
    let dir = tempfile::tempdir().expect("tempdir");
        let mut future = RecipeCache::new();
        future.schema_version = CACHE_VERSION + 1;
        future.save(dir.path(), "future").expect("save");
    assert!(RecipeCache::load(dir.path(), "future").is_none());
}

#[test]
fn schema_version_field_round_trips_at_cache_version() {
    // CS-0048: writers always emit `schema_version = CACHE_VERSION`.
    let cache = RecipeCache::new();
    assert_eq!(cache.schema_version, CACHE_VERSION);
    let text = toml::to_string(&cache).expect("serialize");
    let restored: RecipeCache = toml::from_str(&text).expect("deserialize");
    assert_eq!(restored.schema_version, CACHE_VERSION);
}

#[test]
fn load_index_missing_schema_version_returns_none() {
    // TOML is non-positional: missing schema_version deserialises to
    // default_cache_schema() = 1, which != CACHE_VERSION, so load refuses.
    let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("noversion.toml"), "[globs]\n\n[steps]\n")
        .expect("write");
    assert!(RecipeCache::load(dir.path(), "noversion").is_none());
}

#[test]
fn load_ignores_legacy_bin_file() {
    // Pre-v4 .bin files must not be loaded — loader only opens .toml.
    let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("legacy.bin"), b"\x00\x01junk bytes\xff")
        .expect("write");
    assert!(RecipeCache::load(dir.path(), "legacy").is_none());
}
