//! Regression test: `cook test` must not silently execute or re-record
//! non-test recipes.
//!
//! Before the fix (see the commit that scopes `cmd_test`'s bare/namespace
//! roots to test-bearing recipes), a bare `cook test` rooted the run at
//! EVERY non-chore recipe in the workspace — not just the ones declaring
//! `test` steps. Non-test recipes then executed invisibly under the test
//! reporter (only `test ... ok|FAILED` lines are shown) and their local
//! cache indexes (`.cook/cache/<recipe>.toml`) advanced, so a later
//! `cook <recipe>` reported `cached` on work the user never actually saw
//! run — a false cache hit hiding genuinely stale output.
//!
//! These tests drive the real `cook` binary end-to-end and assert that:
//!   1. `cook test` still runs the tests themselves (and their real
//!      dependency closures), and
//!   2. `cook test` does NOT execute or re-record cache state for
//!      non-test recipes that merely happen to sit in the workspace,
//!      whether at the bare-scope or namespace-scope level.

use std::fs;
use std::process::Command;
use tempfile::tempdir;

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

/// Isolate the persistent shared content-addressed cache
/// (`~/.cache/cook/cloud`): it hashes by content, not path, so a shell
/// step's side effects (e.g. an `echo >> log` appended by the step body)
/// would be silently skipped on a cache HIT served from a *previous test
/// run's* identical content — even though this test creates a brand-new
/// tempdir every time. Point the local backend's cache_dir at the tempdir
/// itself, and set XDG_CACHE_HOME as defense-in-depth for any fallback
/// path resolution. See `surface_conformance.rs` for the same pattern.
fn isolate_cache(root: &std::path::Path, cache_root: &std::path::Path) {
    fs::create_dir_all(root.join(".cook")).unwrap();
    fs::write(
        root.join(".cook/cloud.toml"),
        format!(
            "[cache]\ncache_dir = \"{}\"\n",
            cache_root.join("cache").display()
        ),
    )
    .unwrap();
}

fn run(dir: &std::path::Path, cache_root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_binary())
        .args(args)
        .current_dir(dir)
        .env("XDG_CACHE_HOME", cache_root.join("xdg"))
        .output()
        .unwrap()
}

fn combined(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn count_lines(path: &std::path::Path) -> usize {
    fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

#[test]
fn bare_cook_test_does_not_execute_or_rerecord_unrelated_recipes() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Workspace marker.
    fs::write(root.join(".cookroot"), "").unwrap();
    isolate_cache(root, root);

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/in.txt"), "content-v1").unwrap();

    fs::write(
        root.join("Cookfile"),
        r#"recipe build
    ingredients "src/in.txt"
    cook "dist/out.txt" { mkdir -p dist && cp src/in.txt $<out> }

recipe typecheck: build
    cook "build2/tc.stamp" { echo x >> tc-runs.log; cat $<build> > /dev/null && mkdir -p build2 && echo ok > $<out> }

recipe smoke: build
    test { : $<build>; true }
"#,
    )
    .unwrap();

    let tc_runs_log = root.join("tc-runs.log");
    let typecheck_cache = root.join(".cook/cache/typecheck.toml");

    // Step 1: baseline — `cook typecheck` then `cook test` — typecheck
    // executes exactly once (from the direct `cook typecheck` invocation;
    // `cook test` must not re-execute it since `typecheck` declares no
    // test steps).
    let out = run(root, root, &["typecheck"]);
    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "cook typecheck should exit 0; {}",
        combined(&out)
    );

    let out = run(root, root, &["test"]);
    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "cook test should exit 0; {}",
        combined(&out)
    );

    assert_eq!(
        count_lines(&tc_runs_log),
        1,
        "baseline: typecheck should have executed exactly once after `cook typecheck` + `cook test`"
    );

    // Step 2: mutate the shared dependency's content, then rebuild `build`
    // directly (not through typecheck) so dist/out.txt reflects the new
    // content while typecheck's recorded fingerprints still reference the
    // old content.
    fs::write(root.join("src/in.txt"), "content-v2").unwrap();

    let out = run(root, root, &["build"]);
    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "cook build should exit 0; {}",
        combined(&out)
    );

    // Step 3: snapshot typecheck's cache index, then run bare `cook test`.
    let cache_snapshot =
        fs::read(&typecheck_cache).expect("typecheck cache index should exist after step 1");

    let out = run(root, root, &["test"]);
    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "bare cook test should exit 0; {}",
        combined(&out)
    );
    let out_text = combined(&out);
    assert!(
        out_text.contains("test smoke@"),
        "expected the smoke test itself to still run; output:\n{out_text}"
    );

    let cache_after = fs::read(&typecheck_cache)
        .expect("typecheck cache index should still exist after `cook test`");
    assert_eq!(
        cache_snapshot, cache_after,
        "cook test must not (re)record fingerprints for typecheck, which it did not execute \
         (it declares no test steps and is not a dependency of any test recipe)"
    );

    assert_eq!(
        count_lines(&tc_runs_log),
        1,
        "typecheck must not have executed during bare `cook test`"
    );

    // Step 4: an explicit `cook typecheck` must genuinely re-run on the
    // edited content — no false cache HIT from the (non-)run above.
    let out = run(root, root, &["typecheck"]);
    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "cook typecheck (follow-up) should exit 0; {}",
        combined(&out)
    );

    assert_eq!(
        count_lines(&tc_runs_log),
        2,
        "typecheck should genuinely re-execute on the edited dependency content \
         (no false cache hit from the earlier `cook test` run)"
    );
}

#[test]
fn namespace_scoped_cook_test_skips_non_test_recipes() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    fs::write(root.join(".cookroot"), "").unwrap();
    isolate_cache(root, root);

    fs::write(root.join("Cookfile"), "import sub ./sub\n").unwrap();

    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(
        root.join("sub/Cookfile"),
        r#"recipe pass
    test { true }

recipe sidework
    cook "out/side.txt" { mkdir -p out && echo side > $<out> }
"#,
    )
    .unwrap();

    let out = run(root, root, &["test", "sub"]);
    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "cook test sub should exit 0; {}",
        combined(&out)
    );
    let out_text = combined(&out);
    assert!(
        out_text.contains("test pass@"),
        "expected sub.pass test to run; output:\n{out_text}"
    );

    let side_output = root.join("sub/out/side.txt");
    assert!(
        !side_output.exists(),
        "namespace-scoped cook test must not build non-test recipe sub.sidework \
         (found unexpected output at {})",
        side_output.display()
    );
}
