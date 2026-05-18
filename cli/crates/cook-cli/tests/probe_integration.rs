//! Probe-units integration tests (CS-0074).
//!
//! End-to-end tests exercising the `cook.probe` API and demand-driven
//! scheduling at the binary level — write a Cookfile in a tempdir,
//! invoke `cook build`, and inspect filesystem outputs and `.cook/cache/`.
//!
//! Coverage:
//!   * `probe_consumer_end_to_end_first_run_then_cache_hit` — a probe and
//!     a consumer unit that references it; verifies the probe value reaches
//!     the consumer, an artifact lands in `.cook/cache/`, and a second run
//!     hits the cache with identical output.
//!   * `probe_unreached_is_not_executed` — a probe no recipe-reachable unit
//!     consumes; verifies demand-driven scheduling prunes it (no
//!     `probe_value` artifact written under `.cook/cache/`).

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

fn run_cook(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
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

/// First run: probe executes, consumer unit reads its value, output
/// file `done.marker` is produced and a probe artifact lands in
/// `.cook/cache/`.  Second run: probe + consumer both cache-hit and
/// `done.marker` is identical.
#[test]
fn probe_consumer_end_to_end_first_run_then_cache_hit() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
register
    cook.probe("test:greet", {
        inputs = {},
        produce = "return { word = \"hello-from-probe\" }",
    })
    cook.add_unit({
        name = "echo",
        inputs = {},
        outputs = {"done.marker"},
        probes = {"test:greet"},
        command = "echo $<test:greet.word> > done.marker",
    })

recipe build
    > cook.sh("cat done.marker")
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    // First run.
    let out1 = run_cook(tmp.path(), &["build"]).expect("first run should succeed");
    let marker = fs::read_to_string(tmp.path().join("done.marker"))
        .expect("done.marker should exist after first run");
    assert!(
        marker.contains("hello-from-probe"),
        "marker should contain probe value; got: {:?}\nstdout: {}\nstderr: {}",
        marker,
        String::from_utf8_lossy(&out1.stdout),
        String::from_utf8_lossy(&out1.stderr)
    );

    // Probe artifact should exist in cache.
    let cache_dir = tmp.path().join(".cook/cache");
    let entries: Vec<_> = fs::read_dir(&cache_dir)
        .unwrap_or_else(|_| panic!("cache dir {} missing", cache_dir.display()))
        .filter_map(|e| e.ok())
        .collect();
    assert!(!entries.is_empty(), "expected at least one cache artifact after first run");

    // Second run — should still succeed and produce the same output.
    let _out2 = run_cook(tmp.path(), &["build"]).expect("second run should succeed");
    let marker2 = fs::read_to_string(tmp.path().join("done.marker")).unwrap();
    assert_eq!(marker, marker2, "probe output should be identical on second run (cache hit)");
}

