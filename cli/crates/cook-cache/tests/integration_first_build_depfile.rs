//! AC-Integ.1: Two-machine first-build depfile fixture.
//!
//! Machine A (no .d file → inputs = [source] only) records a StepEntry
//! and uploads. Machine B (also no .d file, fresh checkout) pulls.
//! The cache hit is correct because input *content* matches; B's
//! subsequent builds generate a depfile and pick up header changes.

use cook_cache::backend::{
    cloud_key, get_bytes, put_bytes, ArtifactMeta, CloudKeyInputs, LocalBackend,
};
use cook_cache::store::{FileRecord, StepEntry, CACHE_VERSION};

fn make_step_with_thin_inputs(source_path: &str, source_hash: u64) -> StepEntry {
    StepEntry {
        inputs: vec![FileRecord {
            path: source_path.to_string(),
            mtime: 1700000000,
            hash: source_hash,
        }],
        outputs: vec![FileRecord {
            path: "build/main.o".to_string(),
            mtime: 1700000100,
            hash: 0xabcd_efab_cdef_abcd,
        }],
        command_hash: 0x1111,
        env_contribution: 0x3333,
    }
}

#[test]
fn machine_a_uploads_thin_entry_machine_b_pulls_correctly() {
    // Shared "cloud" backend used by both machines.
    let shared_dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(shared_dir.path().to_path_buf());

    // Machine A: build with no depfile, inputs=[source] only.
    let entry_a = make_step_with_thin_inputs("src/main.c", 0xc01dcafe);

    // Compose the cloud key. Machine A's namespace and key inputs.
    let mut sorted_hashes: Vec<u64> = entry_a.inputs.iter().map(|fr| fr.hash).collect();
    sorted_hashes.sort();
    let inputs_for_key = CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::build",
        command_hash: entry_a.command_hash,
        env_contribution: entry_a.env_contribution,
        sorted_input_content_hashes: &sorted_hashes,
    };
    let key = cloud_key(&inputs_for_key);

    // Machine A uploads the artifact bytes (object file contents).
    let artifact_bytes: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
    let mut meta = ArtifactMeta {
        recipe_namespace: "myproj/Cookfile::build".into(),
        command_hash: entry_a.command_hash,
        env_contribution: entry_a.env_contribution,
        schema_version: CACHE_VERSION,
        size_bytes: artifact_bytes.len() as u64,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: 0,
        output_path: "build/main.o".into(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: None,
    };
    put_bytes(&backend, &key, &artifact_bytes, &mut meta).expect("put");

    // Machine B: fresh checkout, no .d file. Same source content.
    // Its key composition produces the same key.
    let entry_b = make_step_with_thin_inputs("src/main.c", 0xc01dcafe);
    let mut sorted_b: Vec<u64> = entry_b.inputs.iter().map(|fr| fr.hash).collect();
    sorted_b.sort();
    let inputs_for_key_b = CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::build",
        command_hash: entry_b.command_hash,
        env_contribution: entry_b.env_contribution,
        sorted_input_content_hashes: &sorted_b,
    };
    let key_b = cloud_key(&inputs_for_key_b);
    assert_eq!(key, key_b, "same content → same cloud_key across machines");

    // Machine B pulls.
    let bytes_b = get_bytes(&backend, &key_b).expect("get").expect("hit");
    assert_eq!(bytes_b, artifact_bytes, "B receives A's bytes");
}

#[test]
fn header_change_after_pull_invalidates_correctly() {
    // After Machine B pulls A's thin-input entry and runs its first build,
    // B's *next* build SHOULD generate a depfile and pick up header changes.
    // This test verifies that a build with a fattened input set (source +
    // header) produces a different cloud_key from the thin-input one — i.e.,
    // there is no false hit on the second build.

    let entry_thin = make_step_with_thin_inputs("src/main.c", 0xc01dcafe);

    let entry_with_header = StepEntry {
        inputs: vec![
            FileRecord {
                path: "src/main.c".to_string(),
                mtime: 1700000000,
                hash: 0xc01dcafe,
            },
            FileRecord {
                path: "include/widget.h".to_string(),
                mtime: 1700000050,
                hash: 0xdeadbeef,
            },
        ],
        outputs: entry_thin.outputs.clone(),
        command_hash: entry_thin.command_hash,
        env_contribution: entry_thin.env_contribution,
    };

    let mut h_thin: Vec<u64> = entry_thin.inputs.iter().map(|fr| fr.hash).collect();
    h_thin.sort();
    let mut h_fat: Vec<u64> = entry_with_header.inputs.iter().map(|fr| fr.hash).collect();
    h_fat.sort();

    let key_thin = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::build",
        command_hash: entry_thin.command_hash,
        env_contribution: entry_thin.env_contribution,
        sorted_input_content_hashes: &h_thin,
    });
    let key_fat = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::build",
        command_hash: entry_with_header.command_hash,
        env_contribution: entry_with_header.env_contribution,
        sorted_input_content_hashes: &h_fat,
    });

    assert_ne!(key_thin, key_fat, "fattened input set → different cloud_key");
}
