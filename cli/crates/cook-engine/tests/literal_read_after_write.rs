//! §16.1.2 — the literal-output→literal-input read-after-write diagnostic.
//!
//! When recipe `A` declares a literal `outputs[]` entry and recipe `B` declares
//! the SAME path as a literal `inputs[]` entry, §10.6 forms no ordering edge
//! ("Only name references create edges … an implementation MUST NOT infer an
//! edge from path-string equality"). That is correct and stays correct: these
//! tests prove the engine emits a **diagnostic**, never a synthesised edge.
//!
//! Two properties carry the weight here:
//!
//!   * **DIRECTED** reachability, not §16.1.1's undirected predicate. §16.1.1's
//!     write-write collision is serialised by EITHER ordering, so any path
//!     suffices. Read-after-write is asymmetric: only `B requires A` is
//!     correct. The reverse path (`A requires B`) orders the reader BEFORE the
//!     write — a deterministic stale read that an undirected predicate would
//!     wave through. See `reverse_direction_path_still_diagnoses`.
//!
//!   * **CLOSURE-scoped**, not workspace-wide. A literal output is not
//!     build-owned, so `cook producer && cook consumer` is legitimate and MUST
//!     NOT be rejected. See `producer_outside_closure_does_not_fire` — the
//!     same scenario `raw_path_cross_recipe_edge.rs` pins.

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
/// the system-wide local backend.
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

fn setup(cookfile: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    isolate_cache(wd);
    fs::write(wd.join("Cookfile"), cookfile).unwrap();
    fs::write(wd.join("src.c"), b"seed-content\n").unwrap();
    tmp
}

/// Assert the §16.1.2 diagnostic fired, naming both recipes and the path.
///
/// The `cannot stat` assertion is load-bearing, not decoration. Without the
/// check, this exact fixture ALSO exits non-zero — the consumer runs and dies
/// on a missing `build/gen.a`, printing both recipe names and the path along
/// the way. `!ok` plus a name match would therefore pass against an
/// unmodified engine and prove nothing. Requiring that execution never
/// started is what makes this a PLAN-TIME rejection rather than the
/// pre-existing runtime failure wearing a new hat.
fn assert_diagnosed(ok: bool, combined: &str) {
    assert!(
        !ok,
        "the plan MUST be rejected — a literal read-after-write with no \
         ordering path is a silent stale read under --jobs > 1:\n{combined}"
    );
    assert!(
        combined.contains("read-after-write with no ordering edge"),
        "MUST be the §16.1.2 plan-time diagnostic:\n{combined}"
    );
    assert!(
        combined.contains("build/gen.a"),
        "the diagnostic MUST name the shared path:\n{combined}"
    );
    assert!(
        combined.contains("producer") && combined.contains("consumer"),
        "the diagnostic MUST name BOTH recipes:\n{combined}"
    );
    // Plan time: rejected BEFORE any work dispatched.
    assert!(
        !combined.contains("cannot stat"),
        "the rejection MUST happen at PLAN time — reaching the runtime \
         `cp: cannot stat` means the check did not fire and this test is \
         passing on the pre-existing failure instead:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// FIRES — the core case
// ---------------------------------------------------------------------------

/// Producer declares literal output `build/gen.a`; consumer declares the same
/// path as a literal input. Both are pulled into one closure by an aggregator
/// (`all : producer consumer`), and consumer does NOT require producer. Under
/// `--jobs > 1` these race; the plan MUST be rejected at plan time.
///
/// Crucially the aggregator supplies NO ordering between its two deps — a
/// recipe's dep list is a set, not a sequence.
#[test]
fn closure_mates_with_no_path_are_diagnosed() {
    let tmp = setup(
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

recipe all : producer consumer
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "all");
    assert_diagnosed(ok, &combined);

    // A DIAGNOSTIC, NOT AN INFERRED EDGE: the build is rejected outright, so
    // nothing ran and no artifact exists. If `out.bin` were here, the engine
    // had synthesised the §10.6-forbidden ordering instead of diagnosing.
    assert!(
        !tmp.path().join("out.bin").exists(),
        "the build MUST be rejected, NOT reordered — synthesising an edge \
         would reverse §10.6:\n{combined}"
    );
}

/// The fix hint must actually tell the author what to do — all three routes.
#[test]
fn diagnostic_names_the_three_fixes() {
    let tmp = setup(
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

recipe all : producer consumer
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "all");
    assert!(!ok, "{combined}");
    assert!(
        combined.contains(": producer"),
        "hint MUST offer the recipe-header dep fix:\n{combined}"
    );
    assert!(
        combined.contains("$<producer>"),
        "hint MUST offer the sigil fix:\n{combined}"
    );
    assert!(
        combined.contains("cook.require_recipe(\"producer\")"),
        "hint MUST offer the require_recipe fix:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// FIRES — the reverse-direction case: THE reason this predicate is directed
// ---------------------------------------------------------------------------

/// **This is the test that distinguishes §16.1.2 from §16.1.1.**
///
/// Here `producer : consumer` — the PRODUCER requires the CONSUMER. There IS
/// an ordering path between the two recipes, so §16.1.1's undirected
/// `connected()` predicate would report "dep-related, fine" and wave this
/// through. But the direction is backwards: `producer : consumer` means the
/// consumer executes FIRST, so it reads `build/gen.a` before the producer has
/// written it. That is not an ambiguous race that scheduling might win — it is
/// a DETERMINISTIC stale/missing read, strictly worse than the unordered case.
///
/// §16.1.2 therefore passes iff `consumer` transitively requires `producer`.
/// The reverse path is not exculpatory and MUST still diagnose.
#[test]
fn reverse_direction_path_still_diagnoses() {
    let tmp = setup(
        r#"recipe consumer
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })

recipe producer : consumer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "mkdir -p build && cp src.c build/gen.a",
        })
"#,
    );

    // `cook producer` pulls consumer into the closure via the (backwards) dep.
    let (ok, combined) = run_cook(tmp.path(), "producer");
    assert_diagnosed(ok, &combined);
}

