//! Cold fetch-by-key shares depfile (discovered-inputs) units across a fleet.
//!
//! Simulates a producer that compiled `src/main.c` and discovered `src/dep.h`
//! via a depfile, published both the real output artifact (keyed by the FULL
//! declared+discovered hash set) and the discovered-inputs manifest (keyed by
//! the declared-only hash set). Then simulates a fresh consumer — no local
//! StepEntry — that calls `fetch_by_key`, which uses the manifest to
//! reconstruct the full key and materialises `build/main.o`.
//!
//! Test 1 (HIT): consumer has identical `src/dep.h` bytes → full key matches
//!               → output restored.
//! Test 2 (safe miss): consumer's `src/dep.h` differs → full key diverges →
//!               `fetch_by_key` returns false; no output written.

use cook_cache::backend::{
    artifact_key, cloud_key, put_bytes, ArtifactMeta, CloudKeyInputs, DeterminantManifest, LocalBackend,
};
use cook_cache::store::CACHE_VERSION;
use cook_contracts::DiscoveredInputs;
use cook_cache::{
    CacheBackend, fetch_by_key, RestoreCtx, DISCOVERED_INPUTS_MANIFEST_INDEX, DISCOVERED_INPUTS_MANIFEST_PATH,
};

// ── Shared constants ─────────────────────────────────────────────────────────

/// Bytes the producer compiled.  Consumer must have the same file.
const MAIN_C_BYTES: &[u8] = b"int main() { return 0; }";

/// Header content the PRODUCER built against (and discovered via depfile).
const DEP_H_BYTES_PRODUCER: &[u8] = b"#pragma once\n#define ANSWER 42\n";

const RECIPE_NS: &str = "proj/Cookfile::compile_main";
const CMD_HASH: u64 = 0x0C;
const ENV_CONTRIB: u64 = 0x0E;
const SEAL_CONTRIB: u64 = 0x05;

// ── Helper: seed the fleet cache exactly as a producer would ─────────────────

/// Pre-populate the shared backend with:
///   - the discovered-inputs manifest (declared-key scope)
///   - the real output artifact (full-key scope, keyed against `dep_h_bytes`)
///
/// `dep_h_bytes` is the dep.h content the producer had, so the caller can
/// also seed with different bytes to test divergence.
fn seed_backend(backend: &LocalBackend, dep_h_bytes: &[u8]) {
    // Declared input hash: src/main.c only.
    let declared_hash = xxhash_rust::xxh3::xxh3_64(MAIN_C_BYTES);
    let mut declared_hashes = vec![declared_hash];
    declared_hashes.sort();

    // Full hash set: declared ++ discovered (src/dep.h), sorted.
    let discovered_hash = xxhash_rust::xxh3::xxh3_64(dep_h_bytes);
    let mut full_hashes = declared_hashes.clone();
    full_hashes.push(discovered_hash);
    full_hashes.sort();

    // Cloud keys.
    let declared_key = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: RECIPE_NS,
        command_hash: CMD_HASH,
        env_contribution: ENV_CONTRIB,
        seal_contribution: SEAL_CONTRIB,
        sorted_input_content_hashes: &declared_hashes,
    });
    let full_key = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: RECIPE_NS,
        command_hash: CMD_HASH,
        env_contribution: ENV_CONTRIB,
        seal_contribution: SEAL_CONTRIB,
        sorted_input_content_hashes: &full_hashes,
    });

    // 1. Discovered-inputs manifest under the declared-only key.
    let manifest_bytes =
        serde_json::to_vec(&vec!["src/dep.h".to_string()]).expect("manifest serialise");
    let manifest_k = artifact_key(
        &declared_key,
        DISCOVERED_INPUTS_MANIFEST_INDEX,
        DISCOVERED_INPUTS_MANIFEST_PATH,
    );
    let mut manifest_meta = ArtifactMeta {
        recipe_namespace: RECIPE_NS.to_string(),
        command_hash: CMD_HASH,
        env_contribution: ENV_CONTRIB,
        seal_contribution: SEAL_CONTRIB,
        schema_version: CACHE_VERSION,
        size_bytes: manifest_bytes.len() as u64,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: DISCOVERED_INPUTS_MANIFEST_INDEX,
        output_path: DISCOVERED_INPUTS_MANIFEST_PATH.to_string(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: Some("discovered_inputs".to_string()),
        mode: ArtifactMeta::default_mode(),
        target: None,
    };
    put_bytes(backend, &manifest_k, &manifest_bytes, &mut manifest_meta)
        .expect("seed manifest");

    // 2. Real output artifact under the full key.
    let obj_bytes: &[u8] = b"OBJ";
    let obj_k = artifact_key(&full_key, 0, "build/main.o");
    let mut obj_meta = ArtifactMeta {
        recipe_namespace: RECIPE_NS.to_string(),
        command_hash: CMD_HASH,
        env_contribution: ENV_CONTRIB,
        seal_contribution: SEAL_CONTRIB,
        schema_version: CACHE_VERSION,
        size_bytes: obj_bytes.len() as u64,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: 0,
        output_path: "build/main.o".to_string(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: None,
        mode: 0o644,
        target: None,
    };
    put_bytes(backend, &obj_k, obj_bytes, &mut obj_meta).expect("seed obj");

    // 3. The depfile, the implicit output after the files, and the full key's
    // determinant manifest that says so: a depfile unit has no other list.
    let mut dep_meta = ArtifactMeta { output_index: 1, output_path: DEPFILE.to_string(), content_hash: ArtifactMeta::zero_content_hash(), ..obj_meta.clone() };
    put_bytes(backend, &artifact_key(&full_key, 1, DEPFILE), b"main.o: src/dep.h\n", &mut dep_meta).expect("seed depfile");
    backend
        .put_manifest(
            &full_key,
            &DeterminantManifest {
                schema_version: CACHE_VERSION,
                recipe_namespace: RECIPE_NS.to_string(),
                key: hex::encode(full_key),
                command_hash: CMD_HASH,
                env_contribution: ENV_CONTRIB,
                seal_contribution: SEAL_CONTRIB,
                inputs: Default::default(),
                output_paths: vec!["build/main.o".to_string()],
                empty_dir_outputs: vec![],
                consulted_env: Default::default(),
                sealed_probes: Default::default(),
                observation: None,
            },
        )
        .expect("seed manifest");
}

const DEPFILE: &str = ".cook/deps/main.d";

fn manifest_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            found.extend(manifest_files(&p));
        } else if p.to_string_lossy().ends_with(".provenance.json") {
            found.push(p);
        }
    }
    found
}

