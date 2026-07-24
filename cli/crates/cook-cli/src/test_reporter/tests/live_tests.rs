use super::*;

#[test]
fn ok_line_uncolored() {
    let s = Style::new(false);
    assert_eq!(
        outcome_line("pass_basic@5", Outcome::Ok, false, false, &s),
        "test pass_basic@5 ... ok"
    );
}

#[test]
fn ok_cached_modifier() {
    let s = Style::new(false);
    assert_eq!(
        outcome_line("c::r", Outcome::Ok, true, false, &s),
            "test c::r ... ok (cached)"
    );
}

#[test]
fn ok_should_fail_modifier() {
    let s = Style::new(false);
    assert_eq!(
        outcome_line("p::sf", Outcome::Ok, false, true, &s),
        "test p::sf ... ok (should-fail)"
    );
}

#[test]
fn failed_no_modifier_even_if_should_fail() {
    let s = Style::new(false);
    assert_eq!(
        outcome_line("x@3", Outcome::Failed, false, true, &s),
        "test x@3 ... FAILED"
    );
}

#[test]
fn cached_does_not_apply_to_non_ok() {
    // BLOCKED tests can't be "cached"; ensure no spurious modifier.
    let s = Style::new(false);
    assert_eq!(
        outcome_line("b@1", Outcome::Blocked, true, false, &s),
        "test b@1 ... BLOCKED"
    );
}

#[test]
fn colored_ok_wraps_verb() {
    let s = Style::new(true);
    let line = outcome_line("p@1", Outcome::Ok, false, false, &s);
    assert!(line.contains("\x1b[32mok\x1b[0m"), "got: {line}");
}