// ---------------------------------------------------------------------------
// MUST NOT FIRE — the three legitimate ordering mechanisms
// ---------------------------------------------------------------------------

/// A recipe-header dep list (`recipe consumer : producer`) orders producer
/// first. §16.1.2 MUST NOT fire.
#[test]
fn dep_list_edge_does_not_fire() {
    let tmp = setup(
        r#"recipe producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "mkdir -p build && cp src.c build/gen.a",
        })

recipe consumer : producer
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "consumer");
    assert!(ok, "a dep-list edge orders producer first — MUST NOT fire:\n{combined}");
    assert!(
        tmp.path().join("out.bin").exists(),
        "the build MUST actually run:\n{combined}"
    );
}

/// A TRANSITIVE dep-list path (`consumer : middle`, `middle : producer`) also
/// orders producer first — the predicate is reachability, not a direct edge.
///
/// `middle` carries a unit deliberately. A ZERO-unit `middle` does not order
/// producer before consumer at all: `build_dag` records an empty leaf set for
/// a unit-less recipe, so the downstream recipe gets no edge and the chain
/// through the meta-target is silently severed (reproduced on this fixture at
/// HEAD, independent of this rule — reported separately). §16.1.2 reads the
/// DECLARED recipe graph, where the path `consumer → middle → producer`
/// plainly exists, so the rule correctly stays silent either way; giving
/// `middle` a unit keeps this test about THIS rule rather than entangling it
/// with that bug.
#[test]
fn transitive_dep_list_edge_does_not_fire() {
    let tmp = setup(
        r#"recipe producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "mkdir -p build && cp src.c build/gen.a",
        })

recipe middle : producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "mid.txt" },
            command = "cp src.c mid.txt",
        })

recipe consumer : middle
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "consumer");
    assert!(
        ok,
        "a transitive dep path orders producer first — MUST NOT fire:\n{combined}"
    );
}

