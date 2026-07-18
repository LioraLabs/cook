//! COOK-246 — the raw-path output<->input cross-recipe edge, both halves.
//!
//! `build_dag` (`cook-engine/src/dag_builder.rs`) forms cross-recipe edges ONLY
//! from coarse `deps` (recipe-header colon syntax) and name-keyed `dep_edges`
//! (the `$<sigil>` / `cook.dep_output` path). There is no output-path ->
//! producing-unit matching anywhere in edge formation. The consequence splits:
//!
//!   * ORDERING does NOT come from a literal path match  -> test 1 (a GAP)
//!   * CONTENT INVALIDATION DOES fold through a literal path -> test 3 (holds)
//!
//! Per Standard App. C.17.1 this split is the documented model, not a
//! deviation: literal `outputs[]`/`inputs[]` give file-content invalidation
//! propagation, while ordering comes from a recipe dep ("tasks depend on
//! tasks, not files"). §22.1.2's glob-terminality rule exists precisely
//! because globs cannot be lexically matched; the literal case is neither
//! wired for ordering nor diagnosed.
//!
//! These tests pin reality so the gap stays visible and cannot silently
//! regress or be papered over.

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

/// Point the cook cache at a private per-test directory so test runs sharing
/// the same source content / command hash do not collide on artifact keys in
/// the system-wide local backend (~/.cache/cook/cloud).
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

// ---------------------------------------------------------------------------
// Test 1 — the GAP
// ---------------------------------------------------------------------------

/// THIS TEST DOCUMENTS A GAP, NOT A DESIRED BEHAVIOUR.
///
/// `producer` declares the literal output `build/gen.a`; `consumer` declares
/// the literal input `build/gen.a` and has NO other link to it — no recipe
/// header dep, no `$<sigil>`. Invoking only `consumer` builds a closure of
/// exactly ONE node: the producer is never scheduled, and the run dies at
/// `cp: cannot stat 'build/gen.a'`.
///
/// Standard refs: App. C.17.1 (literal paths give content invalidation;
/// ordering comes from a recipe dep) and §22.1.2 (the glob-terminality rule,
/// whose `is_terminal_output` gate means the literal case is not diagnosed
/// either — so this is a silent stale-read under `--jobs > 1`).
///
/// IF THIS TEST EVER GOES RED because raw-path ordering now works, that is a
/// deliberate change to the dependency model and REQUIRES a CS entry in
/// `standard/src/content/docs/appendix/E-changes.mdx`. It is NOT a test to
/// "fix" — update it only alongside that normative decision.
#[test]
fn raw_path_input_does_not_order_producer_before_consumer() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    isolate_cache(wd);

    fs::write(
        wd.join("Cookfile"),
        r#"recipe producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "mkdir -p build && cp src.c build/gen.a",
        })

