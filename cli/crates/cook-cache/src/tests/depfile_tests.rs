use super::*;
use std::fs;

fn write_file(dir: &Path, rel: &str, content: &str) {
    let abs = dir.join(rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(&abs, content).expect("write");
}

#[test]
fn returns_not_found_for_missing_file() {
    let dir = tempfile::tempdir().expect("tempdir");
        let result = parse_make_depfile(
            &dir.path().join("nonexistent.d"),
        "src/a.c",
        dir.path(),
    );
    assert!(matches!(result, Err(DepfileError::NotFound)));
}

#[test]
fn parses_single_line_depfile() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "// source\n");
    write_file(wd, "include/a.h", "#pragma once\n");
    write_file(wd, ".cook/deps/a.d", "build/a.o: src/a.c include/a.h\n");

    let paths = parse_make_depfile(
        &wd.join(".cook/deps/a.d"),
        "src/a.c",
        wd,
    )
    .expect("ok");

    assert_eq!(paths, vec!["include/a.h".to_string()]);
}

#[test]
fn joins_continuation_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
    write_file(wd, "include/a.h", "");
    write_file(wd, "include/b.h", "");
    write_file(wd, ".cook/deps/a.d",
        "build/a.o: src/a.c \\\n  include/a.h \\\n  include/b.h\n");

    let paths = parse_make_depfile(
        &wd.join(".cook/deps/a.d"),
        "src/a.c",
        wd,
    )
    .expect("ok");

    assert_eq!(paths, vec!["include/a.h".to_string(), "include/b.h".to_string()]);
}

#[test]
fn joins_crlf_continuation_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
    write_file(wd, "include/a.h", "");
    write_file(wd, "include/b.h", "");
    write_file(wd, ".cook/deps/a.d",
        "build/a.o: src/a.c \\\r\n  include/a.h \\\r\n  include/b.h\r\n");

    let paths = parse_make_depfile(
        &wd.join(".cook/deps/a.d"),
        "src/a.c",
        wd,
    )
    .expect("ok");

    assert_eq!(paths, vec!["include/a.h".to_string(), "include/b.h".to_string()]);
}

#[test]
fn skips_absolute_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
    write_file(wd, "include/a.h", "");
    write_file(wd, ".cook/deps/a.d",
        "build/a.o: src/a.c /usr/include/stdio.h include/a.h\n");

    let paths = parse_make_depfile(
        &wd.join(".cook/deps/a.d"),
        "src/a.c",
        wd,
    )
    .expect("ok");

    assert_eq!(paths, vec!["include/a.h".to_string()]);
}

#[test]
fn skips_source_self_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
    write_file(wd, "include/a.h", "");
    write_file(wd, ".cook/deps/a.d",
        "build/a.o: src/a.c include/a.h src/a.c\n");

    let paths = parse_make_depfile(
        &wd.join(".cook/deps/a.d"),
        "src/a.c",
        wd,
    )
    .expect("ok");

    assert_eq!(paths, vec!["include/a.h".to_string()]);
}

#[test]
fn skips_nonexistent_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
    write_file(wd, "include/exists.h", "");
    write_file(wd, ".cook/deps/a.d",
        "build/a.o: src/a.c include/exists.h include/missing.h\n");

    let paths = parse_make_depfile(
        &wd.join(".cook/deps/a.d"),
        "src/a.c",
        wd,
    )
    .expect("ok");

    assert_eq!(paths, vec!["include/exists.h".to_string()]);
}

#[test]
fn empty_source_path_disables_self_skip() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
    write_file(wd, ".cook/deps/a.d",
        "build/a.o: src/a.c\n");

    let paths = parse_make_depfile(
        &wd.join(".cook/deps/a.d"),
        "",
        wd,
    )
    .expect("ok");

    assert_eq!(paths, vec!["src/a.c".to_string()]);
}

#[test]
fn malformed_no_colon_returns_error() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, ".cook/deps/a.d", "no colon here at all\n");

    let result = parse_make_depfile(
        &wd.join(".cook/deps/a.d"),
        "src/a.c",
        wd,
    );

    assert!(matches!(result, Err(DepfileError::Malformed { .. })));
}

#[test]
fn deduplicates_repeated_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
    write_file(wd, "include/a.h", "");
    write_file(wd, ".cook/deps/a.d",
        "build/a.o: src/a.c include/a.h include/a.h\n");

    let paths = parse_make_depfile(
        &wd.join(".cook/deps/a.d"),
        "src/a.c",
        wd,
    )
    .expect("ok");

    assert_eq!(paths, vec!["include/a.h".to_string()]);
}
