//! `for_each` data-member fan-out integration tests (COOK-64, §22.5.9).
//!
//! End-to-end at the binary level: write a Cookfile + data file in a tempdir,
//! invoke `cook <recipe>`, and inspect filesystem outputs and the per-member
//! cache behaviour.
//!
//! The headline test (`for_each_per_member_invalidation`) proves the two-layer
//! cache of §22.5.9 / §17.1 observable #5: editing ONE data member re-runs
//! only that member's unit, while the others stay cache hits — even though the
//! feeding probe re-evaluates the whole set.

use std::fs;
use std::path::Path;
use std::process::Command;
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

fn run_cook(dir: &Path, args: &[&str]) -> std::process::Output {
    let out = Command::new(cook_binary())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn cook");
    assert!(
        out.status.success(),
        "cook {args:?} failed (exit={:?}):\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

/// Count the lines a recipe's per-member units appended to `ran.log`. Each
/// `cook` unit appends its member id on a real execution; a cache hit restores
/// the declared output without running the command, so no append occurs.
fn ran_lines(dir: &Path) -> usize {
    fs::read_to_string(dir.join("ran.log"))
        .map(|s| s.lines().filter(|l| !l.is_empty()).count())
        .unwrap_or(0)
}

/// §22.5.9 per-member invalidation. A probe feeds a `for_each` over three
/// records; editing ONE record's field re-runs only that member's unit.
#[test]
fn for_each_per_member_invalidation() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();

    // Uniquify the data per test invocation so the host-wide cache
    // (~/.cache/cook/cloud) cannot serve a stale hit from a prior run — the
    // per-member command_hash folds in the member content (§17.1).
    let uniq = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let data = format!(
        r#"[
  {{"id": "alpha", "tag": "A-{uniq}"}},
  {{"id": "bravo", "tag": "B-{uniq}"}},
  {{"id": "carol", "tag": "C-{uniq}"}}
]
"#
    );
    fs::write(dir.join("data.json"), &data).unwrap();

    // The probe resolves the array; the `for_each` recipe fans out one cook
    // unit per record. Each unit writes its tag to a declared output AND
    // appends its id to `ran.log` (a side effect, not a declared output, so it
    // is NOT restored on a cache hit — exactly what makes it a run-counter).
    let cookfile = r#"
register
    cook.probe("recs", {
        inputs  = { files = {"data.json"} },
        produce = [[ return cook.json_decode(cook.sh("cat data.json")) ]],
    })

recipe gen
    ingredients recs
    cook "out/$<in.id>.txt" using {
        mkdir -p out
        printf '%s' "$<in.tag>" > $<out>
        printf '%s\n' "$<in.id>" >> ran.log
    }
"#;
    fs::write(dir.join("Cookfile"), cookfile).unwrap();

    // ── First run: all three members execute. ──────────────────────────
    run_cook(dir, &["gen"]);
    assert_eq!(ran_lines(dir), 3, "first run executes all three members");
    for id in ["alpha", "bravo", "carol"] {
        assert!(
            dir.join(format!("out/{id}.txt")).exists(),
            "output for {id} should exist after first run"
        );
    }
    let alpha_first = fs::read_to_string(dir.join("out/alpha.txt")).unwrap();

    // ── Re-run with no change: every member is a cache hit. ─────────────
    run_cook(dir, &["gen"]);
    assert_eq!(
        ran_lines(dir),
        3,
        "no-change re-run must not re-execute any member"
    );

    // ── Edit ONLY bravo's tag, re-run. ──────────────────────────────────
    let edited = data.replace(&format!("B-{uniq}"), &format!("B-{uniq}-EDITED"));
    assert_ne!(edited, data, "the edit must actually change the data file");
    fs::write(dir.join("data.json"), &edited).unwrap();

    run_cook(dir, &["gen"]);
    assert_eq!(
        ran_lines(dir),
        4,
        "editing one member re-runs exactly one unit (alpha + carol stay cached)"
    );

    // bravo's output reflects the edit; alpha's is byte-identical (cache hit).
    assert_eq!(
        fs::read_to_string(dir.join("out/bravo.txt")).unwrap(),
        format!("B-{uniq}-EDITED"),
        "bravo's output reflects the edited tag"
    );
    assert_eq!(
        fs::read_to_string(dir.join("out/alpha.txt")).unwrap(),
        alpha_first,
        "alpha's output is unchanged (it was a cache hit)"
    );

    // The fourth append must be bravo (the only member that re-ran).
    let log = fs::read_to_string(dir.join("ran.log")).unwrap();
    assert_eq!(
        log.lines().filter(|l| !l.is_empty()).last(),
        Some("bravo"),
        "the only re-executed member is bravo; got log:\n{log}"
    );
}
