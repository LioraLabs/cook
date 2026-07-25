//! End-to-end tests for `cook cache dump` (CS-0166, COOK-313).
//!
//! The v7 recipe index is binary, so this command is the readable view that
//! replaces opening the file. These tests hold it to that job: after a build,
//! dumping the index must produce valid TOML naming the steps that ran, and
//! asking for an index that is not there must fail clearly rather than
//! printing an empty document.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

/// A one-recipe project whose build produces two units from two inputs.
fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.txt"), "hello\n").unwrap();
    fs::write(dir.path().join("src/b.txt"), "world\n").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        "recipe build\n    ingredients \"src/*.txt\"\n    \
         cook \"out/$<in.stem>.up\" { tr 'a-z' 'A-Z' < $<in> > $<out> }\n",
    )
    .unwrap();
    dir
}

fn run(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_bin())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run cook")
}

#[test]
fn dump_renders_the_index_as_parseable_toml() {
    let dir = project();
    let build = run(dir.path(), &["build"]);
    assert!(build.status.success(), "build failed: {build:?}");

    let out = run(dir.path(), &["cache", "dump", "build"]);
    assert!(out.status.success(), "dump failed: {out:?}");
    let text = String::from_utf8(out.stdout).expect("utf8");

    // It must actually parse — the whole point is a readable view a person
    // (or a one-off script) can trust.
    let parsed: toml::Value = text.parse().unwrap_or_else(|e| {
        panic!("cache dump did not emit valid TOML: {e}\n---\n{text}");
    });

    assert_eq!(
        parsed.get("schema_version").and_then(|v| v.as_integer()),
        Some(cook_engine::cook_cache::CACHE_VERSION as i64),
        "dump states the schema version it rendered"
    );

    let steps = parsed
        .get("steps")
        .and_then(|v| v.as_table())
        .expect("a steps table");
    assert_eq!(steps.len(), 2, "both units are present; got: {text}");

    // Every step carries its attributed determinants and its records.
    for (key, step) in steps {
        assert!(
            step.get("command_hash").and_then(|v| v.as_str()).is_some(),
            "step {key} is missing command_hash"
        );
        let inputs = step.get("inputs").and_then(|v| v.as_array()).expect("inputs");
        let rec = inputs.first().expect("at least one input record");
        assert!(rec.get("path").and_then(|v| v.as_str()).is_some());
        assert!(rec.get("mtime").and_then(|v| v.as_integer()).is_some());
        assert!(rec.get("hash").and_then(|v| v.as_str()).is_some());
    }

    assert!(
        text.contains("src/a.txt") && text.contains("src/b.txt"),
        "both source paths should appear; got: {text}"
    );
}

#[test]
fn dump_inlines_records_rather_than_restating_the_step_key() {
    // The v4..v6 TOML index spent 48.8% of its bytes on
    // `[[steps."<key>".inputs]]` headers, one per record. That shape is the
    // reason the format changed; the readable view must not reintroduce it.
    let dir = project();
    assert!(run(dir.path(), &["build"]).status.success());

    let out = run(dir.path(), &["cache", "dump", "build"]);
    let text = String::from_utf8(out.stdout).expect("utf8");

    assert!(
        !text.contains("[[steps."),
        "records must be inlined, not emitted as an array-of-tables; got: {text}"
    );
    assert!(
        text.contains("{ path = "),
        "each record should be one inline table; got: {text}"
    );
}

#[test]
fn dump_of_an_unbuilt_recipe_fails_clearly() {
    let dir = project();
    // No build has run, so there is no index at all.
    let out = run(dir.path(), &["cache", "dump", "build"]);

    assert!(!out.status.success(), "must not exit 0 with nothing to show");
    assert!(
        out.stdout.is_empty(),
        "must not print a misleading empty document"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("build"),
        "the error should name the recipe asked for; got: {err}"
    );
}

#[test]
fn dump_names_an_unknown_recipe_in_its_error() {
    let dir = project();
    assert!(run(dir.path(), &["build"]).status.success());

    let out = run(dir.path(), &["cache", "dump", "nosuchrecipe"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("nosuchrecipe"),
        "the error should name the recipe asked for; got: {err}"
    );
}
