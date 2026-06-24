//! CS-0085: globbed entries in outputs[] are resolved POST-EXECUTE by the
//! engine into a concrete file list before record_completion sees them.
//! This test pins the cook-cache contract: record_completion treats every
//! meta.output_paths entry as a literal file path. The engine is
//! responsible for resolving any glob patterns before this layer.

use std::fs;
use std::path::PathBuf;

use cook_cache::manager::ThreadSafeCacheManager;
use cook_contracts::CacheMeta;

fn make_meta(output_paths: Vec<String>) -> CacheMeta {
    CacheMeta {
        recipe_name: "rec".into(),
        project_id: String::new(),
        cookfile_path: String::new(),
        cache_key: "step".into(),
        input_paths: vec!["src.c".into()],
        output_paths,
        command_hash: 0xC0DE,
        env_contribution: 0,
        consulted_env: std::collections::BTreeMap::new(),
        discovered_inputs: None,
        seal_keys: Default::default(),
    }
}

#[test]
fn record_completion_with_resolved_glob_produces_concrete_step_entry() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    fs::write(wd.join("src.c"), b"src").unwrap();
    fs::create_dir_all(wd.join("build")).unwrap();
    fs::write(wd.join("build/a.o"), b"a").unwrap();
    fs::write(wd.join("build/b.o"), b"b").unwrap();
    fs::write(wd.join("build/c.o"), b"c").unwrap();

    // "build/**/*" matches files recursively including direct children.
    // Note: glob 0.3 does NOT match files with "build/**" alone (zero-length
    // ** tail only matches the directory itself). The canonical user-facing
    // pattern "build/**" is normalised to "build/**/*" by the engine's
    // normalize_glob_pattern() helper (cook-engine/src/executor.rs, CS-0085
    // trailing-** fix). This test exercises cook-cache's record_completion
    // contract at the fingerprint layer (below the normalisation), so it
    // intentionally uses the already-normalised "build/**/*" form. The
    // engine-level normalisation is tested by unit tests in executor.rs.
    let resolved: Vec<String> = cook_fingerprint::resolve_glob(wd, "build/**/*")
        .into_iter()
        .collect();
    assert_eq!(resolved.len(), 3, "glob resolves to three files");

    let cache_dir: PathBuf = wd.join(".cook/cache");
    fs::create_dir_all(&cache_dir).unwrap();
    let cm = ThreadSafeCacheManager::new(cache_dir);

    let meta = make_meta(resolved);
    let entry = cm.record_completion("rec", "step", &meta, wd).expect("record");
    cm.flush_all().expect("flush");

    assert_eq!(entry.outputs.len(), 3, "StepEntry records three outputs");
    let mut paths: Vec<&str> = entry.outputs.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["build/a.o", "build/b.o", "build/c.o"]);
}
