//! COOK-359: the two probe-evaluation paths must agree.
//!
//! Probe evaluation is implemented twice — `cook-register`'s
//! `evaluate_prepass_probe` (which feeds an `ingredients <probe>` fan-out) and
//! `cook-engine`'s executor G4 path (which feeds sealed consumers). Both do the
//! same thing around a different Lua VM: resolve inputs, compute a fingerprint,
//! cache GET, run the producer on a miss, cache PUT, write
//! `.cook/probes/<key>.json`, decode. Only the VM genuinely differs.
//!
//! §22.5.8's cache-hit clause names no consumption path: when a probe's
//! fingerprint addresses a stored artifact the implementation MUST load the
//! cached bytes and skip execution. How a probe's value is CONSUMED is not
//! part of its identity, so the same probe declaration must cost the same
//! number of producer executions either way.
//!
//! This file pins that as an executable claim. It fails before the paths are
//! unified: the register copy's cache block sits under
//! `match cache_ctx { Some(ctx) => .., None => run_produce(..) }` and
//! `cook-cli/src/pipeline.rs` hardcodes `None` into the function its own doc
//! comment calls "the sole registration path for every command", so the block
//! has never once run against a backend. The fan-out path therefore re-produces
//! on every invocation regardless of what the probe declares.
//!
//! `keyless_probe_reproduces_on_both_paths` is the guard on the fix: CS-0178
//! keylessness lives only in the executor copy, so switching the pre-pass cache
//! on without porting it first would reintroduce COOK-343 on the fan-out path —
//! and there the permanent hit lives in the SHARED store, which `rm -rf .cook`
//! does not reach. That test passes today and must still pass afterwards.

use std::fs;
use std::path::Path;
use std::process::Command;

fn cook_binary() -> std::path::PathBuf {
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

/// Two probes, two consumption paths each.
///
/// `keyed:items` declares a file input, so §22.5.8 gives it a cache key and one
/// producer execution must serve every later run while `dep.txt` is unchanged.
/// `keyless:items` declares nothing, so CS-0178 gives it NO key and it must
/// re-produce every run. Each producer appends one line to its own runlog,
/// which makes execution observable independently of the value.
///
/// Both values are deterministic: the runlogs measure how often the producer
/// RAN, never what it returned, so a divergence cannot hide behind a value that
/// happens to change anyway. The runlogs sit outside `out/` so the orphaned-
/// output sweeper leaves them alone.
const COOKFILE: &str = r#"probe keyed:items
    ingredients "src/dep.txt"
    json {
        echo ran >> keyed.runlog
        printf '["a"]\n'
    }

probe keyless:items
    json {
        echo ran >> keyless.runlog
        printf '["a"]\n'
    }

recipe sealed_keyed
    seal keyed:items
    cook "out/sealed_keyed.txt" { echo built > $<out> }

recipe fanned_keyed
    ingredients keyed:items
    cook "out/fk-$<in>.txt" { echo "$<in>" > $<out> }

recipe sealed_keyless
    seal keyless:items
    cook "out/sealed_keyless.txt" { echo built > $<out> }

recipe fanned_keyless
    ingredients keyless:items
    cook "out/fl-$<in>.txt" { echo "$<in>" > $<out> }
"#;

/// A workspace with its own shared store, so one arm can never warm another's
/// cache and the run count measures exactly one path.
struct Arm {
    _tmp: tempfile::TempDir,
    _cache: tempfile::TempDir,
    wd: std::path::PathBuf,
}

fn arm() -> Arm {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path().to_path_buf();
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache.path().to_string_lossy()),
    )
    .unwrap();
    fs::create_dir_all(wd.join("src")).unwrap();
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(wd.join("src/dep.txt"), "dep-content\n").unwrap();
    fs::write(wd.join("Cookfile"), COOKFILE).unwrap();
    Arm { _tmp: tmp, _cache: cache, wd }
}

fn build(wd: &Path, recipe: &str) {
    let out = Command::new(cook_binary())
        .arg(recipe)
        .current_dir(wd)
        .output()
        .expect("cook invocation");
    assert!(
        out.status.success(),
        "cook {recipe} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

fn producer_runs(wd: &Path, runlog: &str) -> usize {
    fs::read_to_string(wd.join(runlog))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn probe_value_bytes(wd: &Path, key: &str) -> Vec<u8> {
    let path = wd.join(".cook").join("probes").join(format!("{key}.json"));
    fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Run one recipe `n` times in a fresh workspace and report how often the
/// named probe producer executed, plus the canonical value it left behind.
fn measure(recipe: &str, runlog: &str, key: &str, n: usize) -> (usize, Vec<u8>) {
    let a = arm();
    for _ in 0..n {
        build(&a.wd, recipe);
    }
    (producer_runs(&a.wd, runlog), probe_value_bytes(&a.wd, key))
}

const RUNS: usize = 3;

#[test]
fn keyed_probe_costs_the_same_on_both_paths() {
    let (sealed_runs, sealed_value) =
        measure("sealed_keyed", "keyed.runlog", "keyed:items", RUNS);
    let (fanned_runs, fanned_value) =
        measure("fanned_keyed", "keyed.runlog", "keyed:items", RUNS);

    // §22.5.8: a keyed probe whose declared inputs have not moved is served
    // from cache, so the producer runs exactly once across the whole series.
    assert_eq!(
        sealed_runs, 1,
        "seal path: keyed probe producer ran {sealed_runs}x in {RUNS} runs, expected 1",
    );
    assert_eq!(
        fanned_runs, 1,
        "ingredients <probe> path: keyed probe producer ran {fanned_runs}x in \
         {RUNS} runs, expected 1 — the register pre-pass is not consulting the \
         cache (COOK-359)",
    );
    assert_eq!(
        sealed_runs, fanned_runs,
        "the same probe declaration cost a different number of producer \
         executions depending only on how its value was consumed",
    );

    // The ticket's second acceptance criterion: byte-identical values.
    assert_eq!(
        sealed_value, fanned_value,
        "the two paths wrote different bytes to .cook/probes/keyed:items.json",
    );
}

#[test]
fn keyless_probe_reproduces_on_both_paths() {
    let (sealed_runs, _) =
        measure("sealed_keyless", "keyless.runlog", "keyless:items", RUNS);
    let (fanned_runs, _) =
        measure("fanned_keyless", "keyless.runlog", "keyless:items", RUNS);

    // CS-0178: a probe declaring no inputs has no cache key and MUST re-produce
    // on every invocation in which it is reached. Turning the pre-pass cache on
    // without porting this rule would make a keyless driver a permanent hit in
    // the shared store (COOK-343's failure mode, one path over).
    assert_eq!(
        sealed_runs, RUNS,
        "seal path: keyless probe producer ran {sealed_runs}x in {RUNS} runs, \
         expected {RUNS} (CS-0178)",
    );
    assert_eq!(
        fanned_runs, RUNS,
        "ingredients <probe> path: keyless probe producer ran {fanned_runs}x in \
         {RUNS} runs, expected {RUNS} (CS-0178)",
    );
}
