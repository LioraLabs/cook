//! The `use`-wiring contract (§27.1.1, CS-0220).
//!
//! Three cases, two refusals, and one property that outranks all of them:
//! everything outside the inserted line comes back byte-identical. Most
//! assertions here are therefore about what did NOT move.

use super::*;
use crate::pipeline::COOK_GITIGNORE_MARKER;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

// ---------------------------------------------------------------------------
// plan_wiring — the pure decision
// ---------------------------------------------------------------------------

#[test]
fn no_cookfile_plans_a_file_holding_only_the_declaration() {
    let plan = plan_wiring(None, &names(&["cook_cc"])).expect("plans");
    assert_eq!(plan.source.as_deref(), Some("use cook_cc\n"));
    assert_eq!(plan.added, names(&["cook_cc"]));
    assert!(plan.unspellable.is_empty());
}

#[test]
fn the_created_file_carries_no_recipe_of_its_own() {
    // Deliberately NOT `cmd_init`'s template: the next scaffolding verb writes
    // the project's first recipe, and a starter `build` here is a name it
    // would have to collide with or route around.
    let plan = plan_wiring(None, &names(&["cook_cc"])).expect("plans");
    let source = plan.source.expect("writes");
    assert!(!source.contains("recipe"));
    assert!(!source.contains("chore"));
}