// ── Test 1: HIT via manifest recovery ────────────────────────────────────────

/// A fresh consumer (no StepEntry, no build/ dir) holding the same source
/// files as the producer cold-fetches successfully: `fetch_by_key` finds the
/// manifest, re-hashes the consumer's dep.h (identical bytes), reconstructs
/// the full key, and materialises `build/main.o`.
#[test]
fn cold_fetch_shares_depfile_unit_across_fleet() {
    // Shared fleet store — seeded with the producer's artifacts.
    let store_dir = tempfile::tempdir().expect("store");
    let backend = LocalBackend::new(store_dir.path().to_path_buf());
    seed_backend(&backend, DEP_H_BYTES_PRODUCER);

    // Consumer working dir: fresh machine with the same source files.
    let consumer_dir = tempfile::tempdir().expect("consumer");
    let wd = consumer_dir.path();
    std::fs::create_dir_all(wd.join("src")).expect("src dir");
    std::fs::write(wd.join("src/main.c"), MAIN_C_BYTES).expect("main.c");
    std::fs::write(wd.join("src/dep.h"), DEP_H_BYTES_PRODUCER).expect("dep.h");
    // build/ intentionally absent — simulates a clean consumer with no outputs.

    // Consumer only knows its declared input (src/main.c).
    let declared_hash = xxhash_rust::xxh3::xxh3_64(MAIN_C_BYTES);
    let mut declared_hashes = vec![declared_hash];
    declared_hashes.sort();

    let ctx = RestoreCtx {
        backend: &backend,
        recipe_namespace: RECIPE_NS,
        declared_outputs: &["build/main.o"],
    };

    // `from` path need not exist — `fetch_by_key` only tests `discovered_inputs.is_some()`
    // then uses the manifest, not the depfile directly.
    let di = DiscoveredInputs {
        from: ".cook/deps/main.d".to_string(),
        format: "make".to_string(),
    };

    let hit = fetch_by_key(
        &ctx,
        CMD_HASH,
        ENV_CONTRIB,
        SEAL_CONTRIB,
        &declared_hashes,
        &["build/main.o"],
        wd,
        Some(&di),
        false,
    );

    let outcome = hit.expect("cold fetch should HIT via manifest recovery");
    assert_eq!(
        outcome.discovered_paths,
        vec!["src/dep.h".to_string()],
        "outcome must carry the winning discovered set so the caller can record a FAT entry",
    );
    assert_eq!(
        std::fs::read(wd.join("build/main.o")).expect("read build/main.o"),
        b"OBJ",
        "restored bytes must match what the producer published",
    );
}

