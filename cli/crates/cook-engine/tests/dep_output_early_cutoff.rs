//! COOK-211 end-to-end: a byte-identical dependency OUTPUT leaves a consuming
//! TEST cached (early cutoff), exactly as it already does for a consuming
//! `cook` step.
//!
//! Before the fix, a `test`-step unit folded its dependency's EXECUTION
//! identity (command hash + the dep's own source inputs) rather than the
//! dependency's OUTPUT CONTENT. So any change that merely re-ran the dep —
//! even one producing byte-identical output — busted the cached test result,
//! while a sibling `cook` step consuming the same dep via `$<lib>` stayed
//! cached. §17.4 rule 1 mandates the content fold; this pins it.

use std::fs;
use std::process::Output;

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

/// Point the cook cache at a private directory so runs sharing source content /
/// command hashes do not collide on artifact keys in the system-wide backend.
fn write_isolated_cache_config(wd: &std::path::Path, cache_dir: &std::path::Path) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

/// `lib` copies `src.txt` to its output with `copy_cmd`; `useref` (cook step)
/// and `tcheck` (test step) both consume `$<lib>`. Swapping `copy_cmd` between
/// two commands that yield byte-identical output exercises the early cutoff.
fn write_fixture(wd: &std::path::Path, copy_cmd: &str) {
    fs::write(
        wd.join("Cookfile"),
        format!(
            r#"recipe lib
    ingredients "src.txt"
    cook "build/lib.txt" {{
        mkdir -p build
        {copy_cmd}
    }}

recipe useref: lib
    cook "build/useref.txt" {{
        : $<lib>
        printf x > $<out>
    }}

recipe tcheck: lib
    ingredients "t.txt"
    test {{ : $<lib>; true }}
"#
        ),
    )
    .unwrap();
    fs::write(wd.join("src.txt"), "one\n").unwrap();
    fs::write(wd.join("t.txt"), "t\n").unwrap();
}

fn run(wd: &std::path::Path, args: &[&str]) -> Output {
    std::process::Command::new(cook_binary())
        .args(args)
        .current_dir(wd)
        .output()
        .expect("cook invocation")
}

fn combined(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn byte_identical_dep_output_keeps_consuming_test_cached() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_tmp = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    // Variant A: lib copies with `cp`.
    write_fixture(wd, "cp src.txt build/lib.txt");
    write_isolated_cache_config(wd, cache_tmp.path());

    // Run 1 (cold): build the cook consumer + run the test to populate caches.
    let cold_build = run(wd, &["useref"]);
    assert!(
        cold_build.status.success(),
        "cold build should pass. output:\n{}",
        combined(&cold_build)
    );
    let cold_test = run(wd, &["test"]);
    assert!(
        cold_test.status.success(),
        "cold test should pass. output:\n{}",
        combined(&cold_test)
    );

    // Run 2 (warm, no change): both the cook consumer and the test replay.
    let warm_test = run(wd, &["test"]);
    let warm_text = combined(&warm_test);
    assert!(
        warm_text.contains("1 passed (1 cached)"),
        "warm test should replay from cache. output:\n{warm_text}"
    );

    // Change lib's COMMAND to `cat > …` — different command text, byte-identical
    // output (still a copy of src.txt).
    let sha_before = fs::read(wd.join("build/lib.txt")).unwrap();
    write_fixture(wd, "cat src.txt > build/lib.txt");

    // Run 3: lib re-runs (command changed) but produces byte-identical output.
    // CONTROL — the sibling cook step `useref` must stay cached (early cutoff).
    let build3 = run(wd, &["useref"]);
    let build3_text = combined(&build3);
    assert!(
        build3.status.success(),
        "run3 build should pass. output:\n{build3_text}"
    );
    assert_eq!(
        fs::read(wd.join("build/lib.txt")).unwrap(),
        sha_before,
        "lib output must be byte-identical across the command change"
    );
    assert!(
        build3_text.contains("1 cached recipes"),
        "CONTROL: sibling cook step `useref` must stay cached via early cutoff \
         when lib's output is byte-identical. output:\n{build3_text}"
    );

    // THE FIX: the test consuming the same `$<lib>` must ALSO stay cached —
    // its key folds lib's OUTPUT CONTENT, not lib's execution identity.
    let test3 = run(wd, &["test"]);
    let test3_text = combined(&test3);
    assert!(
        test3.status.success(),
        "run3 test should pass. output:\n{test3_text}"
    );
    assert!(
        test3_text.contains("1 passed (1 cached)"),
        "test `tcheck` must stay cached when lib's OUTPUT is byte-identical \
         (early cutoff), matching the sibling cook step. output:\n{test3_text}"
    );
}
