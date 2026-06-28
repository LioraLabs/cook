//! Integration tests for COOK-180 per-file output fidelity: a golden
//! round-trip proving file mode / symlink target / empty-dir kind survive a
//! store-then-restore cycle, plus a security test proving path-traversal
//! symlink targets are rejected (the restore reports a miss and nothing escapes
//! the working dir). Spec §8.6 cache integrity + symlink hardening.

use std::collections::BTreeSet;
use std::sync::Arc;

use cook_cache::backend::{
    artifact_key, cloud_key, put_bytes, ArtifactMeta, CacheBackend, CloudKeyInputs, LocalBackend,
};
use cook_cache::store::{FileRecord, StepEntry, CACHE_VERSION};
use cook_cache::check::{needs_rebuild_cook, RebuildResult, RestoreCtx};

/// Seed a single artifact under `cloud_k` at `idx`/`path` with the given body,
/// kind, target and mode. Returns the artifact body's xxh3_64 (the value a
/// `FileRecord.hash` must carry for the warm-path integrity check).
fn seed(
    backend: &dyn CacheBackend,
    cloud_k: &cook_cache::backend::CloudKey,
    recipe_namespace: &str,
    command_hash: u64,
    idx: u32,
    path: &str,
    body: &[u8],
    kind: Option<&str>,
    target: Option<&str>,
    mode: u32,
) -> u64 {
    let k = artifact_key(cloud_k, idx, path);
    let mut meta = ArtifactMeta {
        recipe_namespace: recipe_namespace.into(),
        command_hash,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: CACHE_VERSION,
        size_bytes: body.len() as u64,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: idx,
        output_path: path.to_string(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: kind.map(|s| s.to_string()),
        mode,
        target: target.map(|s| s.to_string()),
    };
    put_bytes(backend, &k, body, &mut meta).expect("seed put");
    xxhash_rust::xxh3::xxh3_64(body)
}

#[test]
#[cfg(unix)]
fn golden_round_trip_restores_file_mode_symlink_and_empty_dir() {
    use std::os::unix::fs::PermissionsExt;

    let workspace = tempfile::tempdir().expect("workspace");
    let store_dir = tempfile::tempdir().expect("store");
    let backend: Arc<dyn CacheBackend> =
        Arc::new(LocalBackend::new(store_dir.path().to_path_buf()));
    let wd = workspace.path();

    // One real input so the cloud key is well-defined and reproducible on the
    // restore path (try_restore recomputes the key from the input content
    // hashes).
    std::fs::write(wd.join("in.txt"), b"src").unwrap();
    let in_hash = xxhash_rust::xxh3::xxh3_64(b"src");
    let in_record = FileRecord {
        path: "in.txt".into(),
        mtime: cook_cache::stat_mtime(&wd.join("in.txt")).unwrap(),
        hash: in_hash,
    };

    let recipe_namespace = "proj/Cookfile::tree";
    let command_hash = 0xf1de_u64;
    let mut sorted = vec![in_hash];
    sorted.sort();
    let cloud_k = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace,
        command_hash,
        env_contribution: 0,
        seal_contribution: 0,
        sorted_input_content_hashes: &sorted,
    });

    let file_body = b"#!/bin/sh\necho hi\n";
    let file_hash = seed(
        backend.as_ref(),
        &cloud_k,
        recipe_namespace,
        command_hash,
        0,
        "bin/tool",
        file_body,
        None,
        None,
        0o755,
    );
    let link_hash = seed(
        backend.as_ref(),
        &cloud_k,
        recipe_namespace,
        command_hash,
        1,
        "bin/link",
        b"",
        Some("symlink"),
        Some("tool"),
        ArtifactMeta::default_mode(),
    );
    let dir_hash = seed(
        backend.as_ref(),
        &cloud_k,
        recipe_namespace,
        command_hash,
        2,
        "empty",
        b"",
        Some("dir"),
        None,
        0o755,
    );

    // Outputs are absent in the fresh working dir, so the restore path fires
    // for every index (missing-output branch needs no mtime/drift bookkeeping).
    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![
            FileRecord {
                path: "bin/tool".into(),
                mtime: 0,
                hash: file_hash,
            },
            FileRecord {
                path: "bin/link".into(),
                mtime: 0,
                hash: link_hash,
            },
            FileRecord {
                path: "empty".into(),
                mtime: 0,
                hash: dir_hash,
            },
        ],
        command_hash,
        env_contribution: 0,
        seal_contribution: 0,
    };

    let ctx = RestoreCtx {
        backend: backend.as_ref(),
        recipe_namespace,
    };
    let (result, _) = needs_rebuild_cook(
        Some(&entry),
        &["in.txt"],
        &["bin/tool", "bin/link", "empty"],
        command_hash,
        0,
        0,
        wd,
        Some(&ctx),
        None,
        false,
    );

    assert_eq!(result, RebuildResult::Skip, "all outputs must restore cleanly");

    // File: regular file, exact mode bits, exact content.
    let tool = wd.join("bin/tool");
    let tool_meta = std::fs::symlink_metadata(&tool).expect("bin/tool exists");
    assert!(tool_meta.file_type().is_file(), "bin/tool must be a regular file");
    assert_eq!(
        tool_meta.permissions().mode() & 0o777,
        0o755,
        "bin/tool mode must round-trip"
    );
    assert_eq!(std::fs::read(&tool).unwrap(), file_body, "bin/tool content");

    // Symlink: is a symlink pointing at "tool".
    let link = wd.join("bin/link");
    let link_meta = std::fs::symlink_metadata(&link).expect("bin/link exists");
    assert!(
        link_meta.file_type().is_symlink(),
        "bin/link must be a symlink"
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        std::path::PathBuf::from("tool"),
        "symlink target must round-trip"
    );

    // Empty dir: exists and is a directory.
    let empty = wd.join("empty");
    assert!(empty.is_dir(), "empty/ must be restored as a directory");
}