#[test]
fn an_existing_cookfile_takes_the_line_under_its_leading_comment_block() {
    let src = "\
# The game.
# Build it with `cook build`.

recipe build
    cook_cc.bin({ sources = { \"src/main.cpp\" } })
";
    let plan = plan_wiring(Some(src), &names(&["cook_cc"])).expect("plans");
    assert_eq!(
        plan.source.as_deref(),
        Some(
            "\
# The game.
# Build it with `cook build`.
use cook_cc

recipe build
    cook_cc.bin({ sources = { \"src/main.cpp\" } })
"
        )
    );
}

#[test]
fn everything_outside_the_inserted_line_is_byte_identical() {
    let src = "\
# header

use cook_pnpm

recipe app
    cook_cc.bin({
        sources  = { \"src/main.cpp\" },  -- entry point
        links    = { \"mathlib\" },
        standard = cxx_std,
    })
";
    let out = plan_wiring(Some(src), &names(&["cook_cc"]))
        .expect("plans")
        .source
        .expect("writes");
    assert_eq!(out.len(), src.len() + "use cook_cc\n".len());
    assert_eq!(out.replacen("use cook_cc\n", "", 1), src);
}

#[test]
fn a_declaration_already_present_plans_no_write() {
    let src = "use cook_cc\n\nrecipe build\n    cook_cc.bin({})\n";
    let plan = plan_wiring(Some(src), &names(&["cook_cc"])).expect("plans");
    assert_eq!(plan.source, None, "the file must be left byte-identical");
    assert!(plan.added.is_empty(), "and nothing is reported");
}

#[test]
fn wiring_is_idempotent() {
    let src = "# header\n\nrecipe build\n    cook_cc.bin({})\n";
    let once = plan_wiring(Some(src), &names(&["cook_cc"]))
        .expect("plans")
        .source
        .expect("writes");
    let twice = plan_wiring(Some(&once), &names(&["cook_cc"])).expect("plans");
    assert_eq!(twice.source, None);
}

#[test]
fn several_names_are_declared_in_the_order_they_were_asked_for() {
    let plan = plan_wiring(None, &names(&["cook_cc", "cook_pnpm"])).expect("plans");
    assert_eq!(
        plan.source.as_deref(),
        Some("use cook_cc\nuse cook_pnpm\n"),
        "argv order, one line each"
    );
    assert_eq!(plan.added, names(&["cook_cc", "cook_pnpm"]));
}

#[test]
fn a_name_already_present_is_skipped_while_its_neighbour_is_added() {
    let src = "use cook_cc\n\nrecipe build\n    cook_cc.bin({})\n";
    let plan = plan_wiring(Some(src), &names(&["cook_cc", "cook_pnpm"])).expect("plans");
    assert_eq!(plan.added, names(&["cook_pnpm"]));
    assert_eq!(
        plan.source.as_deref(),
        Some("use cook_cc\nuse cook_pnpm\n\nrecipe build\n    cook_cc.bin({})\n")
    );
}

#[test]
fn a_name_that_is_not_a_use_name_is_refused_rather_than_rewritten() {
    let plan = plan_wiring(None, &names(&["foo-bar"])).expect("plans");
    assert_eq!(plan.source, None, "no file is written for it");
    assert_eq!(plan.unspellable, names(&["foo-bar"]));
    assert!(plan.added.is_empty());
}

#[test]
fn a_refused_name_does_not_take_its_spellable_neighbour_with_it() {
    let plan = plan_wiring(None, &names(&["foo.bar", "cook_cc"])).expect("plans");
    assert_eq!(plan.source.as_deref(), Some("use cook_cc\n"));
    assert_eq!(plan.added, names(&["cook_cc"]));
    assert_eq!(plan.unspellable, names(&["foo.bar"]));
}

#[test]
fn an_unparseable_cookfile_is_refused() {
    // An unterminated recipe body: the file the author is mid-edit on.
    let src = "recipe build\n    cook \"out\" { echo hi\n";
    let err = plan_wiring(Some(src), &names(&["cook_cc"])).expect_err("refuses");
    assert_eq!(err, cook_cookfile::EditError::Unparseable);
}

// ---------------------------------------------------------------------------
// wire_use_declarations — the IO half
// ---------------------------------------------------------------------------

#[test]
fn an_empty_directory_gets_a_cookfile_and_the_managed_gitignore() {
    let dir = tempfile::tempdir().expect("tempdir");
    wire_use_declarations(dir.path(), &names(&["cook_cc"])).expect("wires");

    assert_eq!(
        std::fs::read_to_string(dir.path().join("Cookfile")).expect("Cookfile"),
        "use cook_cc\n"
    );
    let ignored = std::fs::read_to_string(dir.path().join(".gitignore")).expect(".gitignore");
    assert!(ignored.contains(COOK_GITIGNORE_MARKER));
    assert!(ignored.contains(".cook/**"));
}

#[test]
fn an_existing_gitignore_is_appended_to_rather_than_replaced() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join(".gitignore"), "build/\n").expect("seed");
    wire_use_declarations(dir.path(), &names(&["cook_cc"])).expect("wires");

    let ignored = std::fs::read_to_string(dir.path().join(".gitignore")).expect(".gitignore");
    assert!(
        ignored.starts_with("build/\n"),
        "the author's rules survive"
    );
    assert!(ignored.contains(COOK_GITIGNORE_MARKER));
}

#[test]
fn an_existing_cookfile_keeps_its_bytes_and_gains_one_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = "# my project\n\nrecipe build\n    cook \"out\" { echo hi > $<out> }\n";
    std::fs::write(dir.path().join("Cookfile"), src).expect("seed");
    wire_use_declarations(dir.path(), &names(&["cook_cc"])).expect("wires");

    let out = std::fs::read_to_string(dir.path().join("Cookfile")).expect("Cookfile");
    assert_eq!(out.replacen("use cook_cc\n", "", 1), src);
    assert!(
        !dir.path().join(".gitignore").exists(),
        "an existing project's .gitignore is not this command's business"
    );
}

#[test]
fn a_project_that_already_uses_the_module_is_left_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = "use cook_cc\n\nrecipe build\n    cook_cc.bin({})\n";
    std::fs::write(dir.path().join("Cookfile"), src).expect("seed");
    wire_use_declarations(dir.path(), &names(&["cook_cc"])).expect("wires");

    assert_eq!(
        std::fs::read_to_string(dir.path().join("Cookfile")).expect("Cookfile"),
        src
    );
}

#[test]
fn an_unparseable_cookfile_fails_the_command_and_is_not_touched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = "recipe build\n    cook \"out\" { echo hi\n";
    std::fs::write(dir.path().join("Cookfile"), src).expect("seed");

    let err = wire_use_declarations(dir.path(), &names(&["cook_cc"])).expect_err("fails");
    let msg = err.to_string();
    assert!(msg.contains("use cook_cc"), "names the missing line: {msg}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Cookfile")).expect("Cookfile"),
        src,
        "the file the author is mid-edit on is left exactly as it was"
    );
}

#[test]
fn an_unspellable_name_alone_creates_no_cookfile() {
    let dir = tempfile::tempdir().expect("tempdir");
    wire_use_declarations(dir.path(), &names(&["foo-bar"])).expect("wires");
    assert!(
        !dir.path().join("Cookfile").exists(),
        "an empty Cookfile is worse than none"
    );
}
