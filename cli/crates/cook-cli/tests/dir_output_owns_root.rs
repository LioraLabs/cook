//! A directory output naming the unit's own directory, an ancestor, a path
//! outside it, or a pattern is refused at registration, before anything runs
//! (CS-0274).
//!
//! Only spellings whose worst case stays inside the temp dir are built here:
//! the project sits two levels deep, so even with the refusal gone `../` sweeps
//! temp siblings, never the host. `/` and absolute forms are covered by the
//! pure tests in `pathlaw` and `cook-cache`.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn cook(root: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cook"))
        .args(args)
        .current_dir(root)
        .env("XDG_CACHE_HOME", cache)
        .output()
        .expect("run cook")
}

/// `<tmp>/a/proj` holding a Cookfile and a `keep.txt`, with `<tmp>/a/sibling.txt` beside it.
fn project(output: &str) -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("a/proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(tmp.path().join("a/sibling.txt"), "s").unwrap();
    std::fs::write(proj.join("keep.txt"), "k").unwrap();
    std::fs::create_dir_all(proj.join("src/lib")).unwrap();
    std::fs::write(proj.join("src/lib/a.c"), "a").unwrap();
    std::fs::write(
        proj.join("Cookfile"),
        format!("recipe build\n    cook \"{output}\" {{ mkdir -p dist out && touch .env dist/a out/a }}\n"),
    )
    .unwrap();
    (tmp, proj)
}

#[test]
fn a_directory_output_owning_the_unit_root_is_refused_before_anything_runs() {
    for output in ["./", "x/../", "../"] {
        let (tmp, proj) = project(output);
        let out = cook(&proj, &tmp.path().join("cache"), &["build"]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{output}: {err}");
        assert!(
            err.contains(&format!("output \"{output}\" (recipe build) is a directory output covering the unit's own directory")),
            "{output}: {err}"
        );
        assert!(!proj.join(".env").exists(), "{output}: the command ran");
        assert!(proj.join("Cookfile").is_file(), "{output}: Cookfile deleted");
        assert!(proj.join("keep.txt").is_file(), "{output}: keep.txt deleted");
        assert!(tmp.path().join("a/sibling.txt").is_file(), "{output}: sibling deleted");
    }
}

#[test]
fn a_subdirectory_output_still_builds() {
    for output in ["dist/", "out/", "./dist/"] {
        let (tmp, proj) = project(output);
        let out = cook(&proj, &tmp.path().join("cache"), &["build"]);
        assert!(out.status.success(), "{output}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(proj.join("Cookfile").is_file() && proj.join("keep.txt").is_file(), "{output}");
    }
}

#[test]
fn a_pattern_directory_output_is_refused_before_anything_runs() {
    for output in ["**/", "*/", "src/*/", "src/?ib/", "src/[l]ib/"] {
        let (tmp, proj) = project(output);
        let out = cook(&proj, &tmp.path().join("cache"), &["build"]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{output}: {err}");
        assert!(
            err.contains(&format!("output \"{output}\" (recipe build) is a directory output with a wildcard in its path")),
            "{output}: {err}"
        );
        assert!(!proj.join(".env").exists(), "{output}: the command ran");
        for f in ["Cookfile", "keep.txt", "src/lib/a.c"] {
            assert!(proj.join(f).is_file(), "{output}: {f} deleted");
        }
    }
}

#[test]
fn a_glob_output_still_builds_and_hits() {
    let (tmp, proj) = project("dist/**");
    for _ in 0..2 {
        let out = cook(&proj, &tmp.path().join("cache"), &["build"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    for f in ["Cookfile", "keep.txt", "src/lib/a.c", "dist/a"] {
        assert!(proj.join(f).is_file(), "{f} deleted");
    }
}

/// An absolute directory output under the unit's working dir is the
/// subdirectory it names (cook_pnpm anchors its outputs as `<root>/./pkg/...`).
#[test]
fn an_absolute_directory_output_under_the_unit_builds() {
    let (tmp, proj) = project("dist/");
    let cookfile = std::fs::read_to_string(proj.join("Cookfile")).unwrap();
    let abs = format!("{}/./dist/", proj.display());
    std::fs::write(proj.join("Cookfile"), cookfile.replace("\"dist/\"", &format!("{abs:?}"))).unwrap();
    let out = cook(&proj, &tmp.path().join("cache"), &["build"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(proj.join("dist/a").is_file() && proj.join("Cookfile").is_file());
    // A cache hit sweeps the stray and keeps what the build recorded.
    std::fs::write(proj.join("dist/stray"), "s").unwrap();
    let out = cook(&proj, &tmp.path().join("cache"), &["build"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("1 cached recipes"), "not a hit");
    assert!(!proj.join("dist/stray").exists(), "stray survived the hit");
    for f in ["dist/a", "Cookfile", "keep.txt", "src/lib/a.c"] {
        assert!(proj.join(f).is_file(), "{f} deleted");
    }
}

#[cfg(unix)]
#[test]
fn a_directory_output_reached_through_a_link_deletes_nothing() {
    let (tmp, proj) = project("dist/");
    std::os::unix::fs::symlink(".", proj.join("dist")).unwrap();
    // Build may succeed or fail; the files and the one warning are asserted.
    let out = cook(&proj, &tmp.path().join("cache"), &["build"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.matches("directory output \"dist\" is reached through a symlink").count(), 1, "{err}");
    assert!(proj.join("Cookfile").is_file(), "Cookfile deleted");
    assert!(proj.join("keep.txt").is_file(), "keep.txt deleted");
    assert!(tmp.path().join("a/sibling.txt").is_file(), "sibling deleted");
}
