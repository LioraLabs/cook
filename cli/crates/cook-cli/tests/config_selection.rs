//! R3 (COOK-307 / CS-0165): `@PRESET` selection is a single per-run global
//! validated against the UNION of the named config blocks of every loaded
//! Cookfile — not just the entry Cookfile.
//!
//! Before CS-0165 `validate_selected_config` checked the entry Cookfile only,
//! so `cook sub.<recipe> @profiling` was rejected ("unknown config
//! 'profiling': no named configs defined") even though `sub/Cookfile` declared
//! `profiling` and the selected name already propagated to every member's
//! builder. Under R3:
//!   * a name declared in ANY loaded Cookfile passes validation;
//!   * a name in NO Cookfile is a hard error listing the union of names;
//!   * a member lacking the selected overlay runs its base config (no-op).

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

/// Root imports `./sub`. `sub` carries a base + a `profiling` overlay that
/// changes `var.MODE`; root carries a `release` overlay declared NOWHERE in
/// `sub`. This lets one fixture exercise all three R3 behaviours.
const ROOT_COOKFILE: &str = r#"import sub ./sub

config release
    var.RTAG = "rel"
"#;

const SUB_COOKFILE: &str = r#"config
    var.MODE = "base"

config profiling
    var.MODE = "prof"

recipe show
    cook "out/mode.txt" { printf '%s' "$<MODE>" > $<out> }
"#;

/// Tempdir workspace (root + `sub` import) with the shared cache pinned to a
/// private subdir and publishing off, so the test never touches host state.
fn init() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join(".cook")).unwrap();
    let shared = dir.path().join(".cook/shared-cache");
    fs::write(
        dir.path().join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", shared.to_string_lossy()),
    )
    .unwrap();
    fs::write(dir.path().join("Cookfile"), ROOT_COOKFILE).unwrap();
    fs::create_dir_all(dir.path().join("out")).unwrap();
    fs::create_dir_all(dir.path().join("sub/out")).unwrap();
    fs::write(dir.path().join("sub/Cookfile"), SUB_COOKFILE).unwrap();
    dir
}

fn run(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_bin())
        .current_dir(dir)
        .env("COOK_NO_PUBLISH", "1")
        .args(args)
        .output()
        .unwrap()
}

/// R3 core: a preset declared only in an IMPORTED Cookfile passes validation
/// and its overlay actually applies to that import.
#[test]
fn preset_from_imported_cookfile_selects_its_overlay() {
    let dir = init();
    let out = run(dir.path(), &["sub.show", "@profiling"]);
    assert!(
        out.status.success(),
        "cook sub.show @profiling must succeed (preset declared in the import):\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("sub/out/mode.txt")).unwrap(),
        "prof",
        "the imported member's profiling overlay must apply"
    );
}

/// A preset declared NOWHERE errors, listing the union of available names
/// across every loaded Cookfile (here `profiling` from sub and `release` from
/// root).
#[test]
fn unknown_preset_errors_listing_the_union() {
    let dir = init();
    let out = run(dir.path(), &["sub.show", "@nope"]);
    assert!(
        !out.status.success(),
        "an undeclared preset must be a hard error"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("profiling") && err.contains("release"),
        "the error must list the UNION of available names across all Cookfiles; got:\n{err}"
    );
}

/// A member that does not declare the selected overlay runs its base config —
/// a silent no-op. `release` is declared only in root, so `sub` no-ops to its
/// base `MODE = base` while the run still succeeds.
#[test]
fn member_without_the_overlay_runs_its_base() {
    let dir = init();
    let out = run(dir.path(), &["sub.show", "@release"]);
    assert!(
        out.status.success(),
        "a root-declared preset is valid for a sub target (union); sub no-ops to base:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("sub/out/mode.txt")).unwrap(),
        "base",
        "sub lacks a `release` overlay, so it runs its base config"
    );
}