#[test]
#[cfg(unix)]
fn security_poisoned_symlink_targets_are_rejected() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store_dir = tempfile::tempdir().expect("store");
    let backend: Arc<dyn CacheBackend> =
        Arc::new(LocalBackend::new(store_dir.path().to_path_buf()));
    let wd = workspace.path();

    std::fs::write(wd.join("in.txt"), b"src").unwrap();
    let in_hash = xxhash_rust::xxh3::xxh3_64(b"src");
    let in_record = FileRecord {
        path: "in.txt".into(),
        mtime: cook_cache::stat_mtime(&wd.join("in.txt")).unwrap(),
        hash: in_hash,
    };

    let recipe_namespace = "proj/Cookfile::evil";
    let command_hash = 0xbad_u64;
    let mut sorted = vec![in_hash];
    sorted.sort();
    let cloud_k = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace,
        command_hash,
        env_contribution: 0,
        seal_contribution: 0,
        sorted_input_content_hashes: &sorted,
    });

    // Two poisoned symlink artifacts: a relative escape and an absolute target.
    let rel_hash = seed(
        backend.as_ref(),
        &cloud_k,
        recipe_namespace,
        command_hash,
        0,
        "rel_link",
        b"",
        Some("symlink"),
        Some("../../etc/passwd"),
        ArtifactMeta::default_mode(),
    );
    let abs_hash = seed(
        backend.as_ref(),
        &cloud_k,
        recipe_namespace,
        command_hash,
        1,
        "abs_link",
        b"",
        Some("symlink"),
        Some("/etc/passwd"),
        ArtifactMeta::default_mode(),
    );

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![
            FileRecord {
                path: "rel_link".into(),
                mtime: 0,
                hash: rel_hash,
            },
            FileRecord {
                path: "abs_link".into(),
                mtime: 0,
                hash: abs_hash,
            },
        ],
        command_hash,
        env_contribution: 0,
        seal_contribution: 0,
    };

    let ctx = RestoreCtx {
        backend: backend.as_ref(),
        recipe_namespace,
    };
    let (result, updated) = needs_rebuild_cook(
        Some(&entry),
        &["in.txt"],
        &["rel_link", "abs_link"],
        command_hash,
        0,
        0,
        wd,
        Some(&ctx),
        None,
        false,
    );

    // The hardened symlink restore must reject both poisoned targets, so the
    // restore fails and the outcome is a rebuild (a MISS), never a clean hit.
    assert!(
        matches!(result, RebuildResult::Rebuild(_)),
        "poisoned symlink restore must NOT be a clean hit: got {result:?}"
    );
    assert!(updated.is_none(), "a miss must not return an updated entry");

    // Nothing was created inside the working dir for either poisoned link.
    assert!(
        !wd.join("rel_link").exists(),
        "relative-escape link must not be created"
    );
    assert!(
        std::fs::symlink_metadata(wd.join("rel_link")).is_err(),
        "no dangling relative-escape symlink may remain"
    );
    assert!(
        !wd.join("abs_link").exists(),
        "absolute-target link must not be created"
    );
    assert!(
        std::fs::symlink_metadata(wd.join("abs_link")).is_err(),
        "no dangling absolute-target symlink may remain"
    );
}
