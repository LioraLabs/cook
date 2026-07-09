//! Integration smoke test for `cook menu`.
//!
//! `cook menu` is the human-readable counterpart of `cook list`: it prints
//! `  recipe NAME` / `  chore  NAME [params...]` lines, where the chore
//! params render via `ChoreParamMeta::display_token()`. Neither `menu` nor
//! `list` invoke any recipe body, so no cache isolation is needed here.

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn write_cookfile(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join("Cookfile"), body).expect("write Cookfile");
}

fn run_cook(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_bin())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("invoke cook")
}

/// `cook menu` on a chore with a required positional and a defaulted-string
/// positional must print the exact `  chore  NAME param1 param2="default"`
/// line that `cmd_menu` (cli/crates/cook-cli/src/pipeline.rs) emits, and a
/// plain recipe with no params must print with no trailing suffix.
#[test]
fn menu_shows_chore_params_and_bare_recipe() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "chore greet caller who=\"world\"\n    echo \"$<who>\"\n\
         \n\
         recipe build\n",
    );

    let out = run_cook(tmp.path(), &["menu"]);
    assert!(
        out.status.success(),
        "cook menu failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    assert!(
        stdout.contains("chore  greet caller who=\"world\""),
        "expected the chore line with rendered params; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("recipe build"),
        "expected the bare recipe line with no param suffix; stdout:\n{stdout}"
    );
    // The recipe line must not carry a trailing space where an empty
    // params suffix would have gone.
    assert!(
        !stdout.contains("recipe build \n") && !stdout.contains("recipe build "),
        "recipe with no params must not have a trailing space; stdout:\n{stdout}"
    );
}

/// `cook list` (the machine-readable counterpart) must print the bare
/// names only — no kind label, no params — for the same Cookfile.
#[test]
fn list_shows_bare_names_no_params() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "chore greet caller who=\"world\"\n    echo \"$<who>\"\n\
         \n\
         recipe build\n",
    );

    let out = run_cook(tmp.path(), &["list"]);
    assert!(
        out.status.success(),
        "cook list failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let mut lines: Vec<&str> = stdout.lines().collect();
    lines.sort();
    // Bare names only, no kind label, no params, no decoration — order
    // between recipe/chore lines isn't asserted here (that's list_subcommand_test.rs's job).
    assert_eq!(lines, vec!["build", "greet"]);
}
