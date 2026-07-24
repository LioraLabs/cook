//! COOK-278: edit-then-revert must restore the cached artifact, not
//! re-execute — including the two shapes the original fetch path missed:
//!
//!   1. **Content-dependent output names** (Next.js chunk hashes): the
//!      caller's output list on a warm revert is the LAST run's concrete
//!      names, which are wrong for the candidate key being probed. The
//!      candidate key's determinant manifest records the right list.
//!   2. **Discovered-set changes**: the single-set COOK-177 manifest is
//!      last-writer-wins, so an edit that changes the discovered SET erased
//!      the older set. The multi-set manifest keeps every set as a
//!      candidate; validation is key recomposition (a set whose files hash
//!      differently composes a different key and naturally misses).
//!
//! Plus format compatibility: a store written by a pre-COOK-278 binary has
//! only the v1 single-set manifest; recovery must still work through it.

use cook_cache::backend::{
    artifact_key, cloud_key, put_bytes, ArtifactMeta, CloudKeyInputs, DeterminantManifest,
    LocalBackend,
};
use cook_cache::store::CACHE_VERSION;
use cook_contracts::DiscoveredInputs;
use cook_fingerprint::{
    fetch_by_key, read_discovered_input_sets, CacheBackend, RestoreCtx,
    DISCOVERED_INPUTS_MANIFEST_INDEX, DISCOVERED_INPUTS_MANIFEST_PATH,
    DISCOVERED_INPUT_SETS_INDEX, DISCOVERED_INPUT_SETS_PATH,
};

const RECIPE_NS: &str = "proj/Cookfile::build";
const CMD_HASH: u64 = 0x0C;
const ENV_CONTRIB: u64 = 0x0E;
const SEAL_CONTRIB: u64 = 0x05;

const MAIN_SRC: &[u8] = b"main line\n";
const HEADER_ORIG: &[u8] = b"original header\n";
const DEPFILE_BYTES: &[u8] = b"out: main.src header.h\n";

fn xxh(bytes: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(bytes)
}

fn key_for(hashes: &mut Vec<u64>) -> [u8; 32] {
    hashes.sort();
    cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: RECIPE_NS,
        command_hash: CMD_HASH,
        env_contribution: ENV_CONTRIB,
        seal_contribution: SEAL_CONTRIB,
        sorted_input_content_hashes: hashes,
    })
}

