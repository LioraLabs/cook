//! Integration smoke test for `cook list`.
//!
//! Builds the `cook` binary (via `env!("CARGO_BIN_EXE_cook")`) and invokes
//! it against a minimal Cookfile in a temp directory. Verifies the
//! machine-readable output contract: one name per line, no decoration, no
//! kind prefix, no column padding.

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn write_cookfile(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join("Cookfile"), body).expect("write Cookfile");
}

fn run_list(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(cook_bin());
    cmd.arg("list");
    for a in args {
        cmd.arg(a);
    }
    cmd.current_dir(dir)
        .output()
        .expect("invoke cook list")
}

#[test]
fn list_prints_names_one_per_line_no_decoration() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        // Two recipes + two chores. Recipes are body-less (declarative);
        // chores keep shell bodies. Trivial so parse succeeds.
        "recipe build\n\
         \n\
         recipe deploy\n\
         \n\
         chore clean\n\
             rm -rf build\n\
         \n\
         chore fmt\n\
             echo fmt\n",
    );

    let out = run_list(tmp.path(), &[]);
    assert!(
        out.status.success(),
        "cook list failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let lines: Vec<&str> = stdout.lines().collect();

    // Recipes first (in source order), then chores. No decoration, no
    // leading whitespace, no kind prefix.
    assert_eq!(lines, vec!["build", "deploy", "clean", "fmt"]);

    // Each line is exactly the name — no trailing spaces, no padding.
    for line in &lines {
        assert_eq!(
            *line,
            line.trim(),
            "line has surrounding whitespace: {line:?}"
        );
        assert!(
            !line.contains("recipe ") && !line.contains("chore "),
            "kind prefix leaked into output: {line:?}",
        );
    }
}

#[test]
fn list_recipes_only_filters_chores() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "recipe build\n\
         \n\
         chore clean\n\
             rm -rf build\n",
    );

    let out = run_list(tmp.path(), &["--recipes-only"]);
    assert!(
        out.status.success(),
        "cook list --recipes-only failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["build"]);
}

#[test]
fn list_chores_only_filters_recipes() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "recipe build\n\
         \n\
         chore clean\n\
             rm -rf build\n",
    );

    let out = run_list(tmp.path(), &["--chores-only"]);
    assert!(
        out.status.success(),
        "cook list --chores-only failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["clean"]);
}

#[test]
fn list_rejects_both_filter_flags() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(tmp.path(), "recipe build\n");

    let out = run_list(tmp.path(), &["--recipes-only", "--chores-only"]);
    assert!(
        !out.status.success(),
        "cook list with both filter flags should fail; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