// ── Test 2: safe miss when the discovered header differs ─────────────────────

/// The consumer's `src/dep.h` has different bytes than the producer's.
/// `fetch_by_key` fetches the manifest, re-hashes the consumer's dep.h
/// (different hash), folds it into the key → different full key → the output
/// artifact is not found under that key → returns false.
/// `build/main.o` must NOT be created.
#[test]
fn cold_fetch_safe_miss_when_header_differs() {
    // Shared fleet store — seeded with the producer's dep.h hash.
    let store_dir = tempfile::tempdir().expect("store");
    let backend = LocalBackend::new(store_dir.path().to_path_buf());
    seed_backend(&backend, DEP_H_BYTES_PRODUCER);

    // Consumer working dir: same declared input, but a different dep.h.
    let consumer_dir = tempfile::tempdir().expect("consumer");
    let wd = consumer_dir.path();
    std::fs::create_dir_all(wd.join("src")).expect("src dir");
    std::fs::write(wd.join("src/main.c"), MAIN_C_BYTES).expect("main.c");
    // Different header content → different xxh3 hash → different full key.
    std::fs::write(
        wd.join("src/dep.h"),
        b"#pragma once\n#define ANSWER 99\n",
    )
    .expect("dep.h (different)");

    let declared_hash = xxhash_rust::xxh3::xxh3_64(MAIN_C_BYTES);
    let mut declared_hashes = vec![declared_hash];
    declared_hashes.sort();

    let ctx = RestoreCtx {
        backend: &backend,
        recipe_namespace: RECIPE_NS,
        declared_outputs: &["build/main.o"],
    };
    let di = DiscoveredInputs {
        from: ".cook/deps/main.d".to_string(),
        format: "make".to_string(),
    };

    let hit = fetch_by_key(
        &ctx,
        CMD_HASH,
        ENV_CONTRIB,
        SEAL_CONTRIB,
        &declared_hashes,
        &["build/main.o"],
        wd,
        Some(&di),
        false,
    );

    assert!(hit.is_none(), "safe miss: consumer dep.h differs, full key must not match");
    assert!(
        !wd.join("build/main.o").exists(),
        "build/main.o must NOT be created on a safe miss",
    );
}

// ── Test 3: no manifest, no list ─────────────────────────────────────────────

/// The full key's determinant manifest is gone (evicted, never put) but the
/// discovered-input sets and the artifacts remain. The caller's list is the
/// last build's and omits the depfile, so a depfile unit has no list to
/// restore: a miss, nothing written.
#[test]
fn a_depfile_unit_without_its_manifest_is_a_miss() {
    let store_dir = tempfile::tempdir().expect("store");
    let backend = LocalBackend::new(store_dir.path().to_path_buf());
    seed_backend(&backend, DEP_H_BYTES_PRODUCER);
    let manifests = manifest_files(store_dir.path());
    assert_eq!(manifests.len(), 1, "only the full key has a manifest");
    std::fs::remove_file(&manifests[0]).unwrap();

    let consumer_dir = tempfile::tempdir().expect("consumer");
    let wd = consumer_dir.path();
    std::fs::create_dir_all(wd.join("src")).unwrap();
    std::fs::write(wd.join("src/main.c"), MAIN_C_BYTES).unwrap();
    std::fs::write(wd.join("src/dep.h"), DEP_H_BYTES_PRODUCER).unwrap();
    let ctx = RestoreCtx { backend: &backend, recipe_namespace: RECIPE_NS, declared_outputs: &["build/main.o"] };
    let di = DiscoveredInputs { from: DEPFILE.to_string(), format: "make".to_string() };
    let hashes = vec![xxhash_rust::xxh3::xxh3_64(MAIN_C_BYTES)];
    let got = fetch_by_key(&ctx, CMD_HASH, ENV_CONTRIB, SEAL_CONTRIB, &hashes, &["build/main.o"], wd, Some(&di), false);
    assert!(got.is_none());
    assert!(!wd.join("build/main.o").exists());
}
