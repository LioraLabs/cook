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

#[test]
fn a_subtraction_expression_is_not_read_as_a_comment() {
    // A single `-` must not start comment mode; `n-1` inside a table is legal
    // Lua and the scan has to keep counting braces through it.
    let src = "recipe app\n    cook_cc.bin({ jobs = { n-1 }, links = { \"a\" } })\n";
    let out = splice_into_field(src, "app", "links", "\"b\"").unwrap();
    assert!(out.contains("{ n-1 }"), "expression untouched: {out}");
    assert!(out.contains("links = { \"a\", \"b\" }"), "got: {out}");
}
