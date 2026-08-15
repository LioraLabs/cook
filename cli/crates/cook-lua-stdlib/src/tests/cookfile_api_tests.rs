//! `cook.cookfile.*` binding contract (CS-0179).
//!
//! The editing algebra is covered in `cook-cookfile`'s own suite. What is
//! pinned here is the binding: that Lua reaches it, that the write lands on
//! disk, that the sandbox holds, and that a failed edit leaves the file alone.

use mlua::Lua;
use std::path::Path;
use tempfile::TempDir;

use crate::cookfile_api::register_cookfile_api;
use crate::sandbox::SandboxSource;
use crate::WorkingDirSource;

const COOKFILE: &str = "\
use cook_cc

recipe app
    cook_cc.bin({
        sources = { \"src/main.cpp\" },  -- entry point
        links   = { \"mathlib\" },
    })
";

fn setup(source: &str) -> (Lua, TempDir) {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("Cookfile"), source).unwrap();

    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    lua.globals().set("cook", cook.clone()).unwrap();
    register_cookfile_api(
        &lua,
        &cook,
        WorkingDirSource::Static(dir.path().to_path_buf()),
        SandboxSource::confined(dir.path().to_path_buf()),
    )
    .unwrap();
    (lua, dir)
}

fn read(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("Cookfile")).unwrap()
}

