//! Binary-level proof that a ZERO-UNIT meta-target recipe in the middle of a
//! transitive cross-recipe chain still preserves ordering.
//!
//! Repro shape:
//!
//!   recipe producer              # outputs build/gen.a
//!   recipe middle : producer     # ZERO units — pure meta-target
//!   recipe consumer : middle     # reads build/gen.a
//!
//! Before the fix (`cli/crates/cook-engine/src/dag_builder.rs`), the
//! leaf-recording site at the end of `build_dag`'s per-recipe loop recorded
//! `middle`'s leaves as its (empty) sequential barrier, discarding the
//! `cross_deps` it inherited from `producer`. That severed the transitive
//! edge: `consumer` never waited on `producer`, and the run failed with
//! `cp: cannot stat 'build/gen.a'` (or, on a lucky scheduling, raced and
//! sometimes read a stale/partial file).
//!
//! This test is deliberately paranoid about the CAS cache: a broken build in
//! a *fresh* directory can still appear to pass if the content-addressed
//! cache is consulted and a prior run (from this test or any other) already
//! populated an entry under the same key — the consumer would "succeed" by
//! serving a cached artifact rather than by correctly ordering against a
//! freshly-run producer. Two independent defenses guard against that:
//!
//!   1. `isolate_cache` (same pattern as `raw_path_cross_recipe_edge.rs`)
//!      points `.cook/cloud.toml`'s `cache_dir` at a private per-test
//!      directory, so the global `~/.cache/cook/cloud` store is never
//!      consulted.
//!   2. The producer's payload embeds a fresh nonce every run, so even if
//!      cache isolation ever regressed, the content-addressed key could not
//!      collide with any prior run's key.
//!
//! The assertion is positive ordering evidence, not a bare exit-0 check:
//! the producer sleeps briefly (so a race would be observable), and the
//! test asserts the copied file's contents equal the nonce actually written
//! by *this* run's producer step.

use std::fs;

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

/// Point the cook cache at a private per-test directory so this run never
/// consults (or is masked by) the system-wide local backend
/// (~/.cache/cook/cloud).
fn isolate_cache(wd: &std::path::Path) {
    let cache_dir = wd.join(".cook/local-cache");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

/// Run `cook <recipe>` in `wd`, returning (success, combined stdout+stderr).
fn run_cook(wd: &std::path::Path, recipe: &str) -> (bool, String) {
    let out = std::process::Command::new(cook_binary())
        .arg(recipe)
        .current_dir(wd)
        .output()
        .expect("cook invocation");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), combined)
}

#[test]
fn zero_unit_meta_target_preserves_transitive_ordering() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    isolate_cache(wd);

    // A fresh nonce per test invocation — belt-and-suspenders against CAS
    // collisions even if cache isolation above ever regressed. Combine a
    // process-unique value (PID) with a wall-clock timestamp in nanoseconds.
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    fs::write(
        wd.join("Cookfile"),
        format!(
            r#"recipe producer
    cook "build/gen.a" {{ sleep 1; echo UNIQUE-{nonce} > $<out> }}

recipe middle : producer

recipe consumer : middle
    cook "build/copy.a" {{ cp build/gen.a $<out> }}
"#,
            nonce = nonce
        ),
    )
    .unwrap();

    let (ok, combined) = run_cook(wd, "consumer");

    assert!(
        ok,
        "consumer MUST succeed: the zero-unit `middle` meta-target must \
         still forward producer's leaves so consumer waits on producer:\n{combined}"
    );

    // Positive ordering evidence: producer's unit line appears before
    // consumer's unit line in the log, not merely "exit 0". `str::find`
    // returns the first occurrence of each, so this only shows producer's
    // line came first — it is not by itself proof of completion ordering.
    // The nonce-equality check below is what actually proves consumer read
    // producer's output rather than a stale or racing write.
    let producer_at = combined
        .find("producer/build/gen.a")
        .unwrap_or_else(|| panic!("producer unit never ran:\n{combined}"));
    let consumer_at = combined
        .find("consumer/build/copy.a")
        .unwrap_or_else(|| panic!("consumer unit never ran:\n{combined}"));
    assert!(
        producer_at < consumer_at,
        "producer's unit MUST complete before consumer's unit begins:\n{combined}"
    );

    // Content evidence: the copy consumer produced must carry THIS run's
    // nonce — proving consumer read a file that THIS producer actually
    // wrote, not a stale/partial file from a race, and not a cache hit from
    // some other run (which could not have this nonce).
    let copy_path = wd.join("build/copy.a");
    let copy_contents = fs::read_to_string(&copy_path).unwrap_or_else(|e| {
        panic!("consumer's output build/copy.a must exist: {e}\n{combined}")
    });
    let expected = format!("UNIQUE-{nonce}");
    assert_eq!(
        copy_contents.trim(),
        expected,
        "consumer's copy MUST carry this run's producer nonce — a mismatch \
         means consumer read something other than this run's producer output:\n{combined}"
    );
}
