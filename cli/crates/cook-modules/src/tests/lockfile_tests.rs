use super::*;
use std::io::Write;
use std::path::PathBuf;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lockfile/rock_tree")
}

fn sample_lockfile() -> Lockfile {
    Lockfile::new(vec![
        LockedModule {
            name: "cook_smoke".into(),
            version: "0.1.0-1".into(),
            source: "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock".into(),
            integrity: "sha256-1f3d".into(),
            direct: true,
        },
        LockedModule {
            name: "luafilesystem".into(),
            version: "1.8.0-1".into(),
            source: "https://luarocks.org/manifests/hisham/luafilesystem-1.8.0-1.src.rock"
                .into(),
            integrity: "sha256-9b2c".into(),
            direct: false,
        },
    ])
}

#[test]
fn round_trip_serialization() {
    let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cook.lock");
    let lock = sample_lockfile();
    write(&path, &lock).expect("write");
    let read_back = read(&path).expect("read");
    assert_eq!(read_back, lock);
}

#[test]
fn schema_mismatch_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cook.lock");
    let mut f = std::fs::File::create(&path).expect("create");
    writeln!(f, "schema = 99").expect("write");
    let err = read(&path).expect_err("must fail");
    assert!(format!("{:#}", err).contains("schema version 99"));
}

#[test]
fn integrity_match_ok() {
    // Use the fixture cache + a hash matching its on-disk content.
    // Compute the expected hash inline so the test is self-checking.
    let cache = fixture_root().join("lib/luarocks/cache");
    let bytes = std::fs::read(cache.join("cook_smoke-0.1.0-1.src.rock"))
        .expect("read fixture");
    let mut h = Sha256::new();
    h.update(&bytes);
    let expected = format!("sha256-{}", B64.encode(h.finalize()));
    let locked = LockedModule {
        name: "cook_smoke".into(),
        version: "0.1.0-1".into(),
        source: "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock".into(),
        integrity: expected,
        direct: true,
    };
    verify_integrity(&locked, &cache).expect("integrity ok");
}

#[test]
fn integrity_mismatch_errors_with_both_hashes() {
    let cache = fixture_root().join("lib/luarocks/cache");
    let locked = LockedModule {
        name: "cook_smoke".into(),
        version: "0.1.0-1".into(),
        source: "https://example/cook_smoke.src.rock".into(),
        integrity: "sha256-wrong".into(),
        direct: true,
    };
    let err = verify_integrity(&locked, &cache).expect_err("must fail");
    let msg = format!("{:#}", err);
    assert!(msg.contains("cook_smoke"));
    assert!(msg.contains("expects"));
    assert!(msg.contains("computes"));
}

#[test]
fn introspect_closure_marks_direct_correctly() {
    let mut manifest = ManifestModules::default();
    manifest.modules.insert("cook_smoke".into(), "*".into());
    // luafilesystem is NOT in the manifest -> direct = false.
    let modules_dir = fixture_root();
    let lock = introspect_closure(&modules_dir, &manifest).expect("introspect");
    assert_eq!(lock.modules.len(), 2);
    let by_name: std::collections::HashMap<&str, &LockedModule> = lock
        .modules
        .iter()
        .map(|m| (m.name.as_str(), m))
        .collect();
    assert!(by_name["cook_smoke"].direct);
    assert!(!by_name["luafilesystem"].direct);
    assert_eq!(by_name["cook_smoke"].version, "0.1.0-1");
    assert_eq!(
        by_name["cook_smoke"].source,
        "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock"
    );
}

#[test]
fn introspect_empty_tree_yields_empty_lockfile() {
    let dir = tempfile::tempdir().expect("tempdir");
        let lock = introspect_closure(dir.path(), &ManifestModules::default())
            .expect("introspect");
    assert!(lock.modules.is_empty());
    assert_eq!(lock.schema, SCHEMA_VERSION);
}

#[test]
fn parse_rockspec_url_skips_description_url() {
    // Some rockspecs put a non-standard `url` inside `description` before
    // `source`. The scraper must skip past it and return the source url.
    let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("foo-1.0-1.rockspec");
    std::fs::write(
        &path,
        r#"package = "foo"
version = "1.0-1"
description = {
   url = "https://homepage.example",
   summary = "x",
}
source = {
   url = "https://rocks.usecook.com/foo-1.0-1.src.rock",
}
"#,
    )
    .expect("write");
    let url = parse_rockspec_source_url(&path).expect("parse");
    assert_eq!(url, "https://rocks.usecook.com/foo-1.0-1.src.rock");
}

#[test]
fn integrity_unknown_predicate_and_guard() {
    let mut locked = LockedModule {
        name: "foo".into(),
        version: "1.0-1".into(),
        source: "https://example/foo.src.rock".into(),
        integrity: INTEGRITY_UNKNOWN.to_string(),
        direct: true,
    };
    assert!(!locked.has_known_integrity());
    let cache = fixture_root().join("lib/luarocks/cache");
    let err = verify_integrity(&locked, &cache).expect_err("must fail");
    assert!(format!("{:#}", err).contains("unknown integrity"));

    // With a real hash, the predicate is true.
    locked.integrity = "sha256-aaaa".to_string();
    assert!(locked.has_known_integrity());
}
