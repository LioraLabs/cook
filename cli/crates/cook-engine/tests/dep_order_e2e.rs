//! COOK-297 — `cook.dep_order(name)` and fine-covered barrier suppression,
//! end-to-end through the real register → build_dag → executor path.
//!
//! `cook.dep_order` is the ordering-only counterpart of `cook.dep_output`:
//! it records a fine-grained dep ref for the units added after it (an
//! execution edge to the named recipe's leaves) WITHOUT folding the
//! producer's outputs into the unit's cache inputs. A recipe that expresses
//! its consumption of a required recipe through this fine-grained channel is
//! "fine-covered" for that dep: its ROOT units stop inheriting the coarse
//! root→leaf-barrier edge, so non-consuming units (compiles) run in one flat
//! wave across the recipe chain.
//!
//! The DAG-shape halves of this contract are pinned at the unit level in
//! `dag_builder.rs` (`dep_edge_to_required_recipe_suppresses_coarse_root_barrier`
//! and siblings); these tests pin the Lua-visible surface and the executor
//! behavior.

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

/// The ordering half: a unit registered after `cook.dep_order("producer")`
/// must wait for the producer's leaves, so its read of the producer's output
/// succeeds deterministically. Also pins that the require_recipe +
/// literal-input combination stays quiet under §16.1.2 when the consumption
/// is expressed through dep_order (the declared `requires` path exists).
#[test]
fn dep_order_unit_waits_for_producer_leaves() {
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
            inputs  = { "src.c" },
            outputs = { "first.txt" },
            command = "cp src.c first.txt",
        })
        cook.dep_order("producer")
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "consumer");
    assert!(ok, "the dep_order edge must order the read after the write:\n{combined}");
    assert_eq!(
        fs::read_to_string(tmp.path().join("out.bin")).unwrap(),
        "seed-content\n",
        "the consuming unit must have read the produced artifact:\n{combined}"
    );
}

/// The flat-wave half. `cook.dep_order` is the fine-grained REPLACEMENT for
/// `cook.require_recipe`, so the consumer declares no coarse dep at all: the
/// dep_order call forces `producer`'s body, pulls it into the build closure,
/// and orders ONLY the unit registered after it. The consumer's first unit is
/// registered BEFORE the call, so it carries no edge and must start
/// immediately. The producer sleeps ~1.5s before writing, so mtime comparison
/// leaves a wide margin against scheduler jitter on a loaded test box.
///
/// Note there is no `require_recipe` here and no dep-list entry — `producer`
/// reaches the closure solely by being NAMED (§22.10, "Closure membership is
/// established"; CS-0121 for the general rule).
#[test]
fn non_consuming_unit_runs_in_parallel_with_producer() {
    let tmp = setup(
        r#"recipe producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "sleep 1.5 && mkdir -p build && cp src.c build/gen.a",
        })

recipe consumer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "first.txt" },
            command = "cp src.c first.txt",
        })
        cook.dep_order("producer")
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "consumer");
    assert!(ok, "build must succeed:\n{combined}");

    let first = fs::metadata(tmp.path().join("first.txt"))
        .expect("first.txt written")
        .modified()
        .unwrap();
    let gen = fs::metadata(tmp.path().join("build/gen.a"))
        .expect("gen.a written")
        .modified()
        .unwrap();
    assert!(
        first < gen,
        "the unit registered before the dep_order call carries no edge and \
         must not wait for the producer; first.txt was written at {first:?}, \
         gen.a at {gen:?}:\n{combined}"
    );
    assert!(
        tmp.path().join("out.bin").exists(),
        "the unit registered after the dep_order call must still run, ordered \
         after the producer:\n{combined}"
    );
}

/// The additive guarantee (CS-0161, "Rejected alternative"). A recipe that
/// DECLARES `cook.require_recipe` keeps byte-identical whole-recipe ordering
/// whether or not its units also carry fine-grained refs. `cook.dep_order`
/// adds a per-unit edge; it never subtracts a coarse one. This is the exact
/// case the withdrawn fine-covered narrowing rule would have suppressed, so it
/// is the regression test for that withdrawal.
#[test]
fn dep_order_does_not_suppress_a_declared_require_recipe_barrier() {
    let tmp = setup(
        r#"recipe producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "sleep 1.5 && mkdir -p build && cp src.c build/gen.a",
        })

recipe consumer
        cook.require_recipe("producer")
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "first.txt" },
            command = "cp src.c first.txt",
        })
        cook.dep_order("producer")
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "consumer");
    assert!(ok, "build must succeed:\n{combined}");

    let first = fs::metadata(tmp.path().join("first.txt"))
        .expect("first.txt written")
        .modified()
        .unwrap();
    let gen = fs::metadata(tmp.path().join("build/gen.a"))
        .expect("gen.a written")
        .modified()
        .unwrap();
    assert!(
        first > gen,
        "the declared require_recipe barrier MUST still hold: every root unit \
         of the consumer waits for the producer's leaves, dep_order or not. \
         first.txt was written at {first:?}, gen.a at {gen:?}:\n{combined}"
    );
}

/// Without any fine-grained ref, the coarse barrier must SURVIVE — the
/// conservative default that keeps `require_recipe` + literal-input recipes
/// ordered (the §16.1.2 suppression contract). Mirror of the flat-wave test
/// with the dep_order call removed: now even the non-consuming first unit
/// waits for the producer.
#[test]
fn without_dep_order_coarse_barrier_still_orders_root_units() {
    let tmp = setup(
        r#"recipe producer
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/gen.a" },
            command = "sleep 1.5 && mkdir -p build && cp src.c build/gen.a",
        })

recipe consumer
        cook.require_recipe("producer")
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "first.txt" },
            command = "cp src.c first.txt",
        })
        cook.add_unit({
            inputs  = { "build/gen.a" },
            outputs = { "out.bin" },
            command = "cp build/gen.a out.bin",
        })
"#,
    );

    let (ok, combined) = run_cook(tmp.path(), "consumer");
    assert!(ok, "build must succeed:\n{combined}");

    let first = fs::metadata(tmp.path().join("first.txt"))
        .expect("first.txt written")
        .modified()
        .unwrap();
    let gen = fs::metadata(tmp.path().join("build/gen.a"))
        .expect("gen.a written")
        .modified()
        .unwrap();
    assert!(
        first > gen,
        "without a fine-grained ref the consumer's root unit must wait on \
         the producer's leaf barrier; first.txt at {first:?}, gen.a at \
         {gen:?}:\n{combined}"
    );
}
