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

#[test]
fn collision_with_builtin_prints_notice_and_runs_builtin() {
    for name in ["menu", "list"] {
        let tmp = TempDir::new().expect("tempdir");
        write_cookfile(
            tmp.path(),
            &format!("recipe {name}\n    cook \"recipe-ran\" {{ echo ran > $<out> }}\n"),
        );

        let out = run_cook(tmp.path(), &[name]);
        assert!(
            out.status.success(),
            "cook {name} failed: stderr={}",
            String::from_utf8_lossy(&out.stderr),
        );
        assert_eq!(
            String::from_utf8(out.stderr).expect("utf-8 stderr"),
            format!("cook: notice: a recipe named '{name}' exists; use cook +{name} to build it\n"),
        );
        assert!(
            !tmp.path().join("recipe-ran").exists(),
            "bare built-in name must not run the colliding recipe",
        );
    }
}

#[test]
fn escaped_collision_runs_recipe_without_notice() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "recipe menu\n    cook \"recipe-ran\" { echo ran > $<out> }\n",
    );

    let out = run_cook(tmp.path(), &["+menu"]);
    assert!(
        out.status.success(),
        "cook +menu failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !String::from_utf8(out.stderr)
            .expect("utf-8 stderr")
            .contains("cook: notice:"),
        "escaped recipe invocation must not print a collision notice",
    );
    assert!(
        tmp.path().join("recipe-ran").exists(),
        "escaped recipe body must run"
    );
}

#[test]
fn collision_check_does_not_run_registration_twice() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "register\n    print(\"register-marker\")\n    cook.recipe(\"menu\", {}, function() end)\n",
    );

    let out = run_cook(tmp.path(), &["menu"]);
    assert!(
        out.status.success(),
        "cook menu failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    assert_eq!(
        stdout.matches("register-marker").count(),
        1,
        "collision preflight must not execute register-phase Lua; stdout:\n{stdout}",
    );
    assert_eq!(
        String::from_utf8(out.stderr).expect("utf-8 stderr"),
        "cook: notice: a recipe named 'menu' exists; use cook +menu to build it\n",
    );
}

#[test]
fn colliding_chore_does_not_print_recipe_notice() {
    for name in ["menu", "list"] {
        let tmp = TempDir::new().expect("tempdir");
        write_cookfile(tmp.path(), &format!("chore {name}\n    echo chore-ran\n"));

        let out = run_cook(tmp.path(), &[name]);
        assert!(
            out.status.success(),
            "cook {name} failed: stderr={}",
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(
            !String::from_utf8(out.stderr)
                .expect("utf-8 stderr")
                .contains("cook: notice:"),
            "a chore collision must not be described as an escapable recipe",
        );
    }
}

#[test]
fn registered_workspace_builtin_warns_for_dynamic_recipe_once() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "register\n    cook.recipe(\"test\", {}, function() end)\n",
    );

    let out = run_cook(tmp.path(), &["test"]);
    assert!(
        out.status.success(),
        "cook test failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8(out.stderr).expect("utf-8 stderr");
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.contains("cook: notice:"))
            .collect::<Vec<_>>(),
        vec!["cook: notice: a recipe named 'test' exists; use cook +test to build it"],
    );
}

#[test]
fn standalone_builtin_warns_for_dynamic_recipe_once() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "register\n    cook.recipe(\"emit-lua\", {}, function() end)\n",
    );

    let out = run_cook(tmp.path(), &["emit-lua"]);
    assert!(
        out.status.success(),
        "cook emit-lua failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(
        String::from_utf8(out.stderr).expect("utf-8 stderr"),
        "cook: notice: a recipe named 'emit-lua' exists; use cook +emit-lua to build it\n",
    );
}

#[test]
fn qualified_imported_recipe_does_not_collide_with_builtin() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir(tmp.path().join("member")).expect("mkdir member");
    write_cookfile(tmp.path(), "import alias ./member\n");
    write_cookfile(&tmp.path().join("member"), "recipe menu\n");

    let out = run_cook(tmp.path(), &["menu"]);
    assert!(
        out.status.success(),
        "cook menu failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !String::from_utf8(out.stderr)
            .expect("utf-8 stderr")
            .contains("cook: notice:"),
        "qualified alias.menu must not collide with bare menu",
    );
}
