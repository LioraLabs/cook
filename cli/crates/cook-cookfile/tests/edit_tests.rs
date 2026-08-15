//! Cookfile editing contract (CS-0179).
//!
//! The property under test throughout is preservation: an edit inserts bytes
//! and changes nothing else. Most assertions therefore check what did NOT
//! move, which is the half a decode/re-encode implementation would fail.

use cook_cookfile::{
    append_declaration, ensure_use, field_entries, find_call, splice_into_field, AbsentField,
    EditError, UseEdit,
};

const APP: &str = "\
use cook_cc

recipe app
    cook_cc.bin({
        sources  = { \"src/main.cpp\" },  -- entry point
        links    = { \"mathlib\" },
        standard = cxx_std,
    })
";

#[test]
fn splices_into_a_populated_list_preserving_everything_else() {
    let out = splice_into_field(APP, "app", "links", "\"physlib\"", AbsentField::Refuse).unwrap();

    assert!(out.contains("{ \"mathlib\", \"physlib\" }"));
    // Nothing else moved: the comment, the non-literal Lua, and the author's
    // column alignment are all byte-identical.
    assert!(out.contains("-- entry point"));
    assert!(out.contains("standard = cxx_std,"));
    assert!(out.contains("sources  = { \"src/main.cpp\" },"));
    // And the edit is exactly the inserted bytes, nothing more.
    assert_eq!(out.len(), APP.len() + ", \"physlib\"".len());
}

#[test]
fn splices_into_an_empty_list_without_a_leading_comma() {
    let src = "recipe app\n    cook_cc.bin({ links = {} })\n";
    let out = splice_into_field(src, "app", "links", "\"mathlib\"", AbsentField::Refuse).unwrap();
    assert!(out.contains("{ links = {\"mathlib\"} }"), "got: {out}");
    assert!(
        !out.contains(", \"mathlib\""),
        "no leading comma on an empty list"
    );
}

#[test]
fn preserves_interior_padding_rather_than_inserting_before_the_brace() {
    // Inserting immediately before `}` would yield `{ "a", "b"}` and quietly
    // restyle the author's line. The anchor is the last non-whitespace byte.
    let src = "recipe app\n    cook_cc.bin({ links = { \"a\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap();
    assert!(out.contains("{ \"a\", \"b\" }"), "got: {out}");
}

#[test]
fn a_brace_inside_a_string_does_not_close_the_list() {
    // The reason field location is a quote-aware scan and not `find('}')`.
    // Getting this wrong splices into the middle of a string literal.
    let src = "recipe app\n    cook_cc.bin({ sources = { \"src/a}b.cpp\" } })\n";
    let out =
        splice_into_field(src, "app", "sources", "\"src/c.cpp\"", AbsentField::Refuse).unwrap();
    assert!(
        out.contains("{ \"src/a}b.cpp\", \"src/c.cpp\" }"),
        "got: {out}"
    );
}

#[test]
fn a_field_name_appearing_inside_another_token_is_not_matched() {
    // `find("links")` would hit `mathlinks` first and splice into `sources`.
    let src =
        "recipe app\n    cook_cc.bin({ sources = { \"mathlinks.cpp\" }, links = { \"m\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"n\"", AbsentField::Refuse).unwrap();
    assert!(
        out.contains("{ \"mathlinks.cpp\" }"),
        "sources untouched: {out}"
    );
    assert!(out.contains("links = { \"m\", \"n\" }"), "got: {out}");
}

#[test]
fn edits_the_named_recipe_not_the_first_one() {
    let src = "\
recipe alpha
    cook_cc.lib({ links = { \"a\" } })

recipe beta
    cook_cc.lib({ links = { \"b\" } })
";
    let out = splice_into_field(src, "beta", "links", "\"z\"", AbsentField::Refuse).unwrap();
    assert!(out.contains("{ \"a\" }"), "alpha untouched: {out}");
    assert!(out.contains("{ \"b\", \"z\" }"), "got: {out}");
}

#[test]
fn multiline_nested_braces_are_spanned_correctly() {
    let src = "\
recipe app
    cook_cc.bin({
        options = { warnings = { \"all\", \"extra\" } },
        links   = { \"a\" },
    })
";
    let out = splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap();
    assert!(out.contains("links   = { \"a\", \"b\" }"), "got: {out}");
    assert!(
        out.contains("warnings = { \"all\", \"extra\" }"),
        "nested table untouched"
    );
}

// ---------------------------------------------------------------------------
// Honest degradation. Each of these is a case where a lossy re-render would
// silently succeed and write something the author did not ask for.
// ---------------------------------------------------------------------------

#[test]
fn missing_recipe_is_named() {
    let err = splice_into_field(APP, "nope", "links", "\"x\"", AbsentField::Refuse).unwrap_err();
    assert_eq!(
        err,
        EditError::RecipeNotFound {
            recipe: "nope".into()
        }
    );
    assert!(err.to_string().contains("no recipe named 'nope'"));
}

#[test]
fn recipe_without_a_module_call_is_named() {
    let src = "recipe app\n    cook \"out\" { echo hi > $<out> }\n";
    let err = splice_into_field(src, "app", "links", "\"x\"", AbsentField::Refuse).unwrap_err();
    assert_eq!(
        err,
        EditError::NoModuleCall {
            recipe: "app".into()
        }
    );
}

#[test]
fn missing_field_reports_the_manual_fix() {
    let src = "recipe app\n    cook_cc.bin({ sources = { \"a.cpp\" } })\n";
    let err =
        splice_into_field(src, "app", "links", "\"mathlib\"", AbsentField::Refuse).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("couldn't find 'links'"), "got: {msg}");
    assert!(msg.contains("cook_cc.bin"), "names the call: {msg}");
    assert!(
        msg.contains("add \"mathlib\" to it manually"),
        "actionable: {msg}"
    );
}

#[test]
fn a_field_that_is_not_a_list_is_refused_rather_than_mangled() {
    let src = "recipe app\n    cook_cc.bin({ links = shared_links })\n";
    let err = splice_into_field(src, "app", "links", "\"x\"", AbsentField::Refuse).unwrap_err();
    assert!(
        matches!(err, EditError::FieldNotAList { .. }),
        "got: {err:?}"
    );
    assert!(err.to_string().contains("is not a `{ ... }` list"));
}

#[test]
fn an_unparseable_cookfile_is_refused_before_any_edit() {
    let src = "recipe app\n    cook_cc.bin({ links = { \"a\" }\n";
    assert_eq!(
        splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap_err(),
        EditError::Unparseable
    );
}

// ---------------------------------------------------------------------------
// Appending and locating
// ---------------------------------------------------------------------------

#[test]
fn append_separates_with_exactly_one_blank_line() {
    let out = append_declaration("use cook_cc\n", "recipe app\n    cook_cc.bin({})");
    assert_eq!(out, "use cook_cc\n\nrecipe app\n    cook_cc.bin({})\n");
}

#[test]
fn append_normalises_ragged_trailing_whitespace() {
    // Whatever the file ended with, the result is one blank line and one
    // trailing newline — appending twice must not drift.
    let once = append_declaration("use cook_cc\n\n\n\n", "recipe a\n    x.y({})");
    let twice = append_declaration(&once, "recipe b\n    x.y({})");
    assert_eq!(
        twice,
        "use cook_cc\n\nrecipe a\n    x.y({})\n\nrecipe b\n    x.y({})\n"
    );
}

#[test]
fn append_to_an_empty_file_adds_no_leading_blank_line() {
    assert_eq!(append_declaration("", "recipe a"), "recipe a\n");
}

#[test]
fn find_call_reports_the_callee() {
    let call = find_call(APP, "app").unwrap();
    assert_eq!(call.callee, "cook_cc.bin");
    assert!(APP[call.span].starts_with("cook_cc.bin({"));
}

#[test]
fn a_brace_inside_a_comment_does_not_close_the_list() {
    // This layer's whole purpose is preserving comments, so miscounting depth
    // on one would be an especially poor way to fail. Without comment
    // awareness the `}` in the comment closes the list and the entry lands
    // between the comment and the real closing brace.
    let src = "\
recipe app
    cook_cc.bin({
        links = {
            \"mathlib\",   -- see docs/build.md {section 2}
        },
    })
";
    let out = splice_into_field(src, "app", "links", "\"physlib\"", AbsentField::Refuse).unwrap();
    assert!(
        out.contains("-- see docs/build.md {section 2}\n"),
        "comment must survive untouched: {out}"
    );
    // Anchored after the trailing comma — which already separates, so no
    // second comma is added — and ahead of the comment, which stays put.
    assert!(
        out.contains("\"mathlib\", \"physlib\"   -- see docs/build.md {section 2}"),
        "got: {out}"
    );
}

// ---------------------------------------------------------------------------
// What is a key (CS-0208). A field name is a table key at the top level of the
// call's argument. Text that merely looks like one — in a comment, in a
// string, or in a table nested inside the call — is not the field, and each of
// the three used to be spliced into.
// ---------------------------------------------------------------------------

#[test]
fn a_commented_out_field_is_not_the_field() {
    // The worst way for this layer to be wrong: the entry lands inside the
    // author's comment, which is the thing the whole splice strategy exists to
    // preserve, and the real list is left alone.
    let src = "\
recipe app
    cook_cc.bin({
        -- links = { \"old\" },
        links = { \"a\" },
    })
";
    let out = splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap();
    assert!(
        out.contains("-- links = { \"old\" },\n"),
        "the comment must be byte-identical: {out}"
    );
    assert!(out.contains("links = { \"a\", \"b\" },"), "got: {out}");
}

#[test]
fn a_key_written_inside_a_string_is_not_the_field() {
    // `"links=1"` satisfies every test a substring scan can make: whole token,
    // followed by `=`. It made the real `links` unreachable and the call was
    // reported as not-a-list.
    let src = "recipe app\n    cook_cc.bin({ defines = { \"links=1\" }, links = { \"a\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap();
    assert!(out.contains("{ \"links=1\" }"), "defines untouched: {out}");
    assert!(out.contains("links = { \"a\", \"b\" }"), "got: {out}");
}

#[test]
fn a_nested_field_of_the_same_name_is_not_edited() {
    // The nested list is a different field of a different table. Editing it
    // still produces a file that parses and looks plausible, which is why this
    // is the one that would have survived review.
    let src = "recipe app\n    cook_cc.bin({ opts = { links = { \"x\" } }, links = { \"a\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap();
    assert!(
        out.contains("opts = { links = { \"x\" } }"),
        "nested untouched: {out}"
    );
    assert!(out.contains(", links = { \"a\", \"b\" } }"), "got: {out}");
}

#[test]
fn a_bracketed_key_is_refused_by_name_rather_than_mis_aimed() {
    // `["links"] = { … }` is a table key this scan does not read: the name
    // lives inside a string literal, and a string literal is not code. The
    // limit is pinned rather than described, because the thing that makes it
    // acceptable is that it FAILS — an unsupported spelling that silently
    // edited the next candidate would not be a limit, it would be the bug.
    let src = "recipe app\n    cook_cc.bin({ [\"links\"] = { \"a\" } })\n";
    let err = splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap_err();
    assert!(
        matches!(err, EditError::FieldNotFound { .. }),
        "got: {err:?}"
    );
}

#[test]
fn a_field_that_exists_only_nested_is_reported_missing() {
    // Failure is explicit and total (§22.13): being told to make the edit by
    // hand beats an edit to a list the caller did not name.
    let src = "recipe app\n    cook_cc.bin({ opts = { links = { \"x\" } } })\n";
    let err = splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap_err();
    assert!(
        matches!(err, EditError::FieldNotFound { .. }),
        "got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Long brackets (CS-0208). Lua spells a string four ways and a comment two,
// and a splice that understands only two of the six edits bytes the author
// never pointed at.
// ---------------------------------------------------------------------------

#[test]
fn a_brace_inside_a_long_bracket_string_does_not_close_the_list() {
    // `[[ … ]]` is a string literal, so §22.13's rule applies to it exactly as
    // it applies to `"…"`. Counting the `}` closes the list two entries early
    // and the insert lands INSIDE the literal: `[[a, "d"}b]]`.
    let src = "recipe app\n    cook_cc.bin({ links = { [[a}b]], \"c\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"d\"", AbsentField::Refuse).unwrap();
    assert!(out.contains("{ [[a}b]], \"c\", \"d\" }"), "got: {out}");
}

#[test]
fn a_levelled_long_bracket_is_understood_at_any_level() {
    // `[=[ … ]=]` is the spelling an author reaches for precisely when the
    // text contains `]]`, so it is the one most likely to hold odd bytes.
    let src = "recipe app\n    cook_cc.bin({ links = { [=[a}b]=], \"c\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"d\"", AbsentField::Refuse).unwrap();
    assert!(out.contains("{ [=[a}b]=], \"c\", \"d\" }"), "got: {out}");
}

#[test]
fn a_long_bracket_comment_does_not_swallow_the_rest_of_its_line() {
    // `--[[ … ]]` ends at `]]`, not at the newline. Reading it as a line
    // comment eats the real `}` that follows it on the same line, and the
    // field then reads as unterminated.
    let src = "recipe app\n    cook_cc.bin({ links = { \"a\" --[[ why } ]] } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap();
    assert!(out.contains("{ \"a\", \"b\" --[[ why } ]] }"), "got: {out}");
}

#[test]
fn two_dashes_inside_a_long_bracket_do_not_open_a_comment() {
    // The anchor retracts to before a `--` because a comment must not be
    // spliced into. Inside a string literal there is no comment to protect,
    // and retracting there splices into the string instead.
    let src = "recipe app\n    cook_cc.bin({ links = { [[note -- x]] } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap();
    assert!(out.contains("{ [[note -- x]], \"b\" }"), "got: {out}");
}

#[test]
fn a_subtraction_expression_is_not_read_as_a_comment() {
    // A single `-` must not start comment mode; `n-1` inside a table is legal
    // Lua and the scan has to keep counting braces through it.
    let src = "recipe app\n    cook_cc.bin({ jobs = { n-1 }, links = { \"a\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap();
    assert!(out.contains("{ n-1 }"), "expression untouched: {out}");
    assert!(out.contains("links = { \"a\", \"b\" }"), "got: {out}");
}

// ---------------------------------------------------------------------------
// `ensure_use`. Two properties carry every case below. Presence is structural,
// so text that merely spells `use cook_cc` does not count as the declaration;
// and the insert is one run of bytes at one offset, so the assertions are on
// the WHOLE result rather than on a substring of it — a `contains` cannot tell
// an insertion from a rewrite that happens to still contain the line.
// ---------------------------------------------------------------------------

/// Unwrap an insertion, naming the other variant when it comes back instead.
fn inserted(edit: UseEdit) -> String {
    match edit {
        UseEdit::Inserted(out) => out,
        UseEdit::AlreadyPresent => panic!("expected an insertion, got AlreadyPresent"),
    }
}

#[test]
fn an_empty_file_gets_the_declaration_and_no_trailing_blank_line() {
    // Nothing follows, so the blank line separating the run from the first
    // declaration would be trailing whitespace nobody asked for.
    assert_eq!(
        ensure_use("", "cook_cc").unwrap(),
        UseEdit::Inserted("use cook_cc\n".into())
    );
}

#[test]
fn the_declaration_lands_after_a_leading_comment_block() {
    // A file's opening comment block is its header — licence, provenance, what
    // this Cookfile builds. Inserting above it buries that under machinery, so
    // the run starts on the first line the block does not own. The blank line
    // that already separates the block from the recipe is left to do its job:
    // no second one is added.
    let src = "\
# Build the app.
# Two lines of it.

recipe app
    cook_cc.bin({})
";
    assert_eq!(
        inserted(ensure_use(src, "cook_cc").unwrap()),
        "\
# Build the app.
# Two lines of it.
use cook_cc

recipe app
    cook_cc.bin({})
"
    );
}

#[test]
fn a_blank_line_ends_the_header_so_a_comment_keeps_the_declaration_it_describes() {
    // The second comment here is not part of the header; it introduces the
    // recipe under it. Continuing the run through it would insert the
    // declaration BETWEEN a comment and the thing it describes, which is the
    // damage §22.13 exists to prevent, arrived at from the other side.
    let src = "\
# Build the app.

# The release binary.
recipe app
    cook_cc.bin({})
";
    assert_eq!(
        inserted(ensure_use(src, "cook_cc").unwrap()),
        "\
# Build the app.
use cook_cc

# The release binary.
recipe app
    cook_cc.bin({})
"
    );
}

#[test]
fn a_use_group_is_joined_across_the_blank_line_below_the_header() {
    // A blank line between `use` lines separates nothing: they are one group
    // however the author spaced them. Stopping at it would put the second
    // install's declaration above the first's, in argv order reversed.
    let src = "\
# Build the app.

use cook_pnpm

recipe app
    cook_cc.bin({})
";
    assert_eq!(
        inserted(ensure_use(src, "cook_cc").unwrap()),
        "\
# Build the app.

use cook_pnpm
use cook_cc

recipe app
    cook_cc.bin({})
"
    );
}

#[test]
fn a_file_opening_on_a_declaration_gets_the_use_at_byte_zero() {
    // Offset 0 with a blank line after it, because `use cook_cc` sitting flush
    // against `recipe app` is not how anyone writes a Cookfile.
    let src = "recipe app\n    cook_cc.bin({})\n";
    assert_eq!(
        inserted(ensure_use(src, "cook_cc").unwrap()),
        "use cook_cc\n\nrecipe app\n    cook_cc.bin({})\n"
    );
}

#[test]
fn an_existing_bare_declaration_is_already_present() {
    assert_eq!(ensure_use(APP, "cook_cc").unwrap(), UseEdit::AlreadyPresent);
}

#[test]
fn an_existing_quoted_declaration_is_already_present() {
    // `use "cook_cc"` is the same declaration spelled the other legal way. A
    // presence check that missed it would write a second `use` for a module
    // already bound.
    let src = "use \"cook_cc\"\n\nrecipe app\n    cook_cc.bin({})\n";
    assert_eq!(ensure_use(src, "cook_cc").unwrap(), UseEdit::AlreadyPresent);
}

#[test]
fn a_declaration_below_the_leading_run_still_counts_as_present() {
    // Presence is asked of every top-level `use`, not of the run the insert
    // would have joined. The author is allowed to have put it further down.
    let src = "recipe app\n    cook_cc.bin({})\n\nuse cook_cc\n";
    assert_eq!(ensure_use(src, "cook_cc").unwrap(), UseEdit::AlreadyPresent);
}

#[test]
fn a_path_form_binding_the_same_name_is_already_present() {
    // The patched-module workflow §27.1.2 sanctions: the author points the
    // name at their own file. Adding `use cook_cc` beside it binds the alias
    // twice, and the two phases disagree about the winner — the register
    // chunk's last `local` wins, the execute prelude's first binding wins — so
    // the build would load the vendored file in one phase and the installed
    // rock in the other, silently. Presence is about the NAME being bound.
    let src = "use cook_cc \"./vendor/cc.lua\"\n\nrecipe app\n    cook_cc.bin({})\n";
    assert_eq!(ensure_use(src, "cook_cc").unwrap(), UseEdit::AlreadyPresent);
}

#[test]
fn a_path_form_with_a_derived_alias_is_already_present() {
    // No explicit alias, so the name comes from the basename by
    // `cook_contracts::module_binding::derived_alias` — the same rule the
    // loader applies. A second opinion here would collide exactly where the
    // first one did.
    let src = "use ./vendor/cook_cc.lua\n\nrecipe app\n    cook_cc.bin({})\n";
    assert_eq!(ensure_use(src, "cook_cc").unwrap(), UseEdit::AlreadyPresent);
}

#[test]
fn a_path_form_binding_some_other_name_is_not_present() {
    // The alias is what matters, and this one binds `cc`, not `cook_cc`.
    let src = "use cc \"./vendor/cc.lua\"\n\nrecipe app\n    cc.bin({})\n";
    assert_eq!(
        inserted(ensure_use(src, "cook_cc").unwrap()),
        "use cc \"./vendor/cc.lua\"\nuse cook_cc\n\nrecipe app\n    cc.bin({})\n"
    );
}

#[test]
fn a_use_line_inside_a_recipe_body_is_not_a_declaration() {
    // The reason presence is decided over nodes and not over `src.contains`.
    // Inside a step body this is shell text — a comment, a shell function, an
    // `echo` argument — and the grammar agrees: it produces `shell_content`,
    // never a `use_declaration`. A substring check reports the module bound
    // and the Cookfile then fails to load.
    let src = "\
recipe app
    cook \"out\" {
        use cook_cc
        echo hi
    }
";
    assert_eq!(
        inserted(ensure_use(src, "cook_cc").unwrap()),
        "\
use cook_cc

recipe app
    cook \"out\" {
        use cook_cc
        echo hi
    }
"
    );
}

#[test]
fn a_second_module_joins_the_existing_run_in_order() {
    let src = "use cook_cc\n\nrecipe app\n    cook_cc.bin({})\n";
    assert_eq!(
        inserted(ensure_use(src, "cook_pnpm").unwrap()),
        "use cook_cc\nuse cook_pnpm\n\nrecipe app\n    cook_cc.bin({})\n"
    );
}

#[test]
fn the_edit_is_exactly_the_inserted_bytes_and_nothing_else() {
    // The preservation property of §22.13, stated as bytes rather than as a
    // list of things that happened to survive: removing the inserted run from
    // the output must give the input back, character for character. The
    // fixture carries a comment, column alignment and a bare identifier value,
    // each of which a decode/re-encode implementation destroys differently.
    let out = inserted(ensure_use(APP, "cook_pnpm").unwrap());
    let insertion = "use cook_pnpm\n";
    assert_eq!(out.len(), APP.len() + insertion.len());
    assert_eq!(out.replacen(insertion, "", 1), APP);
    assert_eq!(
        out,
        "use cook_cc\nuse cook_pnpm\n".to_string() + &APP["use cook_cc\n".len()..]
    );
}

#[test]
fn applying_the_edit_to_its_own_output_changes_nothing() {
    // A chore that runs twice must not write twice. Idempotency is what makes
    // `ensure_` the right name for the verb.
    for src in [
        "",
        "recipe app\n    cook_cc.bin({})\n",
        "# header\n\nrecipe app\n    cook_cc.bin({})\n",
        "use cook_pnpm\n\nrecipe app\n    cook_cc.bin({})\n",
    ] {
        let once = inserted(ensure_use(src, "cook_cc").unwrap());
        assert_eq!(
            ensure_use(&once, "cook_cc").unwrap(),
            UseEdit::AlreadyPresent,
            "second pass over {once:?} must be a no-op"
        );
    }
}

#[test]
fn a_file_without_a_final_newline_is_not_run_together_with_the_insert() {
    // The one place the insert is not simply `use NAME\n`. Anchoring after a
    // last line the author never terminated would produce `# no newlineuse
    // cook_cc`, which is a comment swallowing the declaration: the file still
    // parses and the module is still unbound.
    let src = "# no trailing newline";
    assert_eq!(
        inserted(ensure_use(src, "cook_cc").unwrap()),
        "# no trailing newline\nuse cook_cc\n"
    );
}

#[test]
fn an_unparseable_cookfile_is_refused_before_any_use_is_added() {
    // Same posture as `splice_into_field`: a syntax error the author already
    // has is never compounded by an insertion landing somewhere arbitrary,
    // and an offset derived from a tree full of ERROR nodes is arbitrary.
    let src = "recipe app\n    cook_cc.bin({ links = { \"a\" }\n";
    assert_eq!(
        ensure_use(src, "cook_cc").unwrap_err(),
        EditError::Unparseable
    );
}

// ---------------------------------------------------------------------------
// `AbsentField::Create` (CS-0221). Two claims run through every case: the mode
// is opt-in, so nothing above this line changes meaning; and creating a field
// is still an insertion of one run of bytes, so the assertions are on the
// whole result and on its length rather than on a substring.
// ---------------------------------------------------------------------------

#[test]
fn creating_is_opt_in_and_refusal_is_still_the_default() {
    // The same call, twice, differing only in the policy. This is the whole of
    // the compatibility claim and it is worth asserting as one test rather
    // than trusting that the tests above cover it by omission.
    let src = "recipe app\n    cook_cc.bin({ sources = { \"src/main.cpp\" } })\n";
    assert_eq!(
        splice_into_field(src, "app", "links", "\"math\"", AbsentField::Refuse).unwrap_err(),
        EditError::FieldNotFound {
            recipe: "app".into(),
            callee: "cook_cc.bin".into(),
            field: "links".into(),
            entry: "\"math\"".into(),
        }
    );
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert_eq!(
        out,
        "recipe app\n    cook_cc.bin({ sources = { \"src/main.cpp\" }, links = { \"math\" } })\n"
    );
}

#[test]
fn a_created_field_is_one_insertion_and_nothing_else_moves() {
    let src = "\
recipe app
    cook_cc.bin({
        sources  = { \"src/main.cpp\" },  -- entry point
        standard = cxx_std,
    })
";
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert!(out.contains("-- entry point"), "got: {out}");
    assert!(out.contains("standard = cxx_std,"), "got: {out}");
    assert_eq!(
        out.len(),
        src.len() + "\n        links = { \"math\" },".len()
    );
}

#[test]
fn a_field_created_in_a_one_entry_per_line_call_takes_its_own_line() {
    // Indented to match the line above it, and carrying the trailing comma
    // that line carries. Appending `, links = { … }` after `cxx_std,` would
    // put two entries on one line in a call the author writes one per line.
    let src = "\
recipe app
    cook_cc.bin({
        sources = { \"src/main.cpp\" },
        standard = cxx_std,
    })
";
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert_eq!(
        out,
        "\
recipe app
    cook_cc.bin({
        sources = { \"src/main.cpp\" },
        standard = cxx_std,
        links = { \"math\" },
    })
"
    );
}

#[test]
fn a_created_field_repeats_the_absence_of_a_trailing_comma() {
    // The author who does not write a trailing comma gets a separator before
    // the new entry and no trailing comma after it. The convention is read off
    // the file rather than imposed.
    let src = "\
recipe app
    cook_cc.bin({
        sources = { \"src/main.cpp\" }
    })
";
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert_eq!(
        out,
        "\
recipe app
    cook_cc.bin({
        sources = { \"src/main.cpp\" },
        links = { \"math\" }
    })
"
    );
}

#[test]
fn a_created_field_anchors_after_the_code_not_after_a_trailing_comment() {
    // The same hazard `last_code_end` exists for, one level up: the last
    // non-whitespace byte of the argument table sits inside the comment.
    let src = "\
recipe app
    cook_cc.bin({
        sources = { \"src/main.cpp\" },  -- see docs/build.md {section 2}
    })
";
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert!(
        out.contains("-- see docs/build.md {section 2}\n        links = { \"math\" },\n"),
        "got: {out}"
    );
}

#[test]
fn a_field_created_in_an_empty_argument_table_needs_no_separator() {
    // Consistent with an empty list, which takes the entry with no leading
    // comma and no invented padding: `{}` has no style to preserve.
    let src = "recipe app\n    cook_cc.headers({})\n";
    let out =
        splice_into_field(src, "app", "includes", "\"include\"", AbsentField::Create).unwrap();
    assert_eq!(
        out,
        "recipe app\n    cook_cc.headers({includes = { \"include\" }})\n"
    );
}

#[test]
fn creating_works_through_the_no_parens_call_form() {
    // `f{ … }` is Lua sugar for `f({ … })` and the argument table is located
    // by its brace, not by a paren.
    let src = "recipe app\n    cook_cc.bin{ sources = { \"a.cpp\" } }\n";
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert_eq!(
        out,
        "recipe app\n    cook_cc.bin{ sources = { \"a.cpp\" }, links = { \"math\" } }\n"
    );
}

#[test]
fn creating_does_not_relax_a_field_that_is_present_and_not_a_list() {
    // The field is there. Creating a second one would leave the call with two
    // keys of the same name, the later silently winning, and the author's
    // value discarded without a word.
    let src = "recipe app\n    cook_cc.bin({ links = \"mathlib\" })\n";
    assert_eq!(
        splice_into_field(src, "app", "links", "\"b\"", AbsentField::Create).unwrap_err(),
        EditError::FieldNotAList {
            recipe: "app".into(),
            callee: "cook_cc.bin".into(),
            field: "links".into(),
        }
    );
}

#[test]
fn creating_does_not_relax_a_missing_recipe_or_a_missing_call() {
    let src = "recipe app\n    cook \"out\" { echo hi > $<out> }\n";
    assert_eq!(
        splice_into_field(src, "ghost", "links", "\"b\"", AbsentField::Create).unwrap_err(),
        EditError::RecipeNotFound {
            recipe: "ghost".into()
        }
    );
    assert_eq!(
        splice_into_field(src, "app", "links", "\"b\"", AbsentField::Create).unwrap_err(),
        EditError::NoModuleCall {
            recipe: "app".into()
        }
    );
}

#[test]
fn a_call_with_no_argument_table_has_nowhere_to_put_a_field() {
    // Nothing to add a key to, and inventing `({ links = { … } })` around the
    // author's argument would change what the call is passed.
    let src = "recipe app\n    cook_cc.bin(\"src/main.cpp\")\n";
    assert_eq!(
        splice_into_field(src, "app", "links", "\"b\"", AbsentField::Create).unwrap_err(),
        EditError::NoArgumentTable {
            recipe: "app".into(),
            callee: "cook_cc.bin".into(),
            field: "links".into(),
            entry: "\"b\"".into(),
        }
    );
}

#[test]
fn a_created_field_is_ordinary_input_to_the_next_splice() {
    // `cc.link game math` then `cc.link game sound`: the second call finds a
    // field the first one wrote and takes the append path.
    let src = "recipe app\n    cook_cc.bin({ sources = { \"a.cpp\" } })\n";
    let once = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    let twice = splice_into_field(&once, "app", "links", "\"sound\"", AbsentField::Create).unwrap();
    assert_eq!(
        twice,
        "recipe app\n    cook_cc.bin({ sources = { \"a.cpp\" }, links = { \"math\", \"sound\" } })\n"
    );
}

#[test]
fn a_commented_out_field_is_created_rather_than_revived() {
    // §22.13 already says a commented-out `links` is not the field. It follows
    // that creating one is the right answer here, and that the author's
    // comment is left exactly where it is.
    let src =
        "recipe app\n    cook_cc.bin({ sources = { \"a.cpp\" } --[[ links = { \"old\" } ]] })\n";
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert!(out.contains("--[[ links = { \"old\" } ]]"), "got: {out}");
    assert!(
        out.contains("{ \"a.cpp\" }, links = { \"math\" } --[[ links"),
        "anchored after the code, before the comment: {out}"
    );
}

#[test]
fn an_unparseable_cookfile_is_refused_before_a_field_is_created() {
    let src = "recipe app\n    cook_cc.bin({ sources = { \"a.cpp\" }\n";
    assert_eq!(
        splice_into_field(src, "app", "links", "\"b\"", AbsentField::Create).unwrap_err(),
        EditError::Unparseable
    );
}

// ---------------------------------------------------------------------------
// `field_entries` (CS-0221). A read, so every case asserts what comes back and
// nothing asserts a write. It exists so a verb can answer "is `math` already
// in `links`" without a scanner of its own; `text.find("\"math\"")` matches
// inside `sources = { "src/math/main.cpp" }`, which is the bug it prevents.
// ---------------------------------------------------------------------------

#[test]
fn field_entries_returns_the_entries_verbatim_and_in_order() {
    let src = "recipe app\n    cook_cc.bin({ links = { \"math\", 'sound', util } })\n";
    assert_eq!(
        field_entries(src, "app", "links").unwrap(),
        Some(vec![
            "\"math\"".to_string(),
            "'sound'".to_string(),
            "util".to_string()
        ])
    );
}

#[test]
fn field_entries_does_not_confuse_an_entry_with_a_path_that_contains_it() {
    let src = "recipe app\n    cook_cc.bin({ sources = { \"src/math/main.cpp\" }, links = {} })\n";
    assert_eq!(field_entries(src, "app", "links").unwrap(), Some(vec![]));
}

#[test]
fn field_entries_trims_comments_and_survives_a_trailing_comma() {
    let src = "\
recipe app
    cook_cc.bin({
        links = {
            \"math\",   -- the one that matters
            -- \"sound\",
            \"util\",
        },
    })
";
    assert_eq!(
        field_entries(src, "app", "links").unwrap(),
        Some(vec!["\"math\"".to_string(), "\"util\"".to_string()])
    );
}

#[test]
fn field_entries_keeps_a_comma_that_is_inside_an_entry() {
    // A table entry and a call argument both hold commas that do not separate
    // entries of THIS list.
    let src = "recipe app\n    cook_cc.bin({ links = { { \"a\", \"b\" }, f(1, 2) } })\n";
    assert_eq!(
        field_entries(src, "app", "links").unwrap(),
        Some(vec!["{ \"a\", \"b\" }".to_string(), "f(1, 2)".to_string()])
    );
}

#[test]
fn field_entries_is_nil_for_a_field_a_create_would_have_to_write() {
    // The same question `AbsentField::Create` answers, asked without writing:
    // nil means there is nothing there yet.
    let src = "recipe app\n    cook_cc.bin({ sources = { \"a.cpp\" } })\n";
    assert_eq!(field_entries(src, "app", "links").unwrap(), None);
}

#[test]
fn field_entries_is_nil_for_a_missing_recipe_or_a_missing_call() {
    // `find_call`'s rule: nil is "nothing here to report on", and the caller's
    // next move is the same in all three cases.
    let src = "recipe app\n    cook \"out\" { echo hi > $<out> }\n";
    assert_eq!(field_entries(src, "ghost", "links").unwrap(), None);
    assert_eq!(field_entries(src, "app", "links").unwrap(), None);
}

#[test]
fn field_entries_refuses_a_field_that_is_not_a_list() {
    // Not nil: the field IS there, and a caller told otherwise would go on to
    // create a second one.
    let src = "recipe app\n    cook_cc.bin({ links = \"mathlib\" })\n";
    assert_eq!(
        field_entries(src, "app", "links").unwrap_err(),
        EditError::FieldNotAList {
            recipe: "app".into(),
            callee: "cook_cc.bin".into(),
            field: "links".into(),
        }
    );
}

#[test]
fn creating_refuses_beside_a_bracketed_key_of_the_same_name() {
    // `["links"]` and `links` are one key. §22.13 lets an implementation
    // decline to match the bracketed spelling and report the field as not
    // found, which is a fine answer when the consequence is a refusal and a
    // bad one when the consequence is writing a second key that wins.
    let src = "recipe app\n    cook_cc.bin({ [\"links\"] = { \"mathlib\" } })\n";
    assert_eq!(
        splice_into_field(src, "app", "links", "\"b\"", AbsentField::Create).unwrap_err(),
        EditError::BracketedField {
            recipe: "app".into(),
            callee: "cook_cc.bin".into(),
            field: "links".into(),
            entry: "\"b\"".into(),
        }
    );
    // The refusing policy is untouched: it still reports the field as not
    // found, which is what §22.13 requires of it.
    assert!(matches!(
        splice_into_field(src, "app", "links", "\"b\"", AbsentField::Refuse).unwrap_err(),
        EditError::FieldNotFound { .. }
    ));
}

#[test]
fn a_bracketed_key_of_another_name_does_not_block_creating() {
    // The check is about the key being created, not about bracketed keys in
    // general — refusing on any of them would make one unusual spelling
    // anywhere in the call disable the verb.
    let src = "recipe app\n    cook_cc.bin({ [\"standard\"] = \"c++20\" })\n";
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert_eq!(
        out,
        "recipe app\n    cook_cc.bin({ [\"standard\"] = \"c++20\", links = { \"math\" } })\n"
    );
}

// ---------------------------------------------------------------------------
// Defects found by review of the first CS-0221 draft. Each is a case where an
// edit reported success and left a file that was wrong, which is the one
// outcome this crate exists to make impossible.
// ---------------------------------------------------------------------------

#[test]
fn a_long_comment_on_the_anchor_line_does_not_swallow_the_created_field() {
    // The next `\n` after the code anchor is INSIDE the author's `--[[ … ]]`,
    // so a field placed there vanishes into the comment: the call is
    // unchanged, the edit reports success, and a second run stacks another
    // line in the same comment.
    let src = "\
recipe app
    cook_cc.bin({
        sources = { \"a.cpp\" }, --[[ note
        more ]]
    })
";
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert!(
        out.contains("more ]]\n        links = { \"math\" },\n"),
        "got: {out}"
    );
    // The claim, stated as the thing a caller depends on: what was written can
    // be read back.
    assert_eq!(
        field_entries(&out, "app", "links").unwrap(),
        Some(vec!["\"math\"".to_string()])
    );
}

#[test]
fn a_semicolon_separated_table_keeps_its_separator() {
    // Lua admits `;` between table entries and `find_field_key` has always
    // accepted it as one. Appending a comma after it produces `{ a = 1;, b }`,
    // which does not load.
    let created = splice_into_field(
        "recipe app\n    cook_cc.bin({ sources = { \"a.cpp\" }; })\n",
        "app",
        "links",
        "\"math\"",
        AbsentField::Create,
    )
    .unwrap();
    assert_eq!(
        created,
        "recipe app\n    cook_cc.bin({ sources = { \"a.cpp\" }; links = { \"math\" } })\n"
    );

    // The same hazard on the append path, which had it before this entry.
    let appended = splice_into_field(
        "recipe app\n    cook_cc.bin({ links = { \"a\"; } })\n",
        "app",
        "links",
        "\"b\"",
        AbsentField::Refuse,
    )
    .unwrap();
    assert_eq!(
        appended,
        "recipe app\n    cook_cc.bin({ links = { \"a\"; \"b\" } })\n"
    );

    // And the read agrees with both: a `;` is a separator, not part of an
    // entry, so an idempotence check does not compare `"a";` against `"a"`.
    assert_eq!(
        field_entries(
            "recipe app\n    cook_cc.bin({ links = { \"a\"; \"b\" } })\n",
            "app",
            "links"
        )
        .unwrap(),
        Some(vec!["\"a\"".to_string(), "\"b\"".to_string()])
    );
}

#[test]
fn a_created_field_repeats_a_semicolon_convention_on_its_own_line() {
    let src = "\
recipe app
    cook_cc.bin({
        sources = { \"a.cpp\" };
    })
";
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert!(
        out.contains("\n        links = { \"math\" };\n"),
        "got: {out}"
    );
}

#[test]
fn a_name_that_cannot_be_written_as_a_key_is_refused_rather_than_written() {
    // `find_field_key` already refuses an empty name outright, so without this
    // check the same input would fail cleanly when refusing and silently write
    // ` = { "math" }` when creating. Same posture CS-0220 took for a use name.
    let src = "recipe app\n    cook_cc.bin({ sources = { \"a.cpp\" } })\n";
    for field in ["", "a.b", "end", "my links", "2fast"] {
        assert_eq!(
            splice_into_field(src, "app", field, "\"math\"", AbsentField::Create).unwrap_err(),
            EditError::UnspellableField {
                field: field.to_string()
            },
            "field {field:?} must be refused"
        );
    }
    // A key that merely CONTAINS a reserved word is fine.
    assert!(splice_into_field(src, "app", "ending", "\"math\"", AbsentField::Create).is_ok());
}

#[test]
fn a_crlf_file_keeps_its_line_endings() {
    // "Preservation is normative" is the section's headline claim, and a bare
    // `\n` line inserted into a CRLF file is an unannounced change to the
    // file's convention.
    let src = "recipe app\r\n    cook_cc.bin({\r\n        sources = { \"a.cpp\" },\r\n    })\r\n";
    let out = splice_into_field(src, "app", "links", "\"math\"", AbsentField::Create).unwrap();
    assert_eq!(
        out,
        "recipe app\r\n    cook_cc.bin({\r\n        sources = { \"a.cpp\" },\r\n        links = { \"math\" },\r\n    })\r\n"
    );
    assert!(
        !out.contains("\n\n"),
        "no bare newline was introduced: {out:?}"
    );
}
