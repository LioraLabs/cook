//! COOK-233 E2E: last-access touch-on-read, proven against the real `cook`
//! binary rather than a unit harness.
//!
//! `LocalBackend::get_with_meta` restamps a CAS blob's mtime on every genuine
//! hit so `cook cache gc` can evict in true-LRU order. Unit tests pin the
//! function; this bar pins the *integration*: that the touch fires on the code
//! path a real build takes, and that it is inert for build correctness.
//!
//! The narrative:
//!
//!   1. Cold `cook build` publishes one artifact into an ISOLATED cache_dir
//!      (its own tempdir, never `~/.cache/cook/cloud`).
//!   2. Every CAS blob is backdated to a fixed old stamp; every sidecar's
//!      bytes and mtime are snapshotted.
//!   3. The built output file is deleted — `.cook/` stays intact. That is the
//!      smallest lever that forces a genuine backend read: `needs_rebuild_cook`
//!      sees a missing output, `needs_restore` is non-empty, and `try_restore`
//!      -> `restore_one` -> `get_with_meta` runs. A fully settled run consults
//!      no backend at all, so some lever is required.
//!   4. Warm `cook build` restores the bytes; a blob mtime has advanced past
//!      the backdated stamp, and no sidecar was rewritten.
//!
//! Two properties make this non-vacuous:
//!
//!   * The recipe body is DETERMINISTIC in its declared output. `LocalBackend::put`
//!     short-circuits an idempotent re-put (same `content_hash` => discard tmp,
//!     return early, rewriting neither blob nor sidecar), so a rebuild would
//!     leave the blob mtime backdated and the assertion would fail as it should.
//!     A nondeterministic body would publish a NEW blob at a NEW key with a
//!     fresh mtime, and the mtime assertion would pass even with the touch
//!     removed.
//!   * The runlog counter pins "restored, not rebuilt" permanently. The runlog
//!     is deliberately UNDECLARED, so its growth feeds neither `content_hash`
//!     nor `cache verify`'s byte comparison; and `reconcile_dir_output` is gated
//!     on a trailing `/`, so a plain-file output triggers no sweep of it either.
//!     Determinism is scoped to the declared output; the two are not in conflict.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use filetime::FileTime;

