use super::{bare_key_error, is_segment, is_tool_name, is_valid_bare};

#[test]
fn a_single_segment_is_a_key() {
    assert!(is_valid_bare("host"));
    assert!(is_valid_bare("_private"));
    assert!(is_valid_bare("cc2"));
}

/// COOK-408: the case that was declarable, sigil-referenceable, and neither
/// sealable nor consumable.
#[test]
fn hyphens_are_admitted_in_every_segment() {
    assert!(is_valid_bare("cc-version"));
    assert!(is_valid_bare("demo:cc-version"));
    assert!(is_valid_bare("a-b:c-d:e-f"));
}

/// The cap was enforced on the surface declaration and ignored by
/// `cook.probe()`, so `cc:find:raylib` is the flagship module's ordinary case
/// and could not be sealed.
#[test]
fn there_is_no_segment_cap() {
    assert!(is_valid_bare("cc:find:raylib"));
    assert!(is_valid_bare("cc:compiler:auto"));
    assert!(is_valid_bare("a:b:c:d:e"));
}

/// `.` is member access in a sigil. Admitting it inside a segment makes
/// `$<demo:cc-version.ver>` ambiguous between "field ver of demo:cc-version"
/// and "the key demo:cc-version.ver", with no way to tell from the text.
#[test]
fn dots_are_not_admitted() {
    assert!(!is_valid_bare("cc.version"));
    assert!(!is_valid_bare("demo:cc.version"));
    assert!(!is_segment("a.b"));
}

#[test]
fn a_segment_needs_an_alpha_or_underscore_head() {
    assert!(!is_valid_bare("9lives"));
    assert!(!is_valid_bare("-leading"));
    assert!(!is_valid_bare("cc:9lives"));
}

#[test]
fn empty_segments_are_rejected() {
    assert!(!is_valid_bare(""));
    assert!(!is_valid_bare(":"));
    assert!(!is_valid_bare("a:"));
    assert!(!is_valid_bare(":a"));
    assert!(!is_valid_bare("a::b"));
}

#[test]
fn spellings_outside_the_grammar_are_rejected_and_belong_in_quotes() {
    assert!(!is_valid_bare("g++"));
    assert!(!is_valid_bare("a b"));
    assert!(!is_valid_bare("a/b"));
}

/// `TOOL_NAME` was `PROBE_SEG` (CS-0181), conflating "a segment of a key" with
/// "the name of an executable". Only the first has the member-access
/// ambiguity, and only the second needs a dot.
#[test]
fn tool_names_keep_the_dot_that_probe_segments_lose() {
    assert!(is_tool_name("python3.11"));
    assert!(is_tool_name("clang-15"));
    assert!(is_tool_name("cc"));
    assert!(!is_valid_bare("python3.11"));

    // But a tool name is still a name, not an arbitrary string.
    assert!(!is_tool_name("cc --version"));
    assert!(!is_tool_name("9lives"));
}

/// The diagnostic must name the dot specifically, because "malformed key" over
/// a spelling the declaration accepts reads as a typo rather than a rule.
#[test]
fn the_diagnostic_explains_a_dot_rather_than_just_refusing_it() {
    let e = bare_key_error("seal", "cc.version");
    assert!(e.contains("member access"), "{e}");
    assert!(e.contains("quoted form"), "{e}");

    let plain = bare_key_error("seal", "g++");
    assert!(!plain.contains("member access"), "{plain}");
    assert!(plain.contains("quoted form"), "{plain}");
}