#[test]
fn splice_field_writes_the_edit_to_disk() {
    let (lua, dir) = setup(COOKFILE);
    lua.load(r#"cook.cookfile.splice_field("Cookfile", "app", "links", '"physlib"')"#)
        .exec()
        .unwrap();

    let out = read(dir.path());
    assert!(out.contains(r#"{ "mathlib", "physlib" }"#), "got: {out}");
    // The binding preserves what the core preserves — this is the property a
    // caller actually depends on, so it is asserted here too and not only in
    // the core's suite.
    assert!(out.contains("-- entry point"), "comment survived: {out}");
}

#[test]
fn append_adds_a_declaration_at_end_of_file() {
    let (lua, dir) = setup(COOKFILE);
    lua.load(
        r#"cook.cookfile.append("Cookfile", 'recipe math\n    cook_cc.lib({ sources = { "src/math.cpp" } })')"#,
    )
    .exec()
    .unwrap();

    let out = read(dir.path());
    assert!(
        out.ends_with("recipe math\n    cook_cc.lib({ sources = { \"src/math.cpp\" } })\n"),
        "got: {out}"
    );
    assert!(out.contains("recipe app"), "existing content kept: {out}");
}

#[test]
fn find_call_returns_the_callee_without_editing() {
    let (lua, dir) = setup(COOKFILE);
    let callee: String = lua
        .load(r#"return cook.cookfile.find_call("Cookfile", "app").callee"#)
        .eval()
        .unwrap();
    assert_eq!(callee, "cook_cc.bin");
    assert_eq!(read(dir.path()), COOKFILE, "find_call must not write");
}

#[test]
fn find_call_returns_nil_for_a_recipe_without_one() {
    let (lua, _dir) = setup("recipe app\n    cook \"out\" { echo hi > $<out> }\n");
    let is_nil: bool = lua
        .load(r#"return cook.cookfile.find_call("Cookfile", "app") == nil"#)
        .eval()
        .unwrap();
    assert!(is_nil);
}

#[test]
fn a_failed_splice_leaves_the_file_untouched() {
    // The file is read, edited in memory, and written only on success. A verb
    // that reports an error must not also have half-rewritten the Cookfile —
    // the user would be left reconciling a partial edit against a message
    // telling them the edit did not happen.
    let (lua, dir) = setup(COOKFILE);
    let err = lua
        .load(r#"cook.cookfile.splice_field("Cookfile", "app", "needs", '"sdl2"')"#)
        .exec()
        .unwrap_err()
        .to_string();

    assert!(err.contains("couldn't find 'needs'"), "got: {err}");
    assert!(
        err.contains("add \"sdl2\" to it manually"),
        "actionable: {err}"
    );
    assert_eq!(read(dir.path()), COOKFILE, "file must be byte-identical");
}

#[test]
fn a_missing_recipe_is_reported_with_the_path() {
    let (lua, _dir) = setup(COOKFILE);
    let err = lua
        .load(r#"cook.cookfile.splice_field("Cookfile", "ghost", "links", '"x"')"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("no recipe named 'ghost'"), "got: {err}");
    assert!(err.contains("Cookfile"), "names the file: {err}");
}

#[test]
fn the_sandbox_refuses_a_path_outside_the_project_root() {
    // A chore rewriting the Cookfile that invoked it is the feature; a chore
    // reaching another project's Cookfile is not. Same gate as `fs.*`
    // (CS-0045).
    let (lua, _dir) = setup(COOKFILE);
    let err = lua
        .load(r#"cook.cookfile.splice_field("../../etc/Cookfile", "app", "links", '"x"')"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("cook.cookfile.splice_field"),
        "tagged with the rejecting API: {err}"
    );
    assert!(
        !err.contains("no recipe named"),
        "must be refused by the sandbox before any read: {err}"
    );
}

#[test]
fn an_edit_round_trips_through_a_second_edit() {
    // Two verbs in sequence is the real usage (`cc.link app math` then
    // `cc.link app phys`), and the second must see the first's output as
    // ordinary well-formed input.
    let (lua, dir) = setup(COOKFILE);
    lua.load(r#"cook.cookfile.splice_field("Cookfile", "app", "links", '"physlib"')"#)
        .exec()
        .unwrap();
    lua.load(r#"cook.cookfile.splice_field("Cookfile", "app", "links", '"utillib"')"#)
        .exec()
        .unwrap();

    let out = read(dir.path());
    assert!(
        out.contains(r#"{ "mathlib", "physlib", "utillib" }"#),
        "got: {out}"
    );
}

// ---------------------------------------------------------------------------
// The options table and `field_entries` (CS-0221). The editing algebra is the
// core's suite; what is pinned here is that the policy reaches it, that a
// mistyped option is refused rather than quietly meaning "refuse", and that
// the read comes back as a Lua sequence.
// ---------------------------------------------------------------------------

const NO_LINKS: &str = "\
use cook_cc

recipe game
    cook_cc.bin({ sources = { \"src/game/main.cpp\" } })
";

#[test]
fn create_if_absent_writes_the_field_the_default_refuses() {
    let (lua, dir) = setup(NO_LINKS);
    let err = lua
        .load(r#"cook.cookfile.splice_field("Cookfile", "game", "links", '"math"')"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("couldn't find 'links'"), "got: {err}");
    assert_eq!(read(dir.path()), NO_LINKS, "the default writes nothing");

    lua.load(
        r#"cook.cookfile.splice_field("Cookfile", "game", "links", '"math"', { create_if_absent = true })"#,
    )
    .exec()
    .unwrap();
    assert_eq!(
        read(dir.path()),
        "use cook_cc\n\nrecipe game\n    cook_cc.bin({ sources = { \"src/game/main.cpp\" }, links = { \"math\" } })\n"
    );
}

#[test]
fn create_if_absent_false_is_the_default_spelled_out() {
    let (lua, dir) = setup(NO_LINKS);
    let err = lua
        .load(
            r#"cook.cookfile.splice_field("Cookfile", "game", "links", '"math"', { create_if_absent = false })"#,
        )
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("couldn't find 'links'"), "got: {err}");
    assert_eq!(read(dir.path()), NO_LINKS);
}

#[test]
fn a_mistyped_option_is_refused_rather_than_read_as_a_refusal() {
    // The failure this prevents: a verb asks to create, the option name is
    // wrong, and the user is told the engine could not find a field their verb
    // believed it had just asked for.
    let (lua, dir) = setup(NO_LINKS);
    let err = lua
        .load(
            r#"cook.cookfile.splice_field("Cookfile", "game", "links", '"math"', { create_if_missing = true })"#,
        )
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("unknown option 'create_if_missing'"),
        "got: {err}"
    );
    assert!(
        err.contains("create_if_absent"),
        "names the real one: {err}"
    );
    assert_eq!(read(dir.path()), NO_LINKS, "refused before any write");
}

#[test]
fn a_non_boolean_option_value_is_refused() {
    let (lua, _dir) = setup(NO_LINKS);
    let err = lua
        .load(
            r#"cook.cookfile.splice_field("Cookfile", "game", "links", '"math"', { create_if_absent = "yes" })"#,
        )
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("must be a boolean"), "got: {err}");
}

#[test]
fn field_entries_comes_back_as_a_lua_sequence() {
    let (lua, dir) = setup(COOKFILE);
    let joined: String = lua
        .load(
            r#"return table.concat(cook.cookfile.field_entries("Cookfile", "app", "links"), "|")"#,
        )
        .eval()
        .unwrap();
    assert_eq!(joined, "\"mathlib\"");
    assert_eq!(read(dir.path()), COOKFILE, "field_entries must not write");
}

#[test]
fn field_entries_is_nil_where_there_is_nothing_to_read() {
    // The three cases a verb meets: the field is not there, the recipe is not
    // there, and the recipe holds no module call. All three answer nil, so a
    // verb's create-or-append decision is one comparison.
    let (lua, _dir) = setup(COOKFILE);
    let all_nil: bool = lua
        .load(
            r#"return cook.cookfile.field_entries("Cookfile", "app", "needs") == nil
                and cook.cookfile.field_entries("Cookfile", "ghost", "links") == nil"#,
        )
        .eval()
        .unwrap();
    assert!(all_nil);
}

#[test]
fn field_entries_is_gated_by_the_same_sandbox() {
    let (lua, _dir) = setup(COOKFILE);
    let err = lua
        .load(r#"cook.cookfile.field_entries("../../etc/Cookfile", "app", "links")"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("cook.cookfile.field_entries"), "got: {err}");
    // The second half is what makes this a sandbox test rather than a
    // "something went wrong" test: refused before any read, so the message
    // cannot be about what was or was not in the file.
    assert!(
        !err.contains("no recipe named") && !err.contains("couldn't find"),
        "must be refused by the sandbox before any read: {err}"
    );
}
