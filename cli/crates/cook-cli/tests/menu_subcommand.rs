//! Integration smoke test for `cook menu`.
//!
//! `cook menu` is the workspace listing: it prints `  recipe NAME` /
//! `  chore  NAME [params...]` lines, where the chore params render via
//! `ChoreParamMeta::display_token()`. `cook list` is an alias for it and
//! shares the renderer. Neither invokes any recipe body, so no cache
//! isolation is needed here.

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

/// `cook list` is an alias for `cook menu`: same renderer, byte-identical
/// output. It once printed bare names for shell pipelines; that second render
/// path is gone, and tab completion is the name-discovery surface now.
#[test]
fn list_is_an_alias_for_menu() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "chore greet caller who=\"world\"\n    echo \"$<who>\"\n\
         \n\
         recipe build\n",
    );

    let list = run_cook(tmp.path(), &["list"]);
    assert!(
        list.status.success(),
        "cook list failed: stderr={}",
        String::from_utf8_lossy(&list.stderr),
    );
    let menu = run_cook(tmp.path(), &["menu"]);
    assert!(
        menu.status.success(),
        "cook menu failed: stderr={}",
        String::from_utf8_lossy(&menu.stderr),
    );

    let list_stdout = String::from_utf8(list.stdout).expect("utf-8 stdout");
    let menu_stdout = String::from_utf8(menu.stdout).expect("utf-8 stdout");
    assert_eq!(
        list_stdout, menu_stdout,
        "cook list must render exactly what cook menu renders"
    );
    assert!(
        list_stdout.contains("chore  greet caller who=\"world\""),
        "alias must inherit menu's kind label and params suffix; stdout:\n{list_stdout}"
    );
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

/// A recipe registered with an `origin` field renders `(from ...)` after its
/// existing `  recipe NAME` line.
#[test]
fn annotated_recipe_renders_origin() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "register\n    \
         cook.recipe(\"web:build\", {requires = {}, origin = \"cook_pnpm.workspace\"}, function() end)\n",
    );

    let out = run_cook(tmp.path(), &["menu"]);
    assert!(
        out.status.success(),
        "cook menu failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let line = stdout
        .lines()
        .find(|l| l.contains("web:build"))
        .unwrap_or_else(|| panic!("expected a web:build line; stdout:\n{stdout}"));
    assert_eq!(line, "  recipe web:build  (from cook_pnpm.workspace)");
}

/// A plain, unannotated `recipe build` sharing the Cookfile with an annotated
/// recipe must render exactly as it does today: no padding, no trailing
/// space, unaffected by the annotation column computed from the *other*
/// line. Asserted against the exact stdout line (not a whole-stdout
/// substring check), because once annotations exist elsewhere in the output
/// a `contains` check on a short name like "build" is fragile against
/// sibling recipe names.
#[test]
fn unannotated_sibling_of_annotated_recipe_has_no_trailing_space() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "register\n    \
         cook.recipe(\"web:build\", {requires = {}, origin = \"cook_pnpm.workspace\"}, function() end)\n\
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
    let line = stdout
        .lines()
        .find(|l| l == &"  recipe build")
        .unwrap_or_else(|| panic!("expected an exact '  recipe build' line; stdout:\n{stdout}"));
    assert_eq!(line, "  recipe build");
}

