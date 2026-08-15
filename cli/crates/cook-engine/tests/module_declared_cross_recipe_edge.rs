//! End-to-end proof that `cook.require_recipe` (Standard §22.8) orders the
//! build and joins the closure through the real `cook` binary.
//!
//! Tasks 1-2 landed `cook.require_recipe(name)` and proved, at the register
//! layer, that the edge it establishes lands in the calling recipe's
//! `requires` metadata — the exact field a surface dep-list (`recipe A : B`)
//! lowers to. Downstream, `cook-engine/src/run.rs` overwrites
//! `RecipeUnits.deps` from the analyzer's edges map, built from
//! `RegisteredRecipePub.requires`. So a `cook.require_recipe` edge should be
//! indistinguishable from a dep-list edge for closure computation and
//! ordering. This file proves that through the real binary rather than at
//! the register layer alone.
//!
//! Harness shape (`cook_binary`, `isolate_cache`, `run_cook`, counting
//! `queued` lines rather than trusting `(N nodes)`) is deliberately copied
//! from `raw_path_cross_recipe_edge.rs` — see that file's header for why
//! `(N nodes)` on a `queued` line is a per-recipe UNIT count, not closure
//! size, and does not discriminate a 1-recipe from a 2-recipe closure.
//!
//! Contrast with `raw_path_cross_recipe_edge.rs`: that file's first test
//! pins a GAP where a raw path-string match does NOT create an edge (per
//! Standard §10.6 / App. C.17.1 — "Only name references create edges").
//! `cook.require_recipe` takes a **name**, never a path (§22.8, "This is an
//! instance of the name-reference rule, not an exception to it"), so it
//! supplies exactly the evidence that section demands. The two files are
//! deliberately symmetric: same producer/consumer shape, opposite outcome,
//! because the linking mechanism differs.

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
// Test 1 — the edge orders the build and joins the closure
// ---------------------------------------------------------------------------

/// `producer` is a plain `cook.add_unit` recipe with no dep-list header and
/// no `$<sigil>` reference anywhere. `consumer` links to it ONLY via
/// `cook.require_recipe("producer")` inside its body, followed by a raw-path
/// `cook.add_unit` whose `inputs` is the literal string `"build/gen.a"` —
/// the exact fixture shape `raw_path_cross_recipe_edge.rs`'s GAP test uses,
/// which on its own (no `require_recipe` call) fails with exactly ONE
/// `queued` line and the producer never running. Here, the ONLY difference
/// is the `require_recipe` call, and it MUST flip the outcome: two recipes
/// queued, producer's unit completing before consumer's, and consumer's
/// output carrying producer's bytes.
#[test]
fn module_declared_edge_orders_producer_before_consumer_and_joins_closure() {
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
        cook.require_recipe("producer")
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    )
    .unwrap();
    fs::write(wd.join("src.c"), b"MODULE-DECLARED-BYTES-v1\n").unwrap();

    let (ok, combined) = run_cook(wd, "consumer");

    assert!(
        ok,
        "cook.require_recipe MUST schedule producer then consumer:\n{combined}"
    );

    // Both recipes are queued — the producer joined the closure of `cook
    // consumer` even though nothing but the require_recipe call names it.
    assert_eq!(
        combined.matches("queued").count(),
        2,
        "both producer and consumer MUST be queued — the require_recipe edge \
         MUST pull producer into the closure:\n{combined}"
    );
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
        "MODULE-DECLARED-BYTES-v1",
        "consumer's output MUST carry the producer's bytes:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — observably identical to a dep-list edge
// ---------------------------------------------------------------------------

/// The same producer/consumer pair, linked instead by a plain dep-list
/// header (`recipe consumer : producer`) and no `require_recipe` call at
/// all. This is the control: it MUST exhibit the exact same observable
/// shape (2 queued, 2-node closure, producer-before-consumer, bytes
/// carried through) as test 1 — proving the `require_recipe` edge is
/// indistinguishable from a dep-list edge downstream, per §22.8 ("identical
/// in kind and granularity to a dep-list entry").
#[test]
fn module_declared_edge_matches_dep_list_edge_ordering() {
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

recipe consumer : producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    )
    .unwrap();
    fs::write(wd.join("src.c"), b"DEP-LIST-BYTES-v1\n").unwrap();

    let (ok, combined) = run_cook(wd, "consumer");

    assert!(ok, "dep-list edge MUST schedule producer then consumer:\n{combined}");
    assert_eq!(
        combined.matches("queued").count(),
        2,
        "control: both recipes MUST be queued:\n{combined}"
    );
    assert!(
        combined.contains("2 nodes"),
        "control: closure MUST hold both producer and consumer:\n{combined}"
    );

    let producer_at = combined
        .find("producer/build/gen.a")
        .unwrap_or_else(|| panic!("producer unit never ran:\n{combined}"));
    let consumer_at = combined
        .find("consumer/out.bin")
        .unwrap_or_else(|| panic!("consumer unit never ran:\n{combined}"));
    assert!(
        producer_at < consumer_at,
        "control: producer MUST complete before consumer:\n{combined}"
    );

    let out = fs::read_to_string(wd.join("out.bin")).expect("out.bin must exist");
    assert_eq!(
        out.trim(),
        "DEP-LIST-BYTES-v1",
        "control: consumer's output MUST carry the producer's bytes:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — producer declared AFTER consumer in the file
// ---------------------------------------------------------------------------

/// `consumer` appears first in the file and calls
/// `cook.require_recipe("producer")`, whose body is declared further down.
/// Per §22.8's register-order guarantee, the call forces `producer`'s body
/// to completion synchronously at the moment it is called — it does not
/// depend on file order or a pre-pass sort — so this MUST succeed exactly
/// like test 1.
#[test]
fn module_declared_edge_works_when_producer_declared_after_consumer() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    isolate_cache(wd);

    fs::write(
        wd.join("Cookfile"),
        r#"recipe consumer
        cook.require_recipe("producer")
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })

recipe producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "mkdir -p build && cp src.c build/gen.a",
        })
"#,
    )
    .unwrap();
    fs::write(wd.join("src.c"), b"DECLARED-AFTER-BYTES-v1\n").unwrap();

    let (ok, combined) = run_cook(wd, "consumer");

    assert!(
        ok,
        "require_recipe MUST work regardless of declaration order:\n{combined}"
    );
    assert_eq!(
        combined.matches("queued").count(),
        2,
        "both recipes MUST be queued even though producer is declared after \
         consumer in the file:\n{combined}"
    );
    assert!(
        combined.contains("2 nodes"),
        "closure MUST hold both producer and consumer:\n{combined}"
    );

    let producer_at = combined
        .find("producer/build/gen.a")
        .unwrap_or_else(|| panic!("producer unit never ran:\n{combined}"));
    let consumer_at = combined
        .find("consumer/out.bin")
        .unwrap_or_else(|| panic!("consumer unit never ran:\n{combined}"));
    assert!(
        producer_at < consumer_at,
        "producer MUST complete before consumer even when declared later in \
         the file:\n{combined}"
    );

    let out = fs::read_to_string(wd.join("out.bin")).expect("out.bin must exist");
    assert_eq!(
        out.trim(),
        "DECLARED-AFTER-BYTES-v1",
        "consumer's output MUST carry the producer's bytes:\n{combined}"
    );
}
