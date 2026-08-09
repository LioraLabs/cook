//! Cookfile editing contract (CS-0179).
//!
//! The property under test throughout is preservation: an edit inserts bytes
//! and changes nothing else. Most assertions therefore check what did NOT
//! move, which is the half a decode/re-encode implementation would fail.

use cook_cookfile::{append_declaration, ensure_use, find_call, splice_into_field, EditError, UseEdit};

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
    let out = splice_into_field(APP, "app", "links", "\"physlib\"").unwrap();

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
    let out = splice_into_field(src, "app", "links", "\"mathlib\"").unwrap();
    assert!(out.contains("{ links = {\"mathlib\"} }"), "got: {out}");
    assert!(!out.contains(", \"mathlib\""), "no leading comma on an empty list");
}

#[test]
fn preserves_interior_padding_rather_than_inserting_before_the_brace() {
    // Inserting immediately before `}` would yield `{ "a", "b"}` and quietly
    // restyle the author's line. The anchor is the last non-whitespace byte.
    let src = "recipe app\n    cook_cc.bin({ links = { \"a\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"").unwrap();
    assert!(out.contains("{ \"a\", \"b\" }"), "got: {out}");
}

#[test]
fn a_brace_inside_a_string_does_not_close_the_list() {
    // The reason field location is a quote-aware scan and not `find('}')`.
    // Getting this wrong splices into the middle of a string literal.
    let src = "recipe app\n    cook_cc.bin({ sources = { \"src/a}b.cpp\" } })\n";
    let out = splice_into_field(src, "app", "sources", "\"src/c.cpp\"").unwrap();
    assert!(out.contains("{ \"src/a}b.cpp\", \"src/c.cpp\" }"), "got: {out}");
}

#[test]
fn a_field_name_appearing_inside_another_token_is_not_matched() {
    // `find("links")` would hit `mathlinks` first and splice into `sources`.
    let src = "recipe app\n    cook_cc.bin({ sources = { \"mathlinks.cpp\" }, links = { \"m\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"n\"").unwrap();
    assert!(out.contains("{ \"mathlinks.cpp\" }"), "sources untouched: {out}");
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
    let out = splice_into_field(src, "beta", "links", "\"z\"").unwrap();
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
    let out = splice_into_field(src, "app", "links", "\"b\"").unwrap();
    assert!(out.contains("links   = { \"a\", \"b\" }"), "got: {out}");
    assert!(out.contains("warnings = { \"all\", \"extra\" }"), "nested table untouched");
}

// ---------------------------------------------------------------------------
// Honest degradation. Each of these is a case where a lossy re-render would
// silently succeed and write something the author did not ask for.
// ---------------------------------------------------------------------------

#[test]
fn missing_recipe_is_named() {
    let err = splice_into_field(APP, "nope", "links", "\"x\"").unwrap_err();
    assert_eq!(err, EditError::RecipeNotFound { recipe: "nope".into() });
    assert!(err.to_string().contains("no recipe named 'nope'"));
}

#[test]
fn recipe_without_a_module_call_is_named() {
    let src = "recipe app\n    cook \"out\" { echo hi > $<out> }\n";
    let err = splice_into_field(src, "app", "links", "\"x\"").unwrap_err();
    assert_eq!(err, EditError::NoModuleCall { recipe: "app".into() });
}

#[test]
fn missing_field_reports_the_manual_fix() {
    let src = "recipe app\n    cook_cc.bin({ sources = { \"a.cpp\" } })\n";
    let err = splice_into_field(src, "app", "links", "\"mathlib\"").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("couldn't find 'links'"), "got: {msg}");
    assert!(msg.contains("cook_cc.bin"), "names the call: {msg}");
    assert!(msg.contains("add \"mathlib\" to it manually"), "actionable: {msg}");
}

#[test]
fn a_field_that_is_not_a_list_is_refused_rather_than_mangled() {
    let src = "recipe app\n    cook_cc.bin({ links = shared_links })\n";
    let err = splice_into_field(src, "app", "links", "\"x\"").unwrap_err();
    assert!(matches!(err, EditError::FieldNotAList { .. }), "got: {err:?}");
    assert!(err.to_string().contains("is not a `{ ... }` list"));
}