fn put_artifact(backend: &LocalBackend, key: &[u8; 32], idx: u32, path: &str, bytes: &[u8]) {
    let artifact_k = artifact_key(key, idx, path);
    let mut meta = ArtifactMeta {
        recipe_namespace: RECIPE_NS.to_string(),
        command_hash: CMD_HASH,
        env_contribution: ENV_CONTRIB,
        seal_contribution: SEAL_CONTRIB,
        schema_version: CACHE_VERSION,
        size_bytes: bytes.len() as u64,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: idx,
        output_path: path.to_string(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: None,
        mode: ArtifactMeta::default_mode(),
        target: None,
    };
    put_bytes(backend, &artifact_k, bytes, &mut meta).expect("seed artifact");
}

fn put_json_artifact(backend: &LocalBackend, key: &[u8; 32], idx: u32, path: &str, json: &[u8]) {
    let artifact_k = artifact_key(key, idx, path);
    let mut meta = ArtifactMeta {
        recipe_namespace: RECIPE_NS.to_string(),
        command_hash: CMD_HASH,
        env_contribution: ENV_CONTRIB,
        seal_contribution: SEAL_CONTRIB,
        schema_version: CACHE_VERSION,
        size_bytes: json.len() as u64,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: idx,
        output_path: path.to_string(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: Some("discovered_inputs".to_string()),
        mode: ArtifactMeta::default_mode(),
        target: None,
    };
    put_bytes(backend, &artifact_k, json, &mut meta).expect("seed manifest");
}

fn put_determinant_manifest(
    backend: &LocalBackend,
    key: &[u8; 32],
    output_paths: Vec<String>,
) {
    let manifest = DeterminantManifest {
        schema_version: CACHE_VERSION,
        recipe_namespace: RECIPE_NS.to_string(),
        key: hex::encode(key),
        command_hash: CMD_HASH,
        env_contribution: ENV_CONTRIB,
        seal_contribution: SEAL_CONTRIB,
        inputs: Default::default(),
        output_paths,
        empty_dir_outputs: Vec::new(),
        consulted_env: Default::default(),
        sealed_probes: Default::default(),
    };
    backend.put_manifest(key, &manifest).expect("seed determinant manifest");
}

fn di() -> DiscoveredInputs {
    DiscoveredInputs {
        from: "deps.d".to_string(),
        format: "make".to_string(),
    }
}

/// Scenario 1: warm revert with content-dependent output names.
///
/// The producer's ORIGINAL build (main.src + header.h at original content)
/// published `build/chunk-orig.txt` (+ implicit depfile) under full key K1,
/// with K1's determinant manifest recording that concrete name. The consumer
/// is in the post-revert state: original content on disk, but its caller
/// output list is the EDITED run's stale concrete name. The manifest-driven
/// restore must serve K1's real outputs and report them in the outcome.
#[test]
fn revert_restores_despite_stale_caller_output_names() {
    let store = tempfile::tempdir().expect("store");
    let backend = LocalBackend::new(store.path().to_path_buf());

    let declared = vec![xxh(MAIN_SRC)];
    let declared_key = key_for(&mut declared.clone());
    let mut full = vec![xxh(MAIN_SRC), xxh(HEADER_ORIG)];
    let full_key = key_for(&mut full);

    // Multi-set manifest: one known discovered set.
    put_json_artifact(
        &backend,
        &declared_key,
        DISCOVERED_INPUT_SETS_INDEX,
        DISCOVERED_INPUT_SETS_PATH,
        &serde_json::to_vec(&vec![vec!["header.h".to_string()]]).unwrap(),
    );
    // Artifacts of the ORIGINAL build under the full key: file, then depfile.
    put_artifact(&backend, &full_key, 0, "build/chunk-orig.txt", b"ORIG");
    put_artifact(&backend, &full_key, 1, "deps.d", DEPFILE_BYTES);
    put_determinant_manifest(&backend, &full_key, vec!["build/chunk-orig.txt".to_string()]);

    // Consumer tree in the reverted state.
    let wd_dir = tempfile::tempdir().expect("wd");
    let wd = wd_dir.path();
    std::fs::write(wd.join("main.src"), MAIN_SRC).unwrap();
    std::fs::write(wd.join("header.h"), HEADER_ORIG).unwrap();

    let ctx = RestoreCtx { backend: &backend, recipe_namespace: RECIPE_NS };
    let outcome = fetch_by_key(
        &ctx,
        CMD_HASH,
        ENV_CONTRIB,
        SEAL_CONTRIB,
        &declared,
        // Stale caller list: the EDITED run's concrete chunk name.
        &["build/chunk-edited.txt", "deps.d"],
        wd,
        Some(&di()),
    )
    .expect("revert must restore via the candidate key's manifest");

    assert_eq!(
        outcome.restored_outputs,
        vec!["build/chunk-orig.txt".to_string(), "deps.d".to_string()],
        "restored list must come from the candidate key's manifest, not the stale caller list",
    );
    assert_eq!(outcome.discovered_paths, vec!["header.h".to_string()]);
    assert_eq!(std::fs::read(wd.join("build/chunk-orig.txt")).unwrap(), b"ORIG");
    assert_eq!(std::fs::read(wd.join("deps.d")).unwrap(), DEPFILE_BYTES);
    assert!(
        !wd.join("build/chunk-edited.txt").exists(),
        "the stale name must not be conjured",
    );
}

/// Scenario 2: the discovered SET changed between builds. The newest
/// candidate set re-hashes cleanly but composes a key with no artifacts
/// (that build had different input content); the older set composes K1 and
/// hits. Pre-COOK-278 the older set was erased by last-writer-wins.
#[test]
fn revert_restores_older_discovered_set() {
    let store = tempfile::tempdir().expect("store");
    let backend = LocalBackend::new(store.path().to_path_buf());

    let header_b: &[u8] = b"header b\n";
    let declared = vec![xxh(MAIN_SRC)];
    let declared_key = key_for(&mut declared.clone());
    // K1 = the ORIGINAL build: discovered set {header.h} at original content.
    let mut full = vec![xxh(MAIN_SRC), xxh(HEADER_ORIG)];
    let full_key = key_for(&mut full);

    // Sets manifest newest-first: the edited build discovered {header.h,
    // header_b.h}; the original discovered {header.h}.
    put_json_artifact(
        &backend,
        &declared_key,
        DISCOVERED_INPUT_SETS_INDEX,
        DISCOVERED_INPUT_SETS_PATH,
        &serde_json::to_vec(&vec![
            vec!["header.h".to_string(), "header_b.h".to_string()],
            vec!["header.h".to_string()],
        ])
        .unwrap(),
    );
    put_artifact(&backend, &full_key, 0, "out.txt", b"ORIG");
    put_artifact(&backend, &full_key, 1, "deps.d", DEPFILE_BYTES);
    put_determinant_manifest(&backend, &full_key, vec!["out.txt".to_string()]);

    // Consumer tree: reverted header.h; header_b.h still on disk (its content
    // hashes fine, but the composed key has no artifacts → natural miss).
    let wd_dir = tempfile::tempdir().expect("wd");
    let wd = wd_dir.path();
    std::fs::write(wd.join("main.src"), MAIN_SRC).unwrap();
    std::fs::write(wd.join("header.h"), HEADER_ORIG).unwrap();
    std::fs::write(wd.join("header_b.h"), header_b).unwrap();

    let ctx = RestoreCtx { backend: &backend, recipe_namespace: RECIPE_NS };
    let outcome = fetch_by_key(
        &ctx,
        CMD_HASH,
        ENV_CONTRIB,
        SEAL_CONTRIB,
        &declared,
        &["out.txt", "deps.d"],
        wd,
        Some(&di()),
    )
    .expect("older discovered set must be tried after the newest one misses");

    assert_eq!(outcome.discovered_paths, vec!["header.h".to_string()]);
    assert_eq!(std::fs::read(wd.join("out.txt")).unwrap(), b"ORIG");
}

/// Format compatibility: a store written by a pre-COOK-278 binary has only
/// the v1 single-set manifest. `read_discovered_input_sets` must surface it
/// as the sole candidate, and `fetch_by_key` must recover through it.
#[test]
fn v1_single_set_manifest_still_recovers() {
    let store = tempfile::tempdir().expect("store");
    let backend = LocalBackend::new(store.path().to_path_buf());

    let declared = vec![xxh(MAIN_SRC)];
    let declared_key = key_for(&mut declared.clone());
    let mut full = vec![xxh(MAIN_SRC), xxh(HEADER_ORIG)];
    let full_key = key_for(&mut full);

    // v1 manifest ONLY (old producer): a flat path list.
    put_json_artifact(
        &backend,
        &declared_key,
        DISCOVERED_INPUTS_MANIFEST_INDEX,
        DISCOVERED_INPUTS_MANIFEST_PATH,
        &serde_json::to_vec(&vec!["header.h".to_string()]).unwrap(),
    );
    put_artifact(&backend, &full_key, 0, "out.txt", b"ORIG");
    put_artifact(&backend, &full_key, 1, "deps.d", DEPFILE_BYTES);

    assert_eq!(
        read_discovered_input_sets(&backend, &declared_key),
        vec![vec!["header.h".to_string()]],
        "v1 fallback must surface the single set",
    );

    let wd_dir = tempfile::tempdir().expect("wd");
    let wd = wd_dir.path();
    std::fs::write(wd.join("main.src"), MAIN_SRC).unwrap();
    std::fs::write(wd.join("header.h"), HEADER_ORIG).unwrap();

    let ctx = RestoreCtx { backend: &backend, recipe_namespace: RECIPE_NS };
    // No determinant manifest seeded → falls back to the caller's output
    // list, exactly the pre-COOK-278 behaviour.
    let outcome = fetch_by_key(
        &ctx,
        CMD_HASH,
        ENV_CONTRIB,
        SEAL_CONTRIB,
        &declared,
        &["out.txt", "deps.d"],
        wd,
        Some(&di()),
    )
    .expect("v1 store must keep working");
    assert_eq!(outcome.restored_outputs, vec!["out.txt".to_string(), "deps.d".to_string()]);
    assert_eq!(std::fs::read(wd.join("out.txt")).unwrap(), b"ORIG");
}
