//! Cookfile editing contract (CS-0179).
//!
//! The property under test throughout is preservation: an edit inserts bytes
//! and changes nothing else. Most assertions therefore check what did NOT
//! move, which is the half a decode/re-encode implementation would fail.

use cook_cookfile::{append_declaration, find_call, splice_into_field, EditError};

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