#[test]
fn an_unparseable_cookfile_is_refused_before_any_edit() {
    let src = "recipe app\n    cook_cc.bin({ links = { \"a\" }\n";
    assert_eq!(
        splice_into_field(src, "app", "links", "\"b\"").unwrap_err(),
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
    let out = splice_into_field(src, "app", "links", "\"physlib\"").unwrap();
    assert!(
        out.contains("-- see docs/build.md {section 2}\n"),
        "comment must survive untouched: {out}"
    );
    // Anchored after the trailing comma — which already separates, so no
    // second comma is added — and ahead of the comment, which stays put.
    assert!(out.contains("\"mathlib\", \"physlib\"   -- see docs/build.md {section 2}"),
        "got: {out}");
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
    let out = splice_into_field(src, "app", "links", "\"b\"").unwrap();
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
    let out = splice_into_field(src, "app", "links", "\"b\"").unwrap();
    assert!(out.contains("{ \"links=1\" }"), "defines untouched: {out}");
    assert!(out.contains("links = { \"a\", \"b\" }"), "got: {out}");
}

#[test]
fn a_nested_field_of_the_same_name_is_not_edited() {
    // The nested list is a different field of a different table. Editing it
    // still produces a file that parses and looks plausible, which is why this
    // is the one that would have survived review.
    let src = "recipe app\n    cook_cc.bin({ opts = { links = { \"x\" } }, links = { \"a\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"").unwrap();
    assert!(out.contains("opts = { links = { \"x\" } }"), "nested untouched: {out}");
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
    let err = splice_into_field(src, "app", "links", "\"b\"").unwrap_err();
    assert!(matches!(err, EditError::FieldNotFound { .. }), "got: {err:?}");
}

#[test]
fn a_field_that_exists_only_nested_is_reported_missing() {
    // Failure is explicit and total (§22.13): being told to make the edit by
    // hand beats an edit to a list the caller did not name.
    let src = "recipe app\n    cook_cc.bin({ opts = { links = { \"x\" } } })\n";
    let err = splice_into_field(src, "app", "links", "\"b\"").unwrap_err();
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
    let out = splice_into_field(src, "app", "links", "\"d\"").unwrap();
    assert!(out.contains("{ [[a}b]], \"c\", \"d\" }"), "got: {out}");
}

#[test]
fn a_levelled_long_bracket_is_understood_at_any_level() {
    // `[=[ … ]=]` is the spelling an author reaches for precisely when the
    // text contains `]]`, so it is the one most likely to hold odd bytes.
    let src = "recipe app\n    cook_cc.bin({ links = { [=[a}b]=], \"c\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"d\"").unwrap();
    assert!(out.contains("{ [=[a}b]=], \"c\", \"d\" }"), "got: {out}");
}

#[test]
fn a_long_bracket_comment_does_not_swallow_the_rest_of_its_line() {
    // `--[[ … ]]` ends at `]]`, not at the newline. Reading it as a line
    // comment eats the real `}` that follows it on the same line, and the
    // field then reads as unterminated.
    let src = "recipe app\n    cook_cc.bin({ links = { \"a\" --[[ why } ]] } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"").unwrap();
    assert!(out.contains("{ \"a\", \"b\" --[[ why } ]] }"), "got: {out}");
}

#[test]
fn two_dashes_inside_a_long_bracket_do_not_open_a_comment() {
    // The anchor retracts to before a `--` because a comment must not be
    // spliced into. Inside a string literal there is no comment to protect,
    // and retracting there splices into the string instead.
    let src = "recipe app\n    cook_cc.bin({ links = { [[note -- x]] } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"").unwrap();
    assert!(out.contains("{ [[note -- x]], \"b\" }"), "got: {out}");
}

#[test]
fn a_subtraction_expression_is_not_read_as_a_comment() {
    // A single `-` must not start comment mode; `n-1` inside a table is legal
    // Lua and the scan has to keep counting braces through it.
    let src = "recipe app\n    cook_cc.bin({ jobs = { n-1 }, links = { \"a\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"").unwrap();
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
fn the_path_form_does_not_satisfy_a_module_name() {
    // `use cook_cc "./vendor/cook_cc.lua"` binds the name to a file, and the
    // grammar records `cook_cc` there as the ALIAS, not the module. Reading it
    // as the module would report a module that was never named.
    let src = "use cook_cc \"./vendor/cc.lua\"\n\nrecipe app\n    cook_cc.bin({})\n";
    assert_eq!(
        inserted(ensure_use(src, "cook_cc").unwrap()),
        "use cook_cc \"./vendor/cc.lua\"\nuse cook_cc\n\nrecipe app\n    cook_cc.bin({})\n"
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
    assert_eq!(out, "use cook_cc\nuse cook_pnpm\n".to_string() + &APP["use cook_cc\n".len()..]);
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
    assert_eq!(ensure_use(src, "cook_cc").unwrap_err(), EditError::Unparseable);
}
