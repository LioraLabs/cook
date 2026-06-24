//! Restore-on-hit interaction with depfile-as-implicit-output.
//! Setup: a fat entry exists; backend has the .o and .d artifacts.
//! Disk state: both .o and .d are missing.
//! Expectation: needs_rebuild_cook with restore_ctx restores both
//! files and returns Skip.

use cook_cache::{
    backend::{
        artifact_key, cloud_key, put_bytes, ArtifactMeta, CloudKeyInputs, LocalBackend,
    },
    parse_make_depfile,
    store::{FileRecord, StepEntry, CACHE_VERSION},
};
use cook_contracts::DiscoveredInputs;
use cook_fingerprint::{install_depfile_parser, needs_rebuild_cook, RebuildResult, RestoreCtx};
use std::sync::Once;

fn install_parser_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        install_depfile_parser(|p, src, wd, fmt| {
            if fmt != "make" { return Err(()); }
            parse_make_depfile(p, src, wd).map_err(|_| ())
        });
    });
}

#[test]
fn missing_outputs_and_depfile_are_both_restored() {
    install_parser_once();

    let wd_dir = tempfile::tempdir().expect("wd");
    let wd = wd_dir.path();
    let backend_dir = tempfile::tempdir().expect("backend");
    let backend = LocalBackend::new(backend_dir.path().to_path_buf());

    // Lay out source + header + (initially) .o + .d.
    std::fs::write(wd.join("a.c"), b"src").expect("a.c");
    std::fs::write(wd.join("a.h"), b"hdr").expect("a.h");
    std::fs::write(wd.join("a.o"), b"obj-bytes").expect("a.o");
    std::fs::create_dir_all(wd.join(".cook/deps")).expect("deps");
    std::fs::write(wd.join(".cook/deps/a.d"), b"a.o: a.c a.h\n").expect("d");

    let recipe_namespace = "p/Cookfile::r".to_string();
    let entry = StepEntry {
        inputs: vec![
            FileRecord {
                path: "a.c".into(),
                mtime: 0,
                hash: cook_fingerprint::hash_file(&wd.join("a.c")).unwrap(),
            },
            FileRecord {
                path: "a.h".into(),
                mtime: 0,
                hash: cook_fingerprint::hash_file(&wd.join("a.h")).unwrap(),
            },
        ],
        outputs: vec![
            FileRecord {
                path: "a.o".into(),
                mtime: cook_fingerprint::stat_mtime(&wd.join("a.o")).unwrap_or(0),
                hash: cook_fingerprint::hash_file(&wd.join("a.o")).unwrap(),
            },
            FileRecord {
                path: ".cook/deps/a.d".into(),
                mtime: cook_fingerprint::stat_mtime(&wd.join(".cook/deps/a.d")).unwrap_or(0),
                hash: cook_fingerprint::hash_file(&wd.join(".cook/deps/a.d")).unwrap(),
            },
        ],
        command_hash: 0xc0de,
        env_contribution: 0,
        seal_contribution: 0,
    };

    // Compose cloud_key from the fat input set.
    let mut sorted: Vec<u64> = entry.inputs.iter().map(|fr| fr.hash).collect();
    sorted.sort();
    let cloud_k = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: &recipe_namespace,
        command_hash: 0xc0de,
        env_contribution: 0,
        seal_contribution: 0,
        sorted_input_content_hashes: &sorted,
    });

    // Pre-populate backend with both artifacts.
    let obj_bytes = std::fs::read(wd.join("a.o")).unwrap();
    let dep_bytes = std::fs::read(wd.join(".cook/deps/a.d")).unwrap();
    let obj_k = artifact_key(&cloud_k, 0, "a.o");
    let dep_k = artifact_key(&cloud_k, 1, ".cook/deps/a.d");
    let mk_meta = |idx: u32, path: &str, size: u64| ArtifactMeta {
        recipe_namespace: recipe_namespace.clone(),
        command_hash: 0xc0de,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: CACHE_VERSION,
        size_bytes: size,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: idx,
        output_path: path.to_string(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: None,
    };
    let mut obj_meta = mk_meta(0, "a.o", obj_bytes.len() as u64);
    let mut dep_meta = mk_meta(1, ".cook/deps/a.d", dep_bytes.len() as u64);
    put_bytes(&backend, &obj_k, &obj_bytes, &mut obj_meta).expect("put obj");
    put_bytes(&backend, &dep_k, &dep_bytes, &mut dep_meta).expect("put dep");

    // Wipe .o and .d from the working tree to simulate a partial clean.
    std::fs::remove_file(wd.join("a.o")).expect("rm o");
    std::fs::remove_file(wd.join(".cook/deps/a.d")).expect("rm d");

    let restore_ctx = RestoreCtx {
        backend: &backend,
        recipe_namespace: &recipe_namespace,
    };
    let di = DiscoveredInputs {
        from: ".cook/deps/a.d".into(),
        format: "make".into(),
    };

    let (result, _) = needs_rebuild_cook(
        Some(&entry),
        &["a.c"],
        &["a.o"],
        0xc0de,
        0,
        0,
        wd,
        Some(&restore_ctx),
        Some(&di),
        false,
    );

    assert!(matches!(result, RebuildResult::Skip),
        "expected Skip after restoring both output and depfile; got {result:?}");
    assert!(wd.join("a.o").exists(), "a.o restored");
    assert!(wd.join(".cook/deps/a.d").exists(), ".cook/deps/a.d restored");
}
