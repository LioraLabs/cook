//! Integration tests for `cook init` (the starter-project scaffold).
//!
//! `cmd_init` (cli/crates/cook-cli/src/pipeline.rs) writes a starter
//! Cookfile plus `notes/one.md` / `notes/two.md` sample ingredients, then
//! merges a Cook-managed `.gitignore` section. It guards the notes seeding
//! on `if !notes.exists()` — so a pre-existing `notes/` dir (even with just
//! one file in it) short-circuits ALL seeding, not just the missing file.

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
fn init_seeds_starter_project_and_builds_clean() {
    let tmp = TempDir::new().expect("tempdir");

    let init_out = run_cook(tmp.path(), &["init"]);
    assert!(
        init_out.status.success(),
        "cook init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&init_out.stdout),
        String::from_utf8_lossy(&init_out.stderr),
    );

    assert!(tmp.path().join("Cookfile").exists(), "Cookfile must be created");
    assert!(
        tmp.path().join("notes/one.md").exists(),
        "notes/one.md must be seeded"
    );
    assert!(
        tmp.path().join("notes/two.md").exists(),
        "notes/two.md must be seeded"
    );

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

    assert!(
        tmp.path().join("out/one.html").exists(),
        "out/one.html must be produced by the starter build"
    );
    assert!(
        tmp.path().join("out/two.html").exists(),
        "out/two.html must be produced by the starter build"
    );
}

/// COOK-init no-clobber: if `notes/` already exists (even partially
/// populated), `cook init` must not overwrite or touch it. `cmd_init`
/// guards the entire seeding block on `!notes.exists()`, so a pre-existing
/// notes/ dir means NO seeding happens at all (not even for missing files).
#[test]
fn init_does_not_clobber_pre_existing_notes_dir() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("notes")).unwrap();
    let sentinel = "# My Own Note\n- do not overwrite me\n";
    std::fs::write(tmp.path().join("notes/one.md"), sentinel).unwrap();

    let out = run_cook(tmp.path(), &["init"]);
    assert!(
        out.status.success(),
        "cook init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let contents = std::fs::read_to_string(tmp.path().join("notes/one.md")).unwrap();
    assert_eq!(
        contents, sentinel,
        "cook init must not overwrite an existing notes/one.md"
    );

    // The guard is on the whole notes/ dir existing, not per-file: two.md
    // must NOT have been seeded either, since notes/ already existed.
    assert!(
        !tmp.path().join("notes/two.md").exists(),
        "cook init must not seed notes/two.md when notes/ already existed"
    );
}
