//! Integration tests for multi-output restore round-trip
//! (2026-05-02 addendum spec §5.1 + §5.2).

use std::collections::BTreeSet;
use std::sync::Arc;

use cook_cache::backend::{
    artifact_key, cloud_key, ArtifactMeta, CacheBackend, CloudKeyInputs, LocalBackend,
};
use cook_cache::store::{FileRecord, StepEntry, CACHE_VERSION};
use cook_cache::{
    check::{needs_rebuild_cook, RebuildResult, RestoreCtx},
    RebuildReason,
};
use filetime::{set_file_mtime, FileTime};

// Force a deterministic mtime so the cache's mtime fast-path can't match
// across two writes that land in the same filesystem mtime tick.
fn stamp(p: &std::path::Path, secs: i64) {
    set_file_mtime(p, FileTime::from_unix_time(secs, 0)).expect("stamp mtime");
}

#[test]
fn multi_output_restore_writes_all_outputs() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store_dir = tempfile::tempdir().expect("store");
    let backend: Arc<dyn CacheBackend> =
        Arc::new(LocalBackend::new(store_dir.path().to_path_buf()));

    let wd = workspace.path();
    std::fs::write(wd.join("in.txt"), b"src").unwrap();
    std::fs::write(wd.join("foo.out"), b"foo-correct").unwrap();
    std::fs::write(wd.join("bar.out"), b"bar-correct").unwrap();
    stamp(&wd.join("foo.out"), 1_000_000_000);
    stamp(&wd.join("bar.out"), 1_000_000_000);

    let in_hash = xxhash_rust::xxh3::xxh3_64(b"src");
    let in_record = FileRecord {
        path: "in.txt".into(),
        mtime: cook_cache::stat_mtime(&wd.join("in.txt")).unwrap(),
        hash: in_hash,
    };
    let foo_record = FileRecord {
        path: "foo.out".into(),
        mtime: cook_cache::stat_mtime(&wd.join("foo.out")).unwrap(),
        hash: xxhash_rust::xxh3::xxh3_64(b"foo-correct"),
    };
    let bar_record = FileRecord {
        path: "bar.out".into(),
        mtime: cook_cache::stat_mtime(&wd.join("bar.out")).unwrap(),
        hash: xxhash_rust::xxh3::xxh3_64(b"bar-correct"),
    };

    let recipe_namespace = "proj/Cookfile::pair";
    let mut sorted = vec![in_hash];
    sorted.sort();
    let cloud_k = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace,
        command_hash: 0x1234,
        context_hash: 0,
        env_contribution: 0,
        sorted_input_content_hashes: &sorted,
    });

    // Seed both artifacts.
    for (idx, (path, bytes)) in [
        ("foo.out", &b"foo-correct"[..]),
        ("bar.out", &b"bar-correct"[..]),
    ]
    .iter()
    .enumerate()
    {
        let k = artifact_key(&cloud_k, idx as u32, path);
        let meta = ArtifactMeta {
            recipe_namespace: recipe_namespace.into(),
            command_hash: 0x1234,
            context_hash: 0,
            env_contribution: 0,
            schema_version: CACHE_VERSION,
            size_bytes: bytes.len() as u64,
            tags: BTreeSet::new(),
            consulted_env_keys: BTreeSet::new(),
            output_index: idx as u32,
            output_path: path.to_string(),
            content_hash: ArtifactMeta::zero_content_hash(),
        };
        backend.put(&k, bytes, &meta).expect("seed");
    }

    // Both outputs drift on disk. Stamp distinct mtimes so the cache's
    // mtime fast-path doesn't short-circuit (the two writes can otherwise
    // land in the same fs mtime tick as the recorded `mtime`).
    std::fs::write(wd.join("foo.out"), b"foo-stale").unwrap();
    std::fs::write(wd.join("bar.out"), b"bar-stale").unwrap();
    stamp(&wd.join("foo.out"), 2_000_000_000);
    stamp(&wd.join("bar.out"), 2_000_000_000);

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![foo_record, bar_record],
        command_hash: 0x1234,
        context_hash: 0,
        env_contribution: 0,
    };

    let ctx = RestoreCtx {
        backend: backend.as_ref(),
        recipe_namespace,
    };
    let (result, _) = needs_rebuild_cook(
        Some(&entry),
        &["in.txt"],
        &["foo.out", "bar.out"],
        0x1234,
        0,
        0,
        wd,
        Some(&ctx),
        None,
    );

    assert_eq!(result, RebuildResult::Skip);
    assert_eq!(std::fs::read(wd.join("foo.out")).unwrap(), b"foo-correct");
    assert_eq!(std::fs::read(wd.join("bar.out")).unwrap(), b"bar-correct");
}

#[test]
fn multi_output_partial_miss_falls_back_to_rebuild() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store_dir = tempfile::tempdir().expect("store");
    let backend: Arc<dyn CacheBackend> =
        Arc::new(LocalBackend::new(store_dir.path().to_path_buf()));

    let wd = workspace.path();
    std::fs::write(wd.join("in.txt"), b"src").unwrap();
    std::fs::write(wd.join("foo.out"), b"foo-stale").unwrap();
    std::fs::write(wd.join("bar.out"), b"bar-stale").unwrap();

    let in_hash = xxhash_rust::xxh3::xxh3_64(b"src");
    let in_record = FileRecord {
        path: "in.txt".into(),
        mtime: cook_cache::stat_mtime(&wd.join("in.txt")).unwrap(),
        hash: in_hash,
    };
    let foo_record = FileRecord {
        path: "foo.out".into(),
        mtime: 0,
        hash: xxhash_rust::xxh3::xxh3_64(b"foo-correct"),
    };
    let bar_record = FileRecord {
        path: "bar.out".into(),
        mtime: 0,
        hash: xxhash_rust::xxh3::xxh3_64(b"bar-correct"),
    };

    let recipe_namespace = "proj/Cookfile::pair";
    let mut sorted = vec![in_hash];
    sorted.sort();
    let cloud_k = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace,
        command_hash: 0x1234,
        context_hash: 0,
        env_contribution: 0,
        sorted_input_content_hashes: &sorted,
    });
    // Seed only the first artifact.
    let k0 = artifact_key(&cloud_k, 0, "foo.out");
    let meta0 = ArtifactMeta {
        recipe_namespace: recipe_namespace.into(),
        command_hash: 0x1234,
        context_hash: 0,
        env_contribution: 0,
        schema_version: CACHE_VERSION,
        size_bytes: 11,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: "foo.out".into(),
        content_hash: ArtifactMeta::zero_content_hash(),
    };
    backend.put(&k0, b"foo-correct", &meta0).expect("seed");

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![foo_record, bar_record],
        command_hash: 0x1234,
        context_hash: 0,
        env_contribution: 0,
    };
    let ctx = RestoreCtx {
        backend: backend.as_ref(),
        recipe_namespace,
    };
    let (result, _) = needs_rebuild_cook(
        Some(&entry),
        &["in.txt"],
        &["foo.out", "bar.out"],
        0x1234,
        0,
        0,
        wd,
        Some(&ctx),
        None,
    );
    assert!(matches!(
        result,
        RebuildResult::Rebuild(RebuildReason::OutputChanged)
    ));
}