/// The `$<producer>` sigil is a name reference and thus a §10.6 edge. It
/// orders producer first, so §16.1.2 MUST NOT fire even though the consumer
/// ALSO names the literal path in `inputs[]`.
#[test]
fn sigil_edge_does_not_fire() {
    let tmp = setup(
        r#"recipe producer
    ingredients "src.c"
    cook "build/gen.a" {
        mkdir -p build
        cp $<in> $<out>
    }

recipe consumer
    cook "out.bin" {
        cp $<producer> $<out>
    }
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "consumer");
    assert!(ok, "a $<sigil> edge orders producer first — MUST NOT fire:\n{combined}");
    assert!(
        tmp.path().join("out.bin").exists(),
        "the build MUST actually run:\n{combined}"
    );
}

/// `cook.require_recipe("producer")` (§22.8) is a name reference too. MUST NOT
/// fire.
#[test]
fn require_recipe_edge_does_not_fire() {
    let tmp = setup(
        r#"recipe producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "mkdir -p build && cp src.c build/gen.a",
        })

recipe consumer
        cook.require_recipe("producer")
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "consumer");
    assert!(
        ok,
        "a cook.require_recipe edge orders producer first — MUST NOT fire:\n{combined}"
    );
    assert!(
        tmp.path().join("out.bin").exists(),
        "the build MUST actually run:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// MUST NOT FIRE — closure scoping (THE TRIPWIRE SCENARIO)
// ---------------------------------------------------------------------------

/// **The scoping proof.** Same fixture as the core case, but invoked as
/// `cook consumer`: the producer is NOT in the closure, so §16.1.2 has nothing
/// to compare against and MUST stay silent.
///
/// This is not a concession — it is the rule. A literal output is not
/// build-owned, so `cook producer && cook consumer` is a legitimate workflow
/// that MUST NOT be rejected. (This is exactly where §16.1.2 diverges from
/// §22.1.2's glob rule, whose terminality is an OWNERSHIP claim and therefore
/// holds workspace-wide.)
///
/// This test is the in-crate mirror of
/// `raw_path_cross_recipe_edge.rs::raw_path_input_does_not_order_producer_before_consumer`,
/// which asserts `!combined.contains("producer")`. A workspace-wide check
/// would name the producer there and turn that tripwire red.
#[test]
fn producer_outside_closure_does_not_fire() {
    let tmp = setup(
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
    );

    let (ok, combined) = run_cook(tmp.path(), "consumer");

    // The run still fails — but on the honest `cp: cannot stat`, at execution
    // time, NOT on a §16.1.2 plan-time rejection.
    assert!(!ok, "consumer still fails: nothing schedules producer:\n{combined}");
    assert!(
        !combined.contains("producer"),
        "§16.1.2 is CLOSURE-scoped: with only `consumer` in the closure the \
         producer MUST NOT be named. If it is, the check leaked out of \
         closure scope and the raw_path_cross_recipe_edge.rs tripwire is \
         red:\n{combined}"
    );
}

/// A recipe reading its OWN literal output is not a cross-recipe race and MUST
/// NOT fire (a recipe's units are already ordered within the recipe).
#[test]
fn same_recipe_output_then_input_does_not_fire() {
    let tmp = setup(
        r#"recipe solo
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "mkdir -p build && cp src.c build/gen.a",
        })
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "solo");
    assert!(ok, "a recipe reading its own output MUST NOT fire:\n{combined}");
}

// ---------------------------------------------------------------------------
// The Standard's worked example, pinned as executable truth
// ---------------------------------------------------------------------------

/// **Example 16.1.2, verbatim.** A normative section's example must be
/// executable truth, not illustration — so the Standard's example is pinned
/// here rather than paraphrased. Keep this fixture byte-for-byte in step with
/// the `cook` block in §16.1.2; if you change one, change the other.
///
/// The example as first drafted was NOT diagnosed: its consumer read
/// `build/gen.a` only from shell-body text (`cook "out.bin" { cp build/gen.a
/// $<out> }`), which is not a declared input surface, so the rule's own
/// precondition — "`B` declares that same canonical path as a literal input
/// entry" — never held. It executed and died on `cp: cannot stat`, the exact
/// pre-existing failure §16.1.2 exists to replace, while the section claimed
/// "Building `all` MUST be rejected at plan time". This test exists so that
/// cannot silently recur.
///
/// Note the artifact is ABSENT: this is a cold build closure. That is the
/// point — detection reads source-declared paths, never filesystem state
/// (Note 16.1.1), and the cold build is the case that actually races.
#[test]
fn standard_example_16_1_2_is_diagnosed_verbatim() {
    let tmp = setup(
        r#"recipe producer
    cook "build/gen.a" { mkdir -p build && printf 'a' > $<out> }

recipe consumer
    cook.add_unit({
        inputs  = { "build/gen.a" },
        outputs = { "out.bin" },
        command = "cp build/gen.a out.bin",
    })

recipe all: producer consumer
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "all");
    assert_diagnosed(ok, &combined);
    assert!(
        !tmp.path().join("out.bin").exists(),
        "the example MUST be rejected, not reordered:\n{combined}"
    );
}

