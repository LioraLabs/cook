use super::*;

fn uses(names: &[&str]) -> Vec<UseStatement> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| UseStatement {
            module_name: (*n).to_string(),
            line: i + 1,
        })
        .collect()
}

#[test]
fn alias_rewrites_hyphens_to_underscores() {
    assert_eq!(alias_of("my-mod"), "my_mod");
    assert_eq!(alias_of("plain"), "plain");
}

#[test]
fn no_uses_means_no_prelude() {
    assert_eq!(execute_prelude(&[], "greet.say()"), "");
}

#[test]
fn an_unreferenced_alias_is_not_bound() {
    // §12.1's visibility obligation is discharged either way: a local the body
    // never names is unobservable. Not binding it also keeps the module out of
    // the unit's CS-0204 determinant set.
    assert_eq!(execute_prelude(&uses(&["greet"]), "print(1)"), "");
}

#[test]
fn a_referenced_alias_is_bound_on_one_line() {
    assert_eq!(
        execute_prelude(&uses(&["greet"]), "greet.say(\"x\")"),
        "local greet = cook.load_module(\"greet\"); "
    );
}

#[test]
fn the_prelude_never_ends_in_a_newline() {
    // The whole point of the single-line, glued spelling: the body's first line
    // stays line 1 of the chunk, so no emission site has to compensate.
    let p = execute_prelude(&uses(&["greet"]), "greet.say()");
    assert!(!p.contains('\n'), "prelude must cost zero lines: {p:?}");
}

#[test]
fn referenced_aliases_bind_in_declaration_order() {
    assert_eq!(
        execute_prelude(&uses(&["a", "b"]), "b.f() a.g()"),
        "local a = cook.load_module(\"a\"); local b = cook.load_module(\"b\"); "
    );
}

#[test]
fn only_the_referenced_subset_binds() {
    assert_eq!(
        execute_prelude(&uses(&["a", "b"]), "b.f()"),
        "local b = cook.load_module(\"b\"); "
    );
}

#[test]
fn a_hyphenated_module_binds_the_underscore_alias_and_the_disk_name() {
    // Note 12.1.1: the alias is rewritten, the on-disk name is not.
    assert_eq!(
        execute_prelude(&uses(&["my-mod"]), "my_mod.f()"),
        "local my_mod = cook.load_module(\"my-mod\"); "
    );
}

#[test]
fn a_repeated_use_binds_once() {
    assert_eq!(
        execute_prelude(&uses(&["a", "a"]), "a.f()"),
        "local a = cook.load_module(\"a\"); "
    );
}

#[test]
fn the_module_name_is_escaped() {
    assert_eq!(
        execute_prelude(&uses(&["q\"x"]), "q_x.f()"),
        String::new(),
        "an alias that is not a Lua identifier cannot be referenced, so nothing binds"
    );
}

#[test]
fn with_execute_prelude_glues_the_body_onto_the_same_line() {
    assert_eq!(
        with_execute_prelude(&uses(&["greet"]), "greet.say()\nmore()\n"),
        "local greet = cook.load_module(\"greet\"); greet.say()\nmore()\n"
    );
}

#[test]
fn with_execute_prelude_leaves_an_unreferencing_body_byte_identical() {
    assert_eq!(with_execute_prelude(&uses(&["greet"]), "print(1)"), "print(1)");
    assert_eq!(with_execute_prelude(&[], "print(1)"), "print(1)");
}