/// CS-0074 probe-cache regression (SHI-222 Task 4.4 review C1).
///
/// The unified-DAG transitional shim aggregates per-recipe probes into a
/// workspace-level map; the executor consults that map to enable the
/// probe-value cache fast path on subsequent runs. If the shim drops the
/// probes (which an earlier draft did), the cache lookup misses every time
/// and the probe re-executes on every build.
///
/// This test pins the contract with an observable side effect: the probe's
/// produce body appends a single line to `probe-runs.log` each time it
/// runs. After two `cook build` invocations the log MUST contain exactly
/// one line — proving the second run took the cache fast path and did NOT
/// re-invoke the produce body.
///
/// The probe key and produce-source contents are uniquified per test
/// invocation so the host-wide cache (~/.cache/cook/cloud/) cannot serve a
/// stale hit from a prior `cargo test` run — the probe fingerprint folds
/// in both the key and the produce source (§22.5.3).
#[test]
fn probe_produce_does_not_re_execute_on_cache_hit() {
    let tmp = TempDir::new().unwrap();
    // Uniquify the probe key per test invocation so we never collide with
    // a cached probe-value from a prior test run.
    let uniq = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let probe_key = format!("test:counter-{uniq}");
    // Embed the same uniquifier in the produce source itself so the
    // fingerprint differs even from a same-key prior run (key changes
    // already do this, but defence in depth is cheap).
    let cookfile = format!(
        r#"
register
    cook.probe("{probe_key}", {{
        inputs = {{}},
        produce = [[
            -- uniq={uniq}
            local f = io.open("probe-runs.log", "a")
            f:write("ran\n")
            f:close()
            return {{ v = 1 }}
        ]],
    }})
    cook.add_unit({{
        name = "consume",
        inputs = {{}},
        outputs = {{"done.marker"}},
        probes = {{"{probe_key}"}},
        command = "echo $<{probe_key}.v> > done.marker",
    }})

recipe build
    > cook.sh("cat done.marker")
"#
    );
    fs::write(tmp.path().join("Cookfile"), &cookfile).unwrap();

    // First run: produce body MUST execute (cache miss).
    run_cook(tmp.path(), &["build"]).expect("first run should succeed");
    let log1 = fs::read_to_string(tmp.path().join("probe-runs.log"))
        .expect("probe-runs.log should exist after first run");
    assert_eq!(
        log1, "ran\n",
        "first run: produce body should have executed exactly once, got log: {log1:?}"
    );

    // Second run: probe-cache fast path MUST be taken — produce body MUST
    // NOT re-execute. If the workspace-level `probes` map is empty (the
    // C1 bug), the executor can't find the ProbeUnit by key, falls
    // through to fresh execution, and the log grows to "ran\nran\n".
    run_cook(tmp.path(), &["build"]).expect("second run should succeed");
    let log2 = fs::read_to_string(tmp.path().join("probe-runs.log")).unwrap();
    assert_eq!(
        log2, "ran\n",
        "second run: probe MUST hit cache and NOT re-execute produce body; \
         got log: {log2:?} (expected \"ran\\n\")"
    );
}

/// Demand-driven scheduling: a probe that no recipe-reachable unit references
/// MUST NOT be executed and MUST NOT write a probe-value artifact to
/// `.cook/cache/`.
///
/// Locks the §22.5.7 demand-driven scheduling contract at the binary level:
/// declaring `cook.probe("test:unused", ...)` in the register phase is not
/// sufficient to trigger its execution — only consumer demand (a unit with
/// `probes = {...}` reachable from the requested recipe) causes the probe
/// to run.
///
/// Detection scheme: walk `.cook/cache/` and inspect every `*.meta.json`
/// sidecar; an `ArtifactMeta` with `kind = Some("probe_value")` serializes
/// to JSON containing the substring `"kind":"probe_value"`. The presence of
/// that substring anywhere under `.cook/cache/` would indicate the probe
/// ran and persisted its output. A missing `.cook/cache/` directory is a
/// valid pass (no work executed at all).
#[test]
fn probe_unreached_is_not_executed() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
register
    cook.probe("test:unused", {
        inputs = {},
        produce = "return { v = 1 }",
    })

recipe build
    > cook.sh("echo hello")
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    // `cook build` must succeed: the recipe body doesn't depend on the probe,
    // so demand-driven scheduling should prune the probe entirely.
    run_cook(tmp.path(), &["build"]).expect("cook build should succeed");

    // No probe-value artifact should have been persisted. If `.cook/cache/`
    // doesn't exist at all, the assertion trivially holds.
    let cache_dir = tmp.path().join(".cook").join("cache");
    if cache_dir.exists() {
        let mut found = None;
        for entry in walkdir::WalkDir::new(&cache_dir).into_iter().flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().and_then(|s| s.to_str()) == Some("json")
                && path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| n.ends_with(".meta.json"))
                    .unwrap_or(false)
            {
                if let Ok(content) = fs::read_to_string(path) {
                    // ArtifactMeta with kind = Some("probe_value") serializes
                    // to JSON containing this exact substring.
                    if content.contains("\"kind\":\"probe_value\"") {
                        found = Some(path.to_path_buf());
                        break;
                    }
                }
            }
        }
        assert!(
            found.is_none(),
            "unreached probe must not write a probe-value artifact under .cook/cache/, \
             but found one at: {}",
            found.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
        );
    }
}