recipe consumer
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    )
    .unwrap();
    fs::write(wd.join("src.c"), b"seed-content\n").unwrap();

    let (ok, combined) = run_cook(wd, "consumer");

    assert!(
        !ok,
        "consumer MUST fail: nothing schedules producer, so build/gen.a never exists.\n{combined}"
    );

    // The producer never ran: its declared output does not exist on disk.
    // (We deliberately do NOT assert on the underlying `cp: cannot stat ...`
    // wording here — that is GNU coreutils phrasing, differs on BSD/macOS,
    // and shifts under LC_ALL. `!ok` above plus the on-disk absence check
    // and the `!combined.contains("producer")` check below together already
    // prove the run failed specifically because the producer never ran and
    // never produced `build/gen.a` — not merely that the run exited
    // non-zero, and not because of some unrelated error. A register-time
    // diagnostic naming the producer would trip the `!contains("producer")`
    // assertion below, which is itself a change worth a CS entry.)
    assert!(
        !wd.join("build/gen.a").exists(),
        "producer MUST NOT have run — if build/gen.a exists, raw-path ordering \
         now works and this test's premise (COOK-246) is obsolete:\n{combined}"
    );

    // Exactly one recipe was queued — the producer was never scheduled
    // alongside the consumer. (`(1 nodes)` on the `queued` line is the
    // per-recipe unit count, NOT the closure size, so it does not
    // discriminate here: in `sigil_path_does_order_producer_before_consumer`
    // below, BOTH recipes print `queued  (1 nodes)` even though that
    // closure has two recipes. Counting `queued` lines does discriminate:
    // that control run prints two of them.)
    assert_eq!(
        combined.matches("queued").count(),
        1,
        "exactly one recipe (consumer) MUST be queued to run — if a second \
         `queued` line appears, the producer was scheduled and this test's \
         premise (COOK-246) is obsolete:\n{combined}"
    );
    assert!(
        !combined.contains("producer"),
        "the producer MUST NOT appear in the build at all:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — the control that gives test 1 its meaning
// ---------------------------------------------------------------------------

/// The `$<producer>` sigil — the proven cross-recipe path — DOES order the
/// producer before the consumer over the very same pair of recipes. This
/// contrast is what makes test 1 unambiguous: the failure there is about the
/// *linking mechanism*, not about the recipes being malformed.
#[test]
fn sigil_path_does_order_producer_before_consumer() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    isolate_cache(wd);

    fs::write(
        wd.join("Cookfile"),
        r#"recipe producer
    ingredients "src.c"
    cook "build/gen.a" {
        mkdir -p build
        cp $<in> $<out>
    }

recipe consumer
    ingredients "src.c"
    cook "out.bin" {
        cp $<producer> $<out>
    }
"#,
    )
    .unwrap();
    fs::write(wd.join("src.c"), b"PRODUCER-BYTES-v1\n").unwrap();

    let (ok, combined) = run_cook(wd, "consumer");

    assert!(ok, "the sigil path MUST schedule producer then consumer:\n{combined}");

    // Both recipes are in the closure.
    assert!(
        combined.contains("2 nodes"),
        "closure MUST hold both producer and consumer:\n{combined}"
    );

    // Ordering: producer's unit completes before consumer's.
    let producer_at = combined
        .find("producer/build/gen.a")
        .unwrap_or_else(|| panic!("producer unit never ran:\n{combined}"));
    let consumer_at = combined
        .find("consumer/out.bin")
        .unwrap_or_else(|| panic!("consumer unit never ran:\n{combined}"));
    assert!(
        producer_at < consumer_at,
        "producer MUST complete before consumer:\n{combined}"
    );

    // The consumer's output carries the producer's bytes — the edge is real,
    // not just an ordering coincidence.
    let out = fs::read_to_string(wd.join("out.bin")).expect("out.bin must exist");
    assert_eq!(
        out.trim(),
        "PRODUCER-BYTES-v1",
        "consumer's output MUST carry the producer's bytes:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — the half that DOES hold
// ---------------------------------------------------------------------------

/// With ordering supplied by a recipe header dep (`recipe consumer : producer`),
/// the raw path `build/gen.a` in the consumer's `inputs` folds that file's
/// on-disk content hash into the consumer's cache key (`hash_input_paths` ->
/// `cloud_key`). Changing the producer's SOURCE changes `build/gen.a`'s bytes,
/// which MUST invalidate the consumer rather than serve a stale artifact.
///
/// The no-op rebuild in the middle is load-bearing: it proves caching is
/// actually engaged here, so the final assertion is a real invalidation
/// signal and not a consumer that happens to re-run unconditionally.
#[test]
fn raw_path_input_folds_into_consumer_cache_key() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    isolate_cache(wd);

    // Consumer's ONLY cache-key link to the producer's bytes is the raw path
    // in `inputs`; the header dep supplies ordering and nothing else.
    fs::write(
        wd.join("Cookfile"),
        r#"recipe producer
    ingredients "src.c"
    cook "build/gen.a" {
        mkdir -p build
        cp $<in> $<out>
    }

recipe consumer : producer
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    )
    .unwrap();
    fs::write(wd.join("src.c"), b"BYTES-v1\n").unwrap();

    // --- Build 1: cold.
    let (ok, combined) = run_cook(wd, "consumer");
    assert!(ok, "cold build MUST succeed:\n{combined}");
    assert_eq!(
        fs::read_to_string(wd.join("out.bin")).unwrap().trim(),
        "BYTES-v1",
        "cold build must carry the original bytes:\n{combined}"
    );

    // --- Build 2: no-op. Proves the cache is engaged, so that build 3's
    // re-run below is a genuine invalidation and not an unconditional rebuild.
    let (ok, combined) = run_cook(wd, "consumer");
    assert!(ok, "no-op rebuild MUST succeed:\n{combined}");
    assert!(
        combined.contains("2 cached recipes"),
        "no-op rebuild MUST hit cache for both recipes — otherwise this test \
         cannot distinguish invalidation from an always-rebuild:\n{combined}"
    );

    // --- Mutate the producer's source, so build/gen.a's bytes change.
    fs::write(wd.join("src.c"), b"BYTES-v2-CHANGED\n").unwrap();

    // --- Build 3: the consumer MUST re-run and pick up the new bytes.
    let (ok, combined) = run_cook(wd, "consumer");
    assert!(ok, "rebuild after source change MUST succeed:\n{combined}");
    assert!(
        combined.contains("0 cached recipes"),
        "a change to the producer's output MUST invalidate the consumer via the \
         raw-path content fold:\n{combined}"
    );
    assert_eq!(
        fs::read_to_string(wd.join("out.bin")).unwrap().trim(),
        "BYTES-v2-CHANGED",
        "consumer MUST serve the NEW producer bytes, not a cached artifact:\n{combined}"
    );
}
