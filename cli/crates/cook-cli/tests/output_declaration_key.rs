//! The output declaration is a determinant of a unit's cache key
//! (§{exec.cache.single-key}, "declared output paths"; CS-0276).
//!
//! Each case changes ONLY the output declaration of one unit whose command
//! writes every file any of the declarations names, so the command text, the
//! inputs and the env are identical across the two runs. Before CS-0276 both
//! tiers served the first declaration's record to the second:
//!
//! * the shared key folded no output path, and the manifest guard only refused
//!   a producer naming a path the new declaration does not cover — so a
//!   declaration that GREW (`a.txt` → `a.txt b.txt`, `out/x` → `out/`) restored
//!   the old producer's outputs and recorded them as the new unit's;
//! * the local identity was the first declared output alone, and a record for
//!   a directory or glob declaration is judged by its recorded concrete paths —
//!   so `a.txt out/` → `a.txt dist/` replayed with `dist/` never produced.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// A unit sealed on `src.txt` whose command writes every path any case
/// declares; only `outputs` varies.
fn cookfile(outputs: &str) -> String {
    format!(
        "recipe gen\n    seal \"src.txt\"\n    cook {outputs} {{\n        mkdir -p out dist\n        echo A > a.txt; echo B > b.txt; echo OX > out/x; echo OY > out/y; echo DJ > dist/x.js; echo DC > dist/x.css\n    }}\n"
    )
}

/// A workspace whose shared store is private to it, and whose XDG cache is
/// too, so no other run's artifacts can answer for this one.
fn workspace(outputs: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join(".cook")).unwrap();
    let shared = dir.path().join(".cook/shared-cache");
    fs::write(
        dir.path().join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", shared.to_string_lossy()),
    )
    .unwrap();
    fs::write(dir.path().join("src.txt"), "hi\n").unwrap();
    fs::write(dir.path().join("Cookfile"), cookfile(outputs)).unwrap();
    dir
}

fn cook(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_cook"))
        .current_dir(dir)
        .env("XDG_CACHE_HOME", dir.join(".xdg"))
        .args(args)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "cook {args:?} failed:\n{text}");
    text
}

/// Every built file, so a run is judged by what it left in the tree.
fn remove_outputs(dir: &Path) {
    for f in ["a.txt", "b.txt"] {
        let _ = fs::remove_file(dir.join(f));
    }
    for d in ["out", "dist"] {
        let _ = fs::remove_dir_all(dir.join(d));
    }
}

/// The shared key `cook why --level unit` reports for `gen`.
fn why_key(dir: &Path) -> String {
    let text = cook(dir, &["why", "gen", "--level", "unit"]);
    let line = text
        .lines()
        .find(|l| l.contains("  key "))
        .unwrap_or_else(|| panic!("no key line in:\n{text}"));
    line.rsplit("key ").next().unwrap().trim().to_string()
}

/// A declaration that covers the old producer's outputs is still a different
/// declaration: a machine with no local record must not be served them.
#[test]
fn a_grown_declaration_is_not_served_the_old_producers_outputs() {
    for (before, after, must_exist) in [
        ("\"a.txt\"", "\"a.txt\" \"b.txt\"", "b.txt"),
        ("\"out/x\"", "\"out/\"", "out/y"),
    ] {
        let ws = workspace(before);
        let dir = ws.path();
        cook(dir, &["gen"]);
        // A second machine: the shared store, and nothing else.
        remove_outputs(dir);
        fs::remove_dir_all(dir.join(".cook/cache")).unwrap();
        fs::write(dir.join("Cookfile"), cookfile(after)).unwrap();
        cook(dir, &["gen"]);
        assert!(
            dir.join(must_exist).exists(),
            "{before} -> {after}: {must_exist} was never produced"
        );
    }
}

/// Same first output, so the same local record slot before CS-0276: the rest
/// of the declaration must still separate the two.
#[test]
fn a_changed_declaration_beside_the_same_first_output_rebuilds() {
    for (before, after, must_exist) in [
        ("\"a.txt\" \"out/\"", "\"a.txt\" \"dist/\"", "dist/x.js"),
        ("\"a.txt\" \"out/x\"", "\"a.txt\" \"out/\"", "out/y"),
    ] {
        let ws = workspace(before);
        let dir = ws.path();
        cook(dir, &["gen"]);
        remove_outputs(dir);
        // Restore the first declaration's outputs from its own record, so the
        // local record is warm and every file it lists is present.
        cook(dir, &["gen"]);
        fs::write(dir.join("Cookfile"), cookfile(after)).unwrap();
        cook(dir, &["gen"]);
        assert!(
            dir.join(must_exist).exists(),
            "{before} -> {after}: {must_exist} was never produced"
        );
    }
}

/// Two Cookfiles differing only in the output declaration report two keys,
/// and each names its own declaration; the same declaration reports one.
#[test]
fn cook_why_reports_one_key_per_output_declaration() {
    let pairs = [
        ("\"a.txt\"", "\"b.txt\""),
        ("\"dist/\"", "\"out/\""),
        ("\"dist/*.js\"", "\"dist/*.css\""),
        ("\"out/x\"", "\"out/\""),
        ("\"a.txt\" \"out/\"", "\"a.txt\" \"dist/\""),
    ];
    for (a, b) in pairs {
        let ws = workspace(a);
        let dir = ws.path();
        let key_a = why_key(dir);
        assert_eq!(key_a, why_key(dir), "{a}: one declaration, one key");
        fs::write(dir.join("Cookfile"), cookfile(b)).unwrap();
        let key_b = why_key(dir);
        assert_ne!(key_a, key_b, "{a} and {b} must not share a key");
        let shown = cook(dir, &["why", "gen", "--level", "unit"]);
        let last = b.rsplit('"').nth(1).unwrap();
        assert!(shown.contains(last), "{b}: why does not name the declaration:\n{shown}");
    }
}