/// Two annotated recipes of differing name length have their `(from` at the
/// same column: the annotation column is the max rendered width of
/// `{name}{suffix}` across *annotated* entries only, plus a two-space
/// gutter.
#[test]
fn annotated_recipes_align_origin_column() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "register\n    \
         cook.recipe(\"web:build\", {requires = {}, origin = \"cook_pnpm.workspace\"}, function() end)\n    \
         cook.recipe(\"cc:config-header\", {requires = {}, origin = \"cook_cc.config_header\"}, function() end)\n",
    );

    let out = run_cook(tmp.path(), &["menu"]);
    assert!(
        out.status.success(),
        "cook menu failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let build_line = stdout
        .lines()
        .find(|l| l.contains("web:build"))
        .unwrap_or_else(|| panic!("expected a web:build line; stdout:\n{stdout}"));
    let header_line = stdout
        .lines()
        .find(|l| l.contains("cc:config-header"))
        .unwrap_or_else(|| panic!("expected a cc:config-header line; stdout:\n{stdout}"));

    let build_col = build_line.find("(from").expect("build line has annotation");
    let header_col = header_line.find("(from").expect("header line has annotation");
    assert_eq!(
        build_col, header_col,
        "annotations must align to the same column; stdout:\n{stdout}"
    );
    assert_eq!(build_line, "  recipe web:build         (from cook_pnpm.workspace)");
    assert_eq!(header_line, "  recipe cc:config-header  (from cook_cc.config_header)");
}

/// A workspace with zero annotated recipes must produce output
/// byte-identical to today: no annotation-column computation may leak into
/// the plain rendering path.
#[test]
fn all_unannotated_workspace_output_is_unchanged() {
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
    assert_eq!(
        stdout,
        "  chore  greet caller who=\"world\"\n  recipe build\n",
        "all-unannotated output must be byte-identical to the pre-annotation renderer",
    );
}

/// `list` and `menu` remain byte-equal even when annotations are present.
#[test]
fn list_and_menu_byte_equal_with_annotations() {
    let tmp = TempDir::new().expect("tempdir");
    write_cookfile(
        tmp.path(),
        "register\n    \
         cook.recipe(\"web:build\", {requires = {}, origin = \"cook_pnpm.workspace\"}, function() end)\n\
         \n\
         recipe build\n",
    );

    let list = run_cook(tmp.path(), &["list"]);
    assert!(
        list.status.success(),
        "cook list failed: stderr={}",
        String::from_utf8_lossy(&list.stderr),
    );
    let menu = run_cook(tmp.path(), &["menu"]);
    assert!(
        menu.status.success(),
        "cook menu failed: stderr={}",
        String::from_utf8_lossy(&menu.stderr),
    );

    let list_stdout = String::from_utf8(list.stdout).expect("utf-8 stdout");
    let menu_stdout = String::from_utf8(menu.stdout).expect("utf-8 stdout");
    assert_eq!(
        list_stdout, menu_stdout,
        "cook list must render exactly what cook menu renders, annotations included"
    );
    assert!(
        list_stdout.contains("(from cook_pnpm.workspace)"),
        "alias must inherit menu's annotation rendering; stdout:\n{list_stdout}"
    );
}

/// An origin-annotated recipe minted inside an *imported* member lists under
/// its workspace-qualified name, and the annotation column is measured over
/// that qualified name — not the bare one it was registered with.
///
/// Pins the interaction between `list_workspace_names`' `{prefix}.{name}`
/// rewrite (cook-engine/src/pipeline/registers.rs) and `cmd_menu`'s column:
/// the rewrite happens before the width is taken, so a long prefix widens
/// the gutter rather than pushing `(from …)` out of alignment.
#[test]
fn imported_member_origin_lists_under_qualified_name() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir(tmp.path().join("member")).expect("mkdir member");
    write_cookfile(tmp.path(), "import sub ./member\n");
    write_cookfile(
        &tmp.path().join("member"),
        "register\n    \
         cook.recipe(\"pkg:build\", {requires = {}, origin = \"cook_pnpm.workspace\"}, function() end)\n",
    );

    let out = run_cook(tmp.path(), &["list"]);
    assert!(
        out.status.success(),
        "cook list failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");

    // The annotation must sit on the qualified line, and be measured off the
    // qualified name — not the bare `pkg:build` it was registered with.
    let line = stdout
        .lines()
        .find(|l| l.contains("pkg:build"))
        .unwrap_or_else(|| panic!("qualified line absent; stdout:\n{stdout}"));
    assert_eq!(
        line, "  recipe sub.pkg:build  (from cook_pnpm.workspace)",
        "imported minted recipe must list qualified, annotated, gutter-aligned off the qualified name; stdout:\n{stdout}"
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
