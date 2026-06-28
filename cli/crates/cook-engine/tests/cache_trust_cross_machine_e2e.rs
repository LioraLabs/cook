//! COOK-170 cross-"machine" E2E smoke: the Cache-trust v3 single-key model end
//! to end against the real `cook` binary and a shared store in tmpdirs.
//!
//! One narrative ties the dispositions together (per-feature bars live in
//! seal_host_key_e2e / sharing_disposition_e2e / record_disposition_e2e):
//!
//!   * `portable` (unannotated)  — machine-independent key => HITS across a host
//!     change AND publishes fleet-wide.
//!   * `hostdep` (`seal host`)   — the host probe value folds into the key =>
//!     MISSES + rebuilds on a host change.
//!   * `scratch` (`local`)       — never published to the shared store.
//!   * `generate` (`record`)     — warm hit reuses the recording, no re-generate.
//!   * `pin` (`pinned`)          — cold miss in both stores is a HARD ERROR.
//!
//! The host signal is an `envs { SIMHOST }` probe; flipping SIMHOST
//! simulates moving to a different machine. Per-unit runlogs make a re-run
//! observable; `record` counts via a byte-appended side file. The shared store
//! is a LocalBackend rooted at `.cook/cloud.toml`'s cache_dir, in a SEPARATE
//! tempdir from the local `.cook` index.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("cook");
    assert!(
        path.exists(),
        "cook binary not found at {} — run `cargo build --bin cook` first",
        path.display()
    );
    path
}

const COOKFILE: &str = r#"probe host
    envs { SIMHOST }

recipe portable
    ingredients "src/in.txt"
    cook "out/portable.txt" {
        cp src/in.txt out/portable.txt
        echo ran >> out/portable.runlog
    }

recipe hostdep
    ingredients "src/in.txt"
    seal host
    cook "out/host.txt" {
        printf 'built\n' > out/host.txt
        echo ran >> out/host.runlog
    }

recipe scratch
    ingredients "src/in.txt"
    cook "out/scratch.txt" {
        printf 'scratch-only-marker\n' > out/scratch.txt
        echo ran >> out/scratch.runlog
    } local

recipe generate
    cook "out/gen.txt" {
        printf x >> out/gen.side
        date +%N > out/gen.txt
    } nondet

recipe pin
    ingredients "src/in.txt"
    cook "out/pin.txt" {
        cp src/in.txt out/pin.txt
        echo ran >> out/pin.runlog
    } pinned
"#;

fn write_fixture(wd: &Path, cache_dir: &Path) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
    fs::create_dir_all(wd.join("src")).unwrap();
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(wd.join("src/in.txt"), "src-content\n").unwrap();
    fs::write(wd.join("Cookfile"), COOKFILE).unwrap();
}

fn run(wd: &Path, recipe: &str, simhost: &str) -> Output {
    Command::new(cook_binary())
        .arg(recipe)
        .env("SIMHOST", simhost)
        .current_dir(wd)
        .output()
        .expect("cook invocation")
}

fn build(wd: &Path, recipe: &str, simhost: &str) {
    let out = run(wd, recipe, simhost);
    assert!(
        out.status.success(),
        "cook {recipe} (SIMHOST={simhost}) failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

fn runs(wd: &Path, runlog: &str) -> usize {
    fs::read_to_string(wd.join("out").join(runlog))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn gen_runs(wd: &Path) -> u64 {
    fs::metadata(wd.join("out/gen.side")).map(|m| m.len()).unwrap_or(0)
}

fn artifact_file_count(dir: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                count += 1;
            }
        }
    }
    count
}

/// True if `needle` appears in any regular file under `dir` (recursively).
fn store_contains(dir: &Path, needle: &str) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(body) = fs::read_to_string(&path) {
                if body.contains(needle) {
                    return true;
                }
            }
        }
    }
    false
}

#[test]
fn cache_trust_v3_cross_machine_narrative() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_fixture(wd, cache.path());

    // 1. Cold build under SIMHOST=alpha: every reproducible unit runs once.
    for r in ["portable", "hostdep", "scratch", "generate"] {
        build(wd, r, "alpha");
    }
    assert_eq!(runs(wd, "portable.runlog"), 1, "portable builds cold");
    assert_eq!(runs(wd, "host.runlog"), 1, "hostdep builds cold");
    assert_eq!(runs(wd, "scratch.runlog"), 1, "scratch builds cold");
    assert_eq!(gen_runs(wd), 1, "record generate runs cold");

    // 2. The shared store holds published artifacts, but NONE from `scratch`.
    assert!(
        artifact_file_count(cache.path()) >= 1,
        "non-local units MUST publish to the shared store"
    );
    let scratch_content = fs::read_to_string(wd.join("out/scratch.txt")).unwrap();
    assert!(
        !store_contains(cache.path(), scratch_content.trim()),
        "a `local` unit MUST NOT publish to the shared store"
    );

    // 3. Warm rerun under the SAME host: every unit hits (record included).
    for r in ["portable", "hostdep", "scratch", "generate"] {
        build(wd, r, "alpha");
    }
    assert_eq!(runs(wd, "portable.runlog"), 1, "portable warm hit");
    assert_eq!(runs(wd, "host.runlog"), 1, "hostdep warm hit (stable host value)");
    assert_eq!(gen_runs(wd), 1, "record warm hit reuses recording, no re-generate");

    // 4. Host change to SIMHOST=beta: portable HITS, hostdep MISSES + rebuilds.
    for r in ["portable", "hostdep"] {
        build(wd, r, "beta");
    }
    assert_eq!(
        runs(wd, "portable.runlog"),
        1,
        "portable MUST hit across a host change — its key is machine-independent"
    );
    assert_eq!(
        runs(wd, "host.runlog"),
        2,
        "`seal host` hostdep MUST miss + rebuild — the changed host value folds \
         into its single cache key"
    );

    // 5. A `pinned` unit with no artifact anywhere is a HARD ERROR: non-zero
    //    exit and the unit is NOT executed.
    let out = run(wd, "pin", "alpha");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "pinned cold-miss MUST fail the run; got success.\n{combined}"
    );
    assert!(
        !wd.join("out/pin.txt").exists(),
        "pinned cold-miss MUST NOT execute the unit"
    );
    assert_eq!(runs(wd, "pin.runlog"), 0, "pinned cold-miss MUST NOT execute");
    assert!(
        combined.contains("pinned") || combined.contains("MUST NOT be rebuilt"),
        "pinned cold-miss error should name the pinned / fetch-only rule.\n{combined}"
    );
}
