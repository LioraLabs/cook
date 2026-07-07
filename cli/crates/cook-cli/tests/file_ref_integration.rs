//! `$<file:PATH>` file-reference integration tests (CS-0101, COOK-86).
//!
//! End-to-end tests at the binary level — write a Cookfile in a tempdir,
//! invoke `cook <recipe>`, and inspect outputs, mtimes, and diagnostics.
//!
//! Coverage:
//!   * `editing_referenced_file_invalidates_only_referencing_unit` —
//!     targeted invalidation: an unrelated-file edit is a cache hit; a
//!     referenced-file edit re-executes the referencing unit.
//!   * `glob_file_ref_new_match_invalidates` — a glob file reference
//!     gaining a match invalidates via the resolved-path-list change.
//!   * `missing_file_ref_fails_at_registration` — a literal `file:` path
//!     naming a missing file fails before any unit executes, with the
//!     CS-0101 diagnostic naming the path.
//!   * `fan_out_members_all_track_shared_file` — every member of a probe
//!     fan-out tracks a shared `$<file:...>` reference: a shared-file edit
//!     re-runs ALL members; a no-edit rerun is a full cache hit.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // /target/debug/deps  →  /target/debug
    path.pop(); // /target/debug       →  /target
    path.push("cook");
    if !path.exists() {
        panic!(
            "cook binary not found at {} — run `cargo build --bin cook` first",
            path.display()
        );
    }
    path
}

fn run_cook(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    // Isolate the shared cache backend per test dir. Without this the build
    // uses the global `~/.cache/cook/cloud` store, and COOK-162 cold
    // fetch-by-key would serve an output published by a previous (cross-test)
    // run as a spurious first-run cache hit, perturbing the mtimes these tests
    // assert on. Pointing `cache_dir` inside the tempdir keeps each test's
    // shared store private and empty until the test itself populates it.
    let cloud_toml = dir.join(".cook/cloud.toml");
    if !cloud_toml.exists() {
        fs::create_dir_all(dir.join(".cook")).map_err(|e| e.to_string())?;
        let shared = dir.join(".cook/shared-cache");
        fs::write(
            &cloud_toml,
            format!("[cache]\ncache_dir = {:?}\n", shared.to_string_lossy()),
        )
        .map_err(|e| e.to_string())?;
    }
    let out = Command::new(cook_binary())
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "cook failed (exit={:?}): stdout={}, stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(out)
}

fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {} failed: {e}", path.display()))
        .modified()
        .unwrap()
}

/// Filesystem mtime resolution can be coarse; keep edits and reruns from
/// landing in the same timestamp tick.
fn settle() {
    std::thread::sleep(Duration::from_millis(50));
}

/// CS-0101 observable 6 (§17.1): editing a `$<file:...>`-referenced file
/// MUST invalidate exactly the units that reference it. An unrelated-file
/// edit MUST NOT invalidate (cache hit, output mtime unchanged); a
/// referenced-file edit MUST re-execute (output reflects the new content).
#[test]
fn editing_referenced_file_invalidates_only_referencing_unit() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/page.md"), "# page body\n").unwrap();
    fs::write(tmp.path().join("style.css"), "css-v1\n").unwrap();
    fs::write(tmp.path().join("unrelated.txt"), "noise-v1\n").unwrap();
    let cookfile = r#"
recipe "html"
    ingredients "src/page.md"
    cook "build/$<in.stem>.html" {
        cat $<in> $<file:style.css> > $<out>
    }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    // First run: output contains the v1 css content.
    run_cook(tmp.path(), &["html"]).expect("first run should succeed");
    let out_path = tmp.path().join("build/page.html");
    let out1 = fs::read_to_string(&out_path).expect("build/page.html should exist");
    assert!(out1.contains("css-v1"), "output should contain css-v1; got: {out1:?}");
    let mtime1 = mtime(&out_path);

    // Unrelated-file edit → rerun → cache hit (mtime unchanged).
    settle();
    fs::write(tmp.path().join("unrelated.txt"), "noise-v2\n").unwrap();
    settle();
    run_cook(tmp.path(), &["html"]).expect("rerun after unrelated edit should succeed");
    let mtime2 = mtime(&out_path);
    assert_eq!(
        mtime1, mtime2,
        "unrelated-file edit must NOT invalidate the unit (expected cache hit)"
    );

    // Referenced-file edit → rerun → unit re-executes with new content.
    settle();
    fs::write(tmp.path().join("style.css"), "css-v2\n").unwrap();
    settle();
    run_cook(tmp.path(), &["html"]).expect("rerun after style.css edit should succeed");
    let out2 = fs::read_to_string(&out_path).unwrap();
    assert!(
        out2.contains("css-v2"),
        "referenced-file edit must re-execute the unit; output still: {out2:?}"
    );
}

/// CS-0101 observable 6 (§17.1): a glob file reference gaining a match MUST
/// invalidate via the resolved-path-list change — the new match's content
/// shows up in the rebuilt output.
#[test]
fn glob_file_ref_new_match_invalidates() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::create_dir_all(tmp.path().join("templates")).unwrap();
    fs::write(tmp.path().join("src/page.md"), "# page body\n").unwrap();
    fs::write(tmp.path().join("templates/a.html"), "template-a\n").unwrap();
    let cookfile = r#"
