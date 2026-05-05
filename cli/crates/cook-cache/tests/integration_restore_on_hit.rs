//! Integration tests for 2026-05-02 addendum spec §5.2:
//! `needs_rebuild_cook` restores drifted on-disk output bytes from the
//! backend rather than rebuilding, when the cache entry's hashes still
//! match the current step.

use std::collections::BTreeSet;
use std::sync::Arc;

use cook_cache::backend::{
    artifact_key, cloud_key, put_bytes, ArtifactMeta, CacheBackend, CloudKeyInputs, LocalBackend,
};
use cook_cache::store::{FileRecord, StepEntry, CACHE_VERSION};
use cook_cache::{
    check::{needs_rebuild_cook, RebuildResult, RestoreCtx},
    RebuildReason,
};
use filetime::{set_file_mtime, FileTime};

fn write(p: &std::path::Path, bytes: &[u8]) {
    std::fs::write(p, bytes).expect("write");
}

// Force a deterministic mtime so the cache's mtime fast-path can't match
// across two writes that land in the same filesystem mtime tick.
fn stamp(p: &std::path::Path, secs: i64) {
    set_file_mtime(p, FileTime::from_unix_time(secs, 0)).expect("stamp mtime");
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
    stamp(&wd.join("out.o"), 1_000_000_000);

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
    let mut meta = ArtifactMeta {
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
        content_hash: ArtifactMeta::zero_content_hash(),
    };
    put_bytes(backend.as_ref(), &artifact_k, b"correct-bytes", &mut meta)
        .expect("seed put");

    // Simulate variant-toggle drift: overwrite with stale bytes, and force a
    // distinct mtime so the cache's mtime fast-path doesn't short-circuit
    // (the two writes can otherwise land in the same fs mtime tick).
    write(&wd.join("out.o"), b"stale-variant");
    stamp(&wd.join("out.o"), 2_000_000_000);

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
        None,
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
        None,
    );

    assert!(matches!(
        result,
        RebuildResult::Rebuild(RebuildReason::OutputChanged)
    ));
}

#[test]
fn restore_rejects_tampered_backend_bytes() {
    // Regression: pre-fix, `try_restore` wrote whatever the backend
    // returned to disk without verifying its hash. A remote/shared
    // backend that returned attacker-controlled bytes for a hit key
    // would silently install them. Post-fix, the byte hash MUST be
    // compared against `entry.outputs[idx].hash` and a mismatch MUST
    // fall through to the rebuild path. See spec §8.6 cache integrity.
    let workspace = tempfile::tempdir().expect("workspace");
    let store_dir = tempfile::tempdir().expect("store");
    let backend: Arc<dyn CacheBackend> =
        Arc::new(LocalBackend::new(store_dir.path().to_path_buf()));

    let wd = workspace.path();
    write(&wd.join("in.c"), b"int main(){}");
    // out.o: real prior content; our cached FileRecord pins this hash.
    write(&wd.join("out.o"), b"correct-bytes");
    stamp(&wd.join("out.o"), 1_000_000_000);

    let in_hash = xxhash_rust::xxh3::xxh3_64(b"int main(){}");
    let in_record = FileRecord {
        path: "in.c".into(),
        mtime: cook_cache::stat_mtime(&wd.join("in.c")).unwrap(),
        hash: in_hash,
    };
    let real_out_hash = xxhash_rust::xxh3::xxh3_64(b"correct-bytes");
    let out_record = FileRecord {
        path: "out.o".into(),
        mtime: cook_cache::stat_mtime(&wd.join("out.o")).unwrap(),
        hash: real_out_hash,
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

    // Seed the backend with TAMPERED bytes whose hash does NOT match
    // `out_record.hash`. This simulates a malicious or corrupted remote
    // backend response under the artifact key for which the engine has
    // a legitimate cache hit signal.
    let tampered = b"TAMPERED-BY-AUDIT";
    let mut meta = ArtifactMeta {
        recipe_namespace: recipe_namespace.into(),
        command_hash: 0xbeef,
        context_hash: 0,
        env_contribution: 0,
        schema_version: CACHE_VERSION,
        size_bytes: tampered.len() as u64,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: "out.o".into(),
        content_hash: ArtifactMeta::zero_content_hash(),
    };
    put_bytes(backend.as_ref(), &artifact_k, tampered, &mut meta)
        .expect("seed put with tampered bytes");

    // Force the restore-on-hit path: drift the on-disk output so its
    // hash no longer matches `out_record.hash`. Stamp a distinct mtime
    // so the cache's mtime fast-path doesn't short-circuit.
    write(&wd.join("out.o"), b"stale-variant");
    stamp(&wd.join("out.o"), 2_000_000_000);

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
    let (result, updated) = needs_rebuild_cook(
        Some(&entry),
        &["in.c"],
        &["out.o"],
        0xbeef,
        0,
        0,
        wd,
        Some(&ctx),
        None,
    );

    // The hash mismatch MUST be treated as a restore miss, which falls
    // through to OutputChanged (since the on-disk file existed but had
    // drifted bytes).
    assert!(
        matches!(result, RebuildResult::Rebuild(RebuildReason::OutputChanged)),
        "tampered backend bytes must be rejected: got {result:?}"
    );
    assert!(updated.is_none(), "rebuild path must not return an updated entry");

    // The on-disk file MUST NOT be overwritten with the tampered bytes.
    let on_disk = std::fs::read(wd.join("out.o")).expect("read out.o");
    assert_ne!(
        on_disk, tampered,
        "on-disk bytes must not have been replaced with tampered bytes"
    );
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
        None,
    );

    assert!(matches!(
        result,
        RebuildResult::Rebuild(RebuildReason::OutputChanged)
    ));
}