/// Example 16.1.2's reverse-dep paragraph, pinned: `recipe producer: consumer`
/// MUST still be rejected. The path exists but runs the wrong way.
#[test]
fn standard_example_16_1_2_reverse_dep_still_diagnosed() {
    let tmp = setup(
        r#"recipe producer: consumer
    cook "build/gen.a" { mkdir -p build && printf 'a' > $<out> }

recipe consumer
    cook.add_unit({
        inputs  = { "build/gen.a" },
        outputs = { "out.bin" },
        command = "cp build/gen.a out.bin",
    })

recipe all: producer consumer
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "all");
    assert_diagnosed(ok, &combined);
}

/// Example 16.1.2's repair paragraph, pinned: naming the producer from the
/// consumer fixes it, and the build then actually produces the artifact.
#[test]
fn standard_example_16_1_2_repaired_by_naming_producer() {
    let tmp = setup(
        r#"recipe producer
    cook "build/gen.a" { mkdir -p build && printf 'a' > $<out> }

recipe consumer: producer
    cook.add_unit({
        inputs  = { "build/gen.a" },
        outputs = { "out.bin" },
        command = "cp build/gen.a out.bin",
    })

recipe all: producer consumer
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "all");
    assert!(ok, "naming the producer MUST repair the example:\n{combined}");
    assert_eq!(
        fs::read_to_string(tmp.path().join("out.bin")).unwrap(),
        "a",
        "the repaired build MUST order the write before the read:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// The narrowed scope — `ingredients` literals are NOT covered (Note 16.1.2.2)
// ---------------------------------------------------------------------------

/// **§16.1.2 does not cover `ingredients`-sourced literals, and this pins that
/// exclusion as deliberate.**
///
/// An `ingredients` pattern is a filesystem glob resolved against disk at
/// register time (§21.2.1); a literal is just a glob with no metacharacters.
/// On a COLD build `build/gen.a` does not exist, so the pattern matches ZERO
/// files, contributes no input entry, and there is nothing for the rule to
/// compare — it stays silent and the build fails at runtime instead.
///
/// This test asserts that silence on purpose. Detection here would be
/// INVERTED: quiet on the cold build where the race lives, loud only once a
/// stale artifact already sat on disk (see the sibling test below, which pins
/// the other half of the A/B). A check whose sensitivity depends on the
/// filesystem state it is meant to protect is worse than no check, and it
/// would contradict Note 16.1.1's "source, not filesystem state" principle.
///
/// If you make this fire, you have changed §16.1.2's scope — Note 16.1.2.2 and
/// CS-0144 must change with it. §10.6's PROHIBITION is untouched either way:
/// no edge is inferred from this path match, which is what the
/// `raw_path_cross_recipe_edge.rs` tripwire guards.
#[test]
fn ingredients_sourced_literal_is_not_covered_on_cold_build() {
    let tmp = setup(
        r#"recipe producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "mkdir -p build && cp src.c build/gen.a",
        })

recipe consumer
    ingredients "build/gen.a"
    cook "out.bin" { cp $<in> $<out> }

recipe all : producer consumer
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "all");
    assert!(!ok, "the cold build still fails on its own terms:\n{combined}");
    assert!(
        !combined.contains("read-after-write with no ordering edge"),
        "§16.1.2 MUST NOT claim to cover an `ingredients`-sourced literal: on \
         a cold build the glob matches 0 files and no input entry exists. If \
         this fires, the rule's scope changed and Note 16.1.2.2 / CS-0144 are \
         now wrong:\n{combined}"
    );
}