recipe "html"
    ingredients "src/page.md"
    cook "build/$<in.stem>.html" {
        cat $<in> $<file:templates/*.html> > $<out>
    }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    run_cook(tmp.path(), &["html"]).expect("first run should succeed");
    let out_path = tmp.path().join("build/page.html");
    let out1 = fs::read_to_string(&out_path).expect("build/page.html should exist");
    assert!(out1.contains("template-a"), "output should contain template-a; got: {out1:?}");
    assert!(!out1.contains("template-b"), "template-b must not exist yet; got: {out1:?}");

    // A new glob match appears → the resolved path list changes → the unit
    // MUST re-execute and fold in the new file.
    settle();
    fs::write(tmp.path().join("templates/b.html"), "template-b\n").unwrap();
    settle();
    run_cook(tmp.path(), &["html"]).expect("rerun after new glob match should succeed");
    let out2 = fs::read_to_string(&out_path).unwrap();
    assert!(
        out2.contains("template-b"),
        "new glob match must invalidate and rebuild; output still: {out2:?}"
    );
}

/// CS-0101: a literal `$<file:PATH>` naming a missing file is a
/// register-phase diagnostic — `cook` fails before executing anything,
/// names the missing path, cites CS-0101, and produces no outputs.
#[test]
fn missing_file_ref_fails_at_registration() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/page.md"), "# page body\n").unwrap();
    let cookfile = r#"
recipe "html"
    ingredients "src/page.md"
    cook "build/$<in.stem>.html" {
        cat $<in> $<file:missing.css> > $<out>
    }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    let err = run_cook(tmp.path(), &["html"])
        .expect_err("missing file reference must fail the build");
    assert!(
        err.contains("missing.css"),
        "diagnostic must name the missing file; got: {err}"
    );
    assert!(err.contains("file not found"), "diagnostic must name the file-ref failure; got: {err}");
    assert!(
        !tmp.path().join("build").exists(),
        "registration failure must not produce build/ outputs"
    );
}

/// CS-0101 + CS-0092 fan-out: every member unit of a probe fan-out tracks a
/// shared `$<file:...>` reference. Editing the shared file re-runs ALL
/// members; a no-edit rerun is a full cache hit (both output mtimes
/// unchanged).
#[test]
fn fan_out_members_all_track_shared_file() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("style.css"), "css-v1\n").unwrap();
    // Uniquify the probe key per invocation so the host-wide probe-value
    // cache (~/.cache/cook/cloud) can never serve a stale hit from a prior
    // `cargo test` run (the probe fingerprint folds in the key).
    let uniq = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let probe_key = format!("scenes_{uniq}");
    let cookfile = format!(
        r#"
probe {probe_key}
    json {{ echo '[{{"id":"intro"}},{{"id":"outro"}}]' }}

recipe html
    ingredients {probe_key}
    cook "build/$<in.id>.html" {{ echo $<in.id> > $<out> && cat $<file:style.css> >> $<out> }}
"#
    );
    fs::write(tmp.path().join("Cookfile"), &cookfile).unwrap();

    // First run: one output per member, each carrying the shared css.
    run_cook(tmp.path(), &["html"]).expect("first run should succeed");
    let intro_path = tmp.path().join("build/intro.html");
    let outro_path = tmp.path().join("build/outro.html");
    let intro1 = fs::read_to_string(&intro_path).expect("build/intro.html should exist");
    let outro1 = fs::read_to_string(&outro_path).expect("build/outro.html should exist");
    assert!(intro1.contains("intro") && intro1.contains("css-v1"), "intro output: {intro1:?}");
    assert!(outro1.contains("outro") && outro1.contains("css-v1"), "outro output: {outro1:?}");

    // Shared-file edit → BOTH members re-run.
    settle();
    fs::write(tmp.path().join("style.css"), "css-v2\n").unwrap();
    settle();
    run_cook(tmp.path(), &["html"]).expect("rerun after style.css edit should succeed");
    let intro2 = fs::read_to_string(&intro_path).unwrap();
    let outro2 = fs::read_to_string(&outro_path).unwrap();
    assert!(
        intro2.contains("css-v2"),
        "shared-file edit must re-run the intro member; output still: {intro2:?}"
    );
    assert!(
        outro2.contains("css-v2"),
        "shared-file edit must re-run the outro member; output still: {outro2:?}"
    );

    // No-edit rerun → full cache hit (both mtimes unchanged).
    let intro_mtime = mtime(&intro_path);
    let outro_mtime = mtime(&outro_path);
    settle();
    run_cook(tmp.path(), &["html"]).expect("no-edit rerun should succeed");
    assert_eq!(
        intro_mtime,
        mtime(&intro_path),
        "no-edit rerun must be a cache hit for the intro member"
    );
    assert_eq!(
        outro_mtime,
        mtime(&outro_path),
        "no-edit rerun must be a cache hit for the outro member"
    );
}
