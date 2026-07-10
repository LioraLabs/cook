//! COOK-84 end-to-end: editing a source file invalidates cached test
//! results WITHOUT `--rerun`.
//!
//! Before the fix, the upfront test fingerprint hashed no file content at
//! all (`cook_outputs`/`dep_outputs` were stubbed empty), so a broken
//! source still replayed a stale `ok (cached)` result. The fingerprint now
//! hashes the test's transitive source closure: its own ingredients plus
//! every predecessor unit's declared inputs and unit identity, EXCLUDING
//! predecessor-produced artifacts (stale at fingerprint time) so the
//! fingerprint is stable across the edit→rebuild boundary.

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

/// Point the cook cache at a private directory so test runs sharing the same
/// source content / command hash do not collide on artifact keys in the
/// system-wide local backend (`~/.cache/cook/cloud`).
fn write_isolated_cache_config(wd: &std::path::Path, cache_dir: &std::path::Path) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

fn write_fixture(wd: &std::path::Path) {
    fs::write(
        wd.join("Cookfile"),
        r#"recipe lib
    ingredients "src/lib.txt"
    cook "build/lib.txt" {
        mkdir -p build
        cp src/lib.txt build/lib.txt
    }

recipe unit_direct
    ingredients "src/lib.txt"
    test { grep -qx ok src/lib.txt }

recipe consumer
    test { grep -qx ok $<lib> }

recipe untouched
    ingredients "src/other.txt"
    test { grep -qx stable src/other.txt }
"#,
    )
    .unwrap();
    fs::create_dir_all(wd.join("src")).unwrap();
    fs::write(wd.join("src/lib.txt"), "ok\n").unwrap();
    fs::write(wd.join("src/other.txt"), "stable\n").unwrap();
}

fn run_cook_test(wd: &std::path::Path) -> Output {
    std::process::Command::new(cook_binary())
        .arg("test")
        .current_dir(wd)
        .output()
        .expect("cook test invocation")
}

fn combined(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn source_edit_invalidates_cached_test_without_rerun() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_tmp = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_fixture(wd);
    write_isolated_cache_config(wd, cache_tmp.path());

    // Run 1 (cold): everything runs fresh, nothing cached.
    let out1 = run_cook_test(wd);
    let text1 = combined(&out1);
    assert!(
        out1.status.success(),
        "run1 (cold) should pass. output:\n{text1}"
    );
    assert!(
        !text1.contains("(cached)"),
        "run1 (cold) should have no cache hits. output:\n{text1}"
    );

    // Run 2 (warm): every test replays from cache.
    let out2 = run_cook_test(wd);
    let text2 = combined(&out2);
    assert!(
        out2.status.success(),
        "run2 (warm) should pass. output:\n{text2}"
    );
    assert!(
        text2.contains("3 passed (3 cached)"),
        "run2 (warm) should replay all 3 tests from cache. output:\n{text2}"
    );

    // Break the shared source. NO --rerun: the fingerprint alone must
    // notice — this is the COOK-84 bug being fixed.
    fs::write(wd.join("src/lib.txt"), "broken\n").unwrap();
    let out3 = run_cook_test(wd);
    let text3 = combined(&out3);
    assert!(
        !out3.status.success(),
        "run3 (broken source, no --rerun) MUST FAIL — a stale cached pass \
         means the fingerprint ignored the source edit. output:\n{text3}"
    );
    assert!(
        text3.contains("1 cached"),
        "run3 should keep `untouched` cached (targeted invalidation, not a \
         full bust). output:\n{text3}"
    );

    // Restore: everything passes again.
    fs::write(wd.join("src/lib.txt"), "ok\n").unwrap();
    let out4 = run_cook_test(wd);
    let text4 = combined(&out4);
    assert!(
        out4.status.success(),
        "run4 (restored source) should pass. output:\n{text4}"
    );

    // Run 5: fingerprint stability — artifacts rebuilt in run4 must not
    // shift the fingerprint (guards the stale-artifact exclusion; no
    // once-per-edit oscillation).
    let out5 = run_cook_test(wd);
    let text5 = combined(&out5);
    assert!(
        out5.status.success(),
        "run5 (stable) should pass. output:\n{text5}"
    );
    assert!(
        text5.contains("3 passed (3 cached)"),
        "run5 should replay all 3 tests from cache — fingerprints must be \
         stable once artifacts are rebuilt. output:\n{text5}"
    );
}
