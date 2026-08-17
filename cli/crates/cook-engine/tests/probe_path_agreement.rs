//! COOK-359: the two probe-evaluation paths must agree.
//!
//! Probe evaluation is driven from two places — `cook-register`'s pre-pass
//! (which feeds a `gather <probe>` fan-out) and `cook-engine`'s executor
//! dispatch (which feeds sealed consumers). Both run the same
//! `cook_probe::eval` sequence around a different Lua VM: resolve the declared
//! `tools`/`files` sets, take a value a synthesised producer kind determines
//! without a VM, run the producer otherwise, write `.cook/probes/<key>.json`,
//! decode. Only the VM genuinely differs.
//!
//! §22.5.8's **Always observe** rule (CS-0243) names no consumption path: a
//! reached probe's `produce` body MUST run, once, on every invocation,
//! regardless of how its value is CONSUMED. So the same probe declaration must
//! cost the same number of producer executions either way. The file's original
//! point — one path re-producing on every invocation while the other one
//! cached is a defect, not a feature some consumer can rely on — survives the
//! removal of the cache: agreement between the paths is still the whole of
//! what this file checks.
//!
//! The two tests below are near-duplicates and both earn their place. The
//! first additionally pins that the two paths write byte-identical values. The
//! second covers a probe that declares NOTHING. That is not a separate code
//! path — the resolver iterates the same empty vec zero times — but it is the
//! case the retired CS-0178 keylessness rule used to single out, and the case
//! an implementation could most cheaply turn back into a permanent hit. It is
//! kept as a cheap regression guard on exactly that, not because the route
//! through the resolver differs.

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
/// `declaring:items` declares a file input; `bare:items` declares nothing.
/// The declaration plays no role in whether a probe re-produces — that is
/// CS-0243's rule stated the other way round — and both are here because the
/// bare case is the one CS-0178 used to treat specially, not because it takes a
/// different route through the resolver. Each producer
/// appends one line to its own runlog, which makes execution observable
/// independently of the value.
///
/// Both values are deterministic: the runlogs measure how often the producer
/// RAN, never what it returned, so a divergence cannot hide behind a value that
/// happens to change anyway. The runlogs sit outside `out/` so the orphaned-
/// output sweeper leaves them alone.
const COOKFILE: &str = r#"probe declaring:items
    seal "src/dep.txt"
    json {
        echo ran >> declaring.runlog
        printf '["a"]\n'
    }

probe bare:items
    json {
        echo ran >> bare.runlog
        printf '["a"]\n'
    }

recipe sealed_declaring
    seal declaring:items
    cook "out/sealed_declaring.txt" { echo built > $<out> }

recipe fanned_declaring
    gather declaring:items
    cook "out/fk-$<in>.txt" { echo '$<in>' > $<out> }

recipe sealed_bare
    seal bare:items
    cook "out/sealed_bare.txt" { echo built > $<out> }

recipe fanned_bare
    gather bare:items
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
fn a_probe_declaring_an_input_costs_the_same_on_both_paths() {
    let (sealed_runs, sealed_value) =
        measure("sealed_declaring", "declaring.runlog", "declaring:items", RUNS);
    let (fanned_runs, fanned_value) =
        measure("fanned_declaring", "declaring.runlog", "declaring:items", RUNS);

    // CS-0243 (COOK-527): a reached probe always observes, so a declared
    // input holding still buys the probe nothing — the producer runs once per
    // invocation, every invocation, on both paths.
    assert_eq!(
        sealed_runs, RUNS,
        "seal path: producer ran {sealed_runs}x in {RUNS} runs, \
         expected {RUNS} (CS-0243)",
    );
    assert_eq!(
        fanned_runs, RUNS,
        "gather <probe> path: producer ran {fanned_runs}x in \
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
        "the two paths wrote different bytes to .cook/probes/declaring:items.json",
    );
}

/// The same agreement for a probe that declares NOTHING. It runs the same
/// resolver over an empty declaration rather than a different one, so this is a
/// cheap regression guard rather than separate coverage: a probe with no
/// declared inputs was the case CS-0178 used to single out before CS-0244
/// retired the distinction, and it is the case most cheaply mistaken for a
/// permanent hit. It still discriminates — it counts producer runs across both
/// consumption paths, and fails if either stops re-producing.
#[test]
fn a_probe_declaring_nothing_behaves_as_one_declaring_an_input() {
    let (sealed_runs, _) =
        measure("sealed_bare", "bare.runlog", "bare:items", RUNS);
    let (fanned_runs, _) =
        measure("fanned_bare", "bare.runlog", "bare:items", RUNS);

    // CS-0243: identical counts to the declaring probe above. A probe that
    // declares nothing was, before CS-0244, the one case an implementation
    // could most cheaply have turned into a permanent hit (COOK-343's failure
    // mode); it re-produces on both paths like every other.
    assert_eq!(
        sealed_runs, RUNS,
        "seal path: producer ran {sealed_runs}x in {RUNS} runs, \
         expected {RUNS} (CS-0243)",
    );
    assert_eq!(
        fanned_runs, RUNS,
        "gather <probe> path: producer ran {fanned_runs}x in \
         {RUNS} runs, expected {RUNS} (CS-0243)",
    );
}