fn cook_binary() -> PathBuf {
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

/// Deterministic declared output; undeclared runlog for the rebuild counter.
const COOKFILE: &str = r#"recipe build
    ingredients "src/in.txt"
    cook "out/hello.txt" {
        printf 'hello-deterministic\n' > out/hello.txt
        echo ran >> out/build.runlog
    }
"#;

const EXPECTED_OUTPUT: &str = "hello-deterministic\n";

/// A stamp far enough in the past that no clock skew can confuse it with "now":
/// 2001-09-09T01:46:40Z.
const BACKDATED: FileTime = FileTime::from_unix_time(1_000_000_000, 0);

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

fn run(wd: &Path, args: &[&str]) -> Output {
    Command::new(cook_binary())
        .args(args)
        .current_dir(wd)
        .output()
        .expect("cook invocation")
}

fn run_ok(wd: &Path, args: &[&str]) -> Output {
    let out = run(wd, args);
    assert!(
        out.status.success(),
        "cook {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

/// Lines appended to the undeclared runlog — one per genuine body execution.
fn runs(wd: &Path) -> usize {
    fs::read_to_string(wd.join("out/build.runlog"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(path),
                Ok(ft) if ft.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    out.sort();
    out
}

/// `path_for` yields `{root}/{2hex}/{62hex}`; sidecars are the same 62-hex stem
/// with `.meta.json` / `.provenance.json` bolted on via `with_extension`. So a
/// CAS blob is exactly the extension-less 62-char file name.
fn is_blob(p: &Path) -> bool {
    p.extension().is_none()
        && p.file_name().map(|n| n.len() == 62).unwrap_or(false)
}

fn mtime(p: &Path) -> FileTime {
    FileTime::from_last_modification_time(&fs::metadata(p).unwrap())
}

#[test]
fn touch_on_read_fires_e2e_and_is_inert() {
    let tmp = tempfile::tempdir().expect("workspace tempdir");
    // ISOLATION: the CAS lives in its own tempdir, never the developer's
    // real store at ~/.cache/cook/cloud.
    let cache = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    let cas = cache.path();
    write_fixture(wd, cas);

    // ---- 1. Cold build ---------------------------------------------------
    run_ok(wd, &["build"]);
    assert_eq!(runs(wd), 1, "cold build MUST execute the body exactly once");
    assert_eq!(
        fs::read_to_string(wd.join("out/hello.txt")).unwrap(),
        EXPECTED_OUTPUT,
        "cold build MUST write the declared output"
    );

    let blobs: Vec<PathBuf> = walk(cas).into_iter().filter(|p| is_blob(p)).collect();
    assert!(
        !blobs.is_empty(),
        "cold build MUST publish at least one CAS blob into the isolated \
         cache_dir; found only {:?}",
        walk(cas)
    );

    // ---- 2. Backdate blobs, snapshot sidecars ----------------------------
    for b in &blobs {
        filetime::set_file_mtime(b, BACKDATED).unwrap();
    }
    let sidecars: Vec<(PathBuf, Vec<u8>, FileTime)> = walk(cas)
        .into_iter()
        .filter(|p| !is_blob(p))
        .map(|p| {
            let bytes = fs::read(&p).unwrap();
            let t = mtime(&p);
            (p, bytes, t)
        })
        .collect();
    assert!(
        sidecars.iter().any(|(p, _, _)| p.to_string_lossy().ends_with(".meta.json")),
        "expected at least one .meta.json sidecar alongside the blobs"
    );

    // ---- 3. Force a genuine backend read ---------------------------------
    // Delete the built output, leave `.cook/` intact: needs_rebuild_cook ->
    // needs_restore -> try_restore -> restore_one -> get_with_meta.
    fs::remove_file(wd.join("out/hello.txt")).unwrap();

    // ---- 4. Warm run: restored, not rebuilt, and the touch fired ---------
    run_ok(wd, &["build"]);
    assert_eq!(
        fs::read_to_string(wd.join("out/hello.txt")).unwrap(),
        EXPECTED_OUTPUT,
        "warm run MUST put the correct bytes back"
    );
    assert_eq!(
        runs(wd),
        1,
        "warm run MUST restore from the cache, not re-execute the body — \
         a second runlog line means the mtime assertion below would be \
         measuring a fresh publish, not a touch"
    );

    let advanced: Vec<&PathBuf> = blobs.iter().filter(|b| mtime(b) > BACKDATED).collect();
    assert!(
        !advanced.is_empty(),
        "COOK-233: a restore MUST bump the CAS blob's mtime (last-access). \
         All {} blob(s) still carry the backdated stamp {BACKDATED}: {:?}",
        blobs.len(),
        blobs
    );

    // ---- 5. Sidecars are never restamped or rewritten --------------------
    for (path, bytes, stamp) in &sidecars {
        assert_eq!(
            mtime(path),
            *stamp,
            "sidecar {} MUST NOT be restamped by a read",
            path.display()
        );
        assert_eq!(
            &fs::read(path).unwrap(),
            bytes,
            "sidecar {} MUST NOT be rewritten by a read",
            path.display()
        );
    }

    // ---- 6. The touch is inert for build reproduction --------------------
    // `cache verify` re-runs cached shell steps in a sandboxed tempdir copy
    // and byte-compares against the local `.cook/cache` index. The
    // verification stage itself never reads the CAS (the subcommand's leading
    // populate build can, since `no_publish` suppresses uploads only), so this
    // proves reproduction is undisturbed, not CAS integrity.
    // CAS integrity is already implied: the warm restore above succeeded, and
    // every restore drains a `VerifyingReader` that fails at EOF on any
    // bytes-vs-sidecar mismatch. Exit status only — no stdout grepping.
    run_ok(wd, &["cache", "verify"]);
}
