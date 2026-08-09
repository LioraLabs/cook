// COOK-425: what a depfile MEANS is `cook_contracts::depfile`, and its ten
// grammar cases are tested there over string literals with no filesystem. What
// is tested here is the part that needs one: reading the file, mapping the two
// IO failures, and dropping prerequisites that do not exist on disk.

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

/// The grammar's one syntax failure reaches callers as `Malformed` with its
/// offset and reason intact. `cook-graph` turns any error into an empty edge
/// set and the executor logs the text, so both ends depend on this mapping
/// surviving the read.
#[test]
fn a_syntax_failure_surfaces_as_malformed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    write_file(wd, ".cook/deps/a.d", "no colon here at all\n");

    let result = parse_make_depfile(&wd.join(".cook/deps/a.d"), "src/a.c", wd);

    match result {
        Err(DepfileError::Malformed { byte_offset, reason }) => {
            assert_eq!(byte_offset, 0);
            assert!(reason.contains("no ':'"), "got {reason:?}");
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

/// The reader delegates to the grammar rather than re-deciding anything: a
/// depfile on disk yields exactly what its text names, minus the source.
#[test]
fn reads_a_depfile_from_disk_and_returns_what_it_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    write_file(wd, "src/a.c", "// source\n");
    write_file(wd, "include/a.h", "#pragma once\n");
    write_file(wd, ".cook/deps/a.d", "build/a.o: src/a.c include/a.h\n");

    let paths = parse_make_depfile(&wd.join(".cook/deps/a.d"), "src/a.c", wd).expect("ok");

    assert_eq!(paths, vec!["include/a.h".to_string()]);
}

/// A prerequisite the compiler named but nothing produced is not an input. A
/// generated header deleted between runs would otherwise be recorded, missed on
/// the next check, and rebuild the unit forever.
#[test]
fn skips_nonexistent_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    write_file(wd, "src/a.c", "");
    write_file(wd, "include/exists.h", "");
    write_file(wd, ".cook/deps/a.d",
        "build/a.o: src/a.c include/exists.h include/missing.h\n");

    let paths = parse_make_depfile(&wd.join(".cook/deps/a.d"), "src/a.c", wd).expect("ok");

    assert_eq!(paths, vec!["include/exists.h".to_string()]);
}

/// COOK-425 moved the dedupe into the grammar, so it now runs BEFORE the
/// existence filter instead of after it. This pins that the observable list is
/// unchanged: a repeated missing path contributes nothing and does not displace
/// or reorder the existing ones around it. Recorded input sets are compared
/// element-wise, so a reordering here reads as an input change on every
/// compiled unit in a project.
#[test]
fn a_repeated_missing_path_neither_appears_nor_reorders_the_rest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    write_file(wd, "src/a.c", "");
    write_file(wd, "keep.h", "");
    write_file(wd, "other.h", "");
    write_file(wd, ".cook/deps/a.d",
        "build/a.o: src/a.c gone.h keep.h gone.h keep.h other.h\n");

    let paths = parse_make_depfile(&wd.join(".cook/deps/a.d"), "src/a.c", wd).expect("ok");

    assert_eq!(paths, vec!["keep.h".to_string(), "other.h".to_string()]);
}
