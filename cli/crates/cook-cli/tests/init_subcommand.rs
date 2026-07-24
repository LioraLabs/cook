//! Integration tests for `cook init` (the starter-project scaffold).
//!
//! `cmd_init` (cli/crates/cook-cli/src/pipeline.rs) writes a deliberately
//! minimal starter Cookfile — one `build` recipe that produces a single
//! cached artifact (`build/hello.txt`) plus a `clean` chore — and merges a
//! Cook-managed `.gitignore` section. It seeds no sample inputs, and it
//! refuses to overwrite an existing Cookfile.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

/// Isolate the shared artifact store so this test never touches
/// ~/.cache/cook/cloud.
fn write_isolated_cache_config(root: &Path, cache_dir: &Path) {
    std::fs::create_dir_all(root.join(".cook")).unwrap();
    std::fs::write(
        root.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

fn run_cook(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_bin())
        .args(args)
        .current_dir(dir)
        .env("COOK_NO_PUBLISH", "1")
        .output()
        .expect("invoke cook")
}

#[test]
fn init_scaffolds_builds_and_cleans() {
    let tmp = TempDir::new().expect("tempdir");

    let init_out = run_cook(tmp.path(), &["init"]);
    assert!(
        init_out.status.success(),
        "cook init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&init_out.stdout),
        String::from_utf8_lossy(&init_out.stderr),
    );

    assert!(tmp.path().join("Cookfile").exists(), "Cookfile must be created");

    // Cache-isolate before running the default build so the test never
    // touches the shared artifact store.
    let cache_dir = TempDir::new().expect("cache tempdir");
    write_isolated_cache_config(tmp.path(), cache_dir.path());

    let build_out = run_cook(tmp.path(), &[]);
    assert!(
        build_out.status.success(),
        "default `cook` build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&build_out.stdout),
        String::from_utf8_lossy(&build_out.stderr),
    );

    let artifact = tmp.path().join("build/hello.txt");
    assert!(
        artifact.exists(),
        "build/hello.txt must be produced by the starter build"
    );
    assert_eq!(
        std::fs::read_to_string(&artifact).unwrap().trim(),
        "built with cook",
        "the starter artifact should say 'built with cook'"
    );

    // The `clean` chore removes the build output.
    let clean_out = run_cook(tmp.path(), &["clean"]);
    assert!(
        clean_out.status.success(),
        "cook clean failed: stdout={} stderr={}",
        String::from_utf8_lossy(&clean_out.stdout),
        String::from_utf8_lossy(&clean_out.stderr),
    );
    assert!(
        !tmp.path().join("build").exists(),
        "cook clean must remove build/"
    );
}

/// `cook init` must never clobber an existing Cookfile: it errors out and
/// leaves the file untouched.
#[test]
fn init_refuses_to_overwrite_existing_cookfile() {
    let tmp = TempDir::new().expect("tempdir");
    let sentinel = "recipe mine\n    cook \"o.txt\" { echo hi > $<out> }\n";
    std::fs::write(tmp.path().join("Cookfile"), sentinel).unwrap();

    let out = run_cook(tmp.path(), &["init"]);
    assert!(
        !out.status.success(),
        "cook init must fail when a Cookfile already exists"
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("Cookfile")).unwrap(),
        sentinel,
        "cook init must not overwrite an existing Cookfile"
    );
}
