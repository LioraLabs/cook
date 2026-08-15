use super::*;
// ---------------------------------------------------------------------------
// identifier_occurs (CS-0205)
// ---------------------------------------------------------------------------

#[test]
fn identifier_occurs_finds_a_bare_reference() {
    assert!(free_identifier_occurs("print(greet.say(\"x\"))", "greet"));
}

#[test]
fn identifier_occurs_is_word_bounded() {
    assert!(!free_identifier_occurs("greeting.say()", "greet"));
    assert!(!free_identifier_occurs("mygreet.say()", "greet"));
    assert!(!free_identifier_occurs("_greet.say()", "greet"));
    assert!(!free_identifier_occurs("greet2.say()", "greet"));
}

#[test]
fn identifier_occurs_ignores_strings_and_comments() {
    assert!(!free_identifier_occurs("print(\"greet\")", "greet"));
    assert!(!free_identifier_occurs("print('greet')", "greet"));
    assert!(!free_identifier_occurs("print([[greet]])", "greet"));
    assert!(!free_identifier_occurs("-- greet\nprint(1)", "greet"));
    assert!(!free_identifier_occurs("--[[ greet ]] print(1)", "greet"));
}

#[test]
fn identifier_occurs_after_a_string_still_matches() {
    // The scanner must resume scanning code AFTER a literal, not stop at it.
    assert!(free_identifier_occurs("print(\"hello\") greet.say()", "greet"));
}

#[test]
fn identifier_occurs_skips_a_field_or_method_access() {
    // `t.greet` and `t:greet()` name a FIELD, never the local. Binding the
    // module for them is not a harmless extra load: it evaluates the module on
    // a worker VM for a body that never asked, which hard-fails for a
    // register-oriented module — the exact scenario CS-0205 gates to avoid.
    assert!(!free_identifier_occurs("t.greet = 1", "greet"));
    assert!(!free_identifier_occurs("t:greet()", "greet"));
    assert!(!free_identifier_occurs("local o = {} o.greet = 1 print(o.greet)", "greet"));
}

#[test]
fn identifier_occurs_is_not_fooled_by_concatenation() {
    // `..` is two dots, and the second one is NOT a field-access dot. Missing
    // this is a false NEGATIVE — the direction that reinstates the defect —
    // so it is pinned separately from the field-access case above.
    assert!(free_identifier_occurs("x .. greet.f()", "greet"));
    assert!(free_identifier_occurs("\"pre\"..greet.f()", "greet"));
    assert!(free_identifier_occurs("a.b..greet.f()", "greet"));
}

#[test]
fn identifier_occurs_stops_at_an_unterminated_literal() {
    // Same halt the other scanners take: past an unterminated literal there is
    // no honest answer, and guessing invents matches.
    assert!(!free_identifier_occurs("print(\"open [[ greet", "greet"));
}

#[test]
fn identifier_occurs_is_false_for_an_empty_body() {
    assert!(!free_identifier_occurs("", "greet"));
}
