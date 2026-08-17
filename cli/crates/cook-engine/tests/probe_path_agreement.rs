//! COOK-359: the two probe-evaluation paths must agree.
//!
//! Probe evaluation is implemented twice — `cook-register`'s
//! `evaluate_prepass_probe` (which feeds an `gather <probe>` fan-out) and
//! `cook-engine`'s executor G4 path (which feeds sealed consumers). Both do the
//! same thing around a different Lua VM: resolve inputs, compute a fingerprint,
//! decide whether a value is already resolved without a VM, run the producer
//! otherwise, write `.cook/probes/<key>.json`, decode. Only the VM genuinely
//! differs — and, since CS-0243 (COOK-527), there is no cache GET or PUT left
//! in either copy to have drifted.
//!
//! §22.5.8's **Always observe** rule (CS-0243) names no consumption path: a
//! reached probe's `produce` body MUST run, once, on every invocation,
//! regardless of how its value is CONSUMED. That is still not part of the
//! probe's identity, so the same probe declaration must cost the same number
//! of producer executions either way — now trivially "every invocation" for
//! BOTH a keyed and a keyless probe, where before CS-0243 keyedness was what
//! made the two counts diverge (`keyed_probe_costs_the_same_on_both_paths`
//! below inverted from "1 run in 3" to "3 runs in 3" for exactly that
//! reason). The file's original point — one path re-producing on every
//! invocation while the other one cached is a defect, not a feature some
//! consumer can rely on — survives the flip: agreement between the paths is
//! still the whole of what this file checks.
//!
//! `keyless_probe_reproduces_on_both_paths` no longer distinguishes anything
//! CS-0178 governs (every probe re-produces now, keyed or not) but is kept:
//! it is still evidence that neither path regressed into caching anything.

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
/// `keyed:items` declares a file input; `keyless:items` declares nothing.
/// Before CS-0243 that distinction was everything — §22.5.8 gave the keyed
/// probe a cache key so one producer execution served every later run while
/// `dep.txt` stayed unchanged, and CS-0178 gave the keyless one no key so it
/// re-produced every run. CS-0243 removed the cache the distinction was
/// FOR: a reached probe re-produces every invocation regardless, keyed or
/// not, so both probes now cost the same either way. `keyed:items` and its
/// `seal "src/dep.txt"` are kept anyway — the fixture still proves a keyed
/// probe's declared input plays no role in whether it re-produces, which is
/// the CS-0243 rule stated the other way round. Each producer appends one
/// line to its own runlog, which makes execution observable independently
/// of the value.
///
/// Both values are deterministic: the runlogs measure how often the producer
/// RAN, never what it returned, so a divergence cannot hide behind a value that
/// happens to change anyway. The runlogs sit outside `out/` so the orphaned-
/// output sweeper leaves them alone.
const COOKFILE: &str = r#"probe keyed:items
    seal "src/dep.txt"
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
    gather keyed:items
    cook "out/fk-$<in>.txt" { echo '$<in>' > $<out> }

recipe sealed_keyless
    seal keyless:items
    cook "out/sealed_keyless.txt" { echo built > $<out> }

recipe fanned_keyless
    gather keyless:items
    cook "out/fl-$<in>.txt" { echo '$<in>' > $<out> }
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
    Arm {
        _tmp: tmp,
        _cache: cache,
        wd,
    }
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

    // CS-0243 (COOK-527): a reached probe always observes, so a keyed
    // probe's declared inputs holding still buys it nothing any more — the
    // producer runs once per invocation, every invocation, on both paths.
    assert_eq!(
        sealed_runs, RUNS,
        "seal path: keyed probe producer ran {sealed_runs}x in {RUNS} runs, \
         expected {RUNS} (CS-0243)",
    );
    assert_eq!(
        fanned_runs, RUNS,
        "gather <probe> path: keyed probe producer ran {fanned_runs}x in \
         {RUNS} runs, expected {RUNS} (CS-0243)",
    );
    assert_eq!(
        sealed_runs, fanned_runs,
        "the same probe declaration cost a different number of producer \
         executions depending only on how its value was consumed",
    );

    // The ticket's second acceptance criterion: byte-identical values. Still
    // meaningful post-CS-0243 — the producer is deterministic, so both paths
    // must still agree on the canonical bytes even though each ran RUNS
    // times rather than once.
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
        "gather <probe> path: keyless probe producer ran {fanned_runs}x in \
         {RUNS} runs, expected {RUNS} (CS-0178)",
    );
}
