//! Integration tests for 2026-05-02 addendum spec §5.2:
//! `needs_rebuild_cook` restores drifted on-disk output bytes from the
//! backend rather than rebuilding, when the cache entry's hashes still
//! match the current step.

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

fn write(p: &std::path::Path, bytes: &[u8]) {
    std::fs::write(p, bytes).expect("write");
}

#[test]
fn restore_on_hit_writes_bytes_back_to_disk_and_returns_skip() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let store_dir = tempfile::tempdir().expect("store tempdir");
    let backend: Arc<dyn CacheBackend> =
        Arc::new(LocalBackend::new(store_dir.path().to_path_buf()));

    let wd = workspace.path();
    write(&wd.join("in.c"), b"int main(){}");
    write(&wd.join("out.o"), b"correct-bytes");

    let in_hash = xxhash_rust::xxh3::xxh3_64(b"int main(){}");
    let in_record = FileRecord {
        path: "in.c".into(),
        mtime: cook_cache::stat_mtime(&wd.join("in.c")).unwrap(),
        hash: in_hash,
    };
    let out_record = FileRecord {
        path: "out.o".into(),
        mtime: cook_cache::stat_mtime(&wd.join("out.o")).unwrap(),
        hash: xxhash_rust::xxh3::xxh3_64(b"correct-bytes"),
    };

    let recipe_namespace = "proj/Cookfile::build";
    let mut sorted = vec![in_hash];
    sorted.sort();
    let cloud_k = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace,
        command_hash: 0xbeef,
        context_hash: 0,
        env_contribution: 0,
        sorted_input_content_hashes: &sorted,
    });
    let artifact_k = artifact_key(&cloud_k, 0, "out.o");

    // Seed the artifact store with the correct bytes.
    let meta = ArtifactMeta {
        recipe_namespace: recipe_namespace.into(),
        command_hash: 0xbeef,
        context_hash: 0,
        env_contribution: 0,
        schema_version: CACHE_VERSION,
        size_bytes: 13,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: "out.o".into(),
    };
    backend
        .put(&artifact_k, b"correct-bytes", &meta)
        .expect("seed put");

    // Simulate variant-toggle drift: overwrite with stale bytes.
    write(&wd.join("out.o"), b"stale-variant");

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![out_record],
        command_hash: 0xbeef,
        context_hash: 0,
        env_contribution: 0,
    };

    let ctx = RestoreCtx {
        backend: backend.as_ref(),
        recipe_namespace,
    };
    let (result, _) = needs_rebuild_cook(
        Some(&entry),
        &["in.c"],
        &["out.o"],
        0xbeef,
        0,
        0,
        wd,
        Some(&ctx),
    );

    assert_eq!(result, RebuildResult::Skip);
    let on_disk = std::fs::read(wd.join("out.o")).expect("read out.o");
    assert_eq!(
        on_disk, b"correct-bytes",
        "bytes must be restored from artifact store"
    );
}

#[test]
fn restore_miss_falls_through_to_output_changed() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store_dir = tempfile::tempdir().expect("store");
    let backend: Arc<dyn CacheBackend> =
        Arc::new(LocalBackend::new(store_dir.path().to_path_buf()));

    let wd = workspace.path();
    write(&wd.join("in.c"), b"int main(){}");
    write(&wd.join("out.o"), b"stale");

    let in_hash = xxhash_rust::xxh3::xxh3_64(b"int main(){}");
    let in_record = FileRecord {
        path: "in.c".into(),
        mtime: cook_cache::stat_mtime(&wd.join("in.c")).unwrap(),
        hash: in_hash,
    };
    let out_record = FileRecord {
        path: "out.o".into(),
        mtime: 0,
        hash: xxhash_rust::xxh3::xxh3_64(b"different"),
    };

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![out_record],
        command_hash: 0xbeef,
        context_hash: 0,
        env_contribution: 0,
    };

    let ctx = RestoreCtx {
        backend: backend.as_ref(),
        recipe_namespace: "proj/Cookfile::build",
    };
    let (result, _) = needs_rebuild_cook(
        Some(&entry),
        &["in.c"],
        &["out.o"],
        0xbeef,
        0,
        0,
        wd,
        Some(&ctx),
    );

    assert!(matches!(
        result,
        RebuildResult::Rebuild(RebuildReason::OutputChanged)
    ));
}

#[test]
fn restore_with_no_ctx_returns_output_changed() {
    // Without a RestoreCtx, drift behavior MUST match the pre-amendment code.
    let workspace = tempfile::tempdir().expect("workspace");
    let wd = workspace.path();
    write(&wd.join("in.c"), b"int main(){}");
    write(&wd.join("out.o"), b"stale");

    let in_hash = xxhash_rust::xxh3::xxh3_64(b"int main(){}");
    let in_record = FileRecord {
        path: "in.c".into(),
        mtime: cook_cache::stat_mtime(&wd.join("in.c")).unwrap(),
        hash: in_hash,
    };
    let out_record = FileRecord {
        path: "out.o".into(),
        mtime: 0,
        hash: xxhash_rust::xxh3::xxh3_64(b"different"),
    };

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![out_record],
        command_hash: 0xbeef,
        context_hash: 0,
        env_contribution: 0,
    };

    let (result, _) = needs_rebuild_cook(
        Some(&entry),
        &["in.c"],
        &["out.o"],
        0xbeef,
        0,
        0,
        wd,
        None,
    );

    assert!(matches!(
        result,
        RebuildResult::Rebuild(RebuildReason::OutputChanged)
    ));
}
