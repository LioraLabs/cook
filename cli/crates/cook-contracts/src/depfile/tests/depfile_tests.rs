use super::{parse_prerequisites, DepfileSyntax};

/// Everything before the first `:` is the target the compiler was asked to
/// produce, not something the target depends on. A parser that kept it would
/// key every object file on itself.
#[test]
fn the_target_before_the_colon_is_not_a_prerequisite() {
    let got = parse_prerequisites("build/a.o: src/a.c include/a.h\n", "").expect("parses");
    assert_eq!(got, vec!["src/a.c", "include/a.h"]);
}

/// `-MMD` wraps long prerequisite lists. A backslash-newline is a line
/// continuation, not a token, and the tokens either side of it are separate.
#[test]
fn a_backslash_newline_joins_a_continuation() {
    let got = parse_prerequisites(
        "build/a.o: src/a.c \\\n  include/a.h \\\n  include/b.h\n",
        "src/a.c",
    )
    .expect("parses");
    assert_eq!(got, vec!["include/a.h", "include/b.h"]);
}

/// The CRLF form is processed first so the trailing `\r` never leaks into the
/// token that precedes it. Without that ordering the last prerequisite on
/// every continued line of a Windows-authored depfile is a path with an
/// invisible carriage return glued to it, which then exists on no filesystem.
#[test]
fn a_backslash_crlf_joins_a_continuation_without_leaking_the_carriage_return() {
    let got = parse_prerequisites(
        "build/a.o: src/a.c \\\r\n  include/a.h \\\r\n  include/b.h\r\n",
        "src/a.c",
    )
    .expect("parses");
    assert_eq!(got, vec!["include/a.h", "include/b.h"]);
}

/// An absolute prerequisite is a system header. It is outside the project,
/// nothing in the build produces it, and folding it into a unit's inputs would
/// key the cache on the machine rather than on the source.
#[test]
fn an_absolute_prerequisite_is_not_a_project_input() {
    let got = parse_prerequisites(
        "build/a.o: src/a.c /usr/include/stdio.h include/a.h\n",
        "src/a.c",
    )
    .expect("parses");
    assert_eq!(got, vec!["include/a.h"]);
}

/// The compiler names the translation unit among its own prerequisites. The
/// caller already declared it as an input, so admitting it again would record
/// it twice.
#[test]
fn a_source_does_not_depend_on_itself() {
    let got =
        parse_prerequisites("build/a.o: src/a.c include/a.h src/a.c\n", "src/a.c").expect("parses");
    assert_eq!(got, vec!["include/a.h"]);
}

/// A caller with no source to exclude passes the empty string, and then
/// nothing is excluded — the empty string must not match a token by accident.
#[test]
fn an_empty_source_path_disables_the_self_skip() {
    let got = parse_prerequisites("build/a.o: src/a.c\n", "").expect("parses");
    assert_eq!(got, vec!["src/a.c"]);
}

/// One path reached twice is one prerequisite. A unit's recorded input set is
/// compared element-wise, so a duplicate is an input-set change on the next
/// compiler that happens not to repeat itself.
#[test]
fn one_path_named_twice_is_one_prerequisite() {
    let got = parse_prerequisites("build/a.o: src/a.c include/a.h include/a.h\n", "src/a.c")
        .expect("parses");
    assert_eq!(got, vec!["include/a.h"]);
}

/// Order is first occurrence, not sorted and not last. The recorded set is
/// compared element-wise against the next run's, so a reordering reads as a
/// change to every unit in the project.
#[test]
fn prerequisites_keep_first_occurrence_order() {
    let got = parse_prerequisites("build/a.o: z.h a.h m.h a.h z.h\n", "").expect("parses");
    assert_eq!(got, vec!["z.h", "a.h", "m.h"]);
}

/// Without a `:` there is no target and no prerequisite list, so the text is
/// not a depfile at all. This is the only thing the grammar can call
/// malformed: everything else it can see is a token.
#[test]
fn text_with_no_colon_is_not_a_depfile() {
    let err = parse_prerequisites("no colon here at all\n", "src/a.c").expect_err("must not parse");
    assert_eq!(
        err,
        DepfileSyntax {
            byte_offset: 0,
            reason: "no ':' separating target from prerequisites".to_string()
        }
    );
}

/// A compiler that found no prerequisites emits a target and nothing else.
/// That is an empty list, not an error, and not a reason to rebuild.
#[test]
fn a_target_with_no_prerequisites_is_an_empty_list() {
    let got = parse_prerequisites("build/a.o:\n", "").expect("parses");
    assert!(got.is_empty(), "got {got:?}");
}

/// Only the FIRST colon is a target separator, so `-MP`'s phony stanzas come
/// back as tokens with their colon still attached. This pins the limitation
/// rather than endorsing it: cook has never emitted `-MP` from
/// `discovered_inputs`, and `cook_cache::parse_make_depfile` drops
/// `"include/a.h:"` because no file is named that. Written down because the
/// grammar's correctness here is currently supplied by a filter that is not
/// its business, and a reader who deletes that filter should find this first.
#[test]
fn a_phony_target_stanza_comes_back_with_its_colon_attached() {
    let got = parse_prerequisites("build/a.o: src/a.c include/a.h\ninclude/a.h:\n", "src/a.c")
        .expect("parses");
    assert_eq!(got, vec!["include/a.h", "include/a.h:"]);
}

/// The same limitation for a depfile carrying two rules: the second rule's
/// target is not recognised as a target, only as a token.
#[test]
fn a_second_rules_target_is_not_recognised_as_a_target() {
    let got = parse_prerequisites("build/a.o: src/a.c\nbuild/b.o: src/b.c\n", "").expect("parses");
    assert_eq!(got, vec!["src/a.c", "build/b.o:", "src/b.c"]);
}
