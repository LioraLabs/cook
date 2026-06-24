//! COOK-161 end-to-end: the cache-key bar for the single-key seal model.
//!
//! The single content-addressed cache key folds the effective seal set's probe
//! VALUES. The two properties this proves, against the real `cook` binary:
//!
//!   1. A **machine-independent** unit (no `seal`) HITS across a simulated host
//!      change — host identity is not in its key, so it shares fleet-wide.
//!   2. A `seal host` unit MISSES on the same host change — the sealed `host`
//!      probe's value changed and folded into the unit's key.
//!
//! The host signal is a `produce as env { SIMHOST }` probe (CS-0106): changing
//! `SIMHOST` re-runs the probe and changes its value. Each unit appends a line
//! to a per-unit runlog, so a re-run is observable as a line-count bump
//! independent of any human-readable build summary wording.

use std::fs;
use std::path::Path;
use std::process::Command;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // .../target/debug/deps  ->  .../target/debug
    path.pop();
    path.push("cook");
    assert!(
        path.exists(),
        "cook binary not found at {} — run `cargo build --bin cook` first",
        path.display()
    );
    path
}

/// Point the cache at a private dir so runs do not collide on artifact keys in
/// the system-wide local backend (`~/.cache/cook/cloud`).
fn write_fixture(wd: &Path, cache_dir: &Path) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
    fs::create_dir_all(wd.join("src")).unwrap();
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(wd.join("src/in.txt"), "src-content\n").unwrap();
    fs::write(
        wd.join("Cookfile"),
        r#"probe host
    produce as env { SIMHOST }

recipe shared
    ingredients "src/in.txt"
    cook "out/shared.txt" {
        cp src/in.txt out/shared.txt
        echo ran >> out/shared.runlog
    }

recipe hostdep
    ingredients "src/in.txt"
    seal host
    cook "out/host.txt" {
        printf 'built\n' > out/host.txt
        echo ran >> out/host.runlog
    }
"#,
    )
    .unwrap();
}

/// Build both recipes under a given `SIMHOST`. Each recipe is a separate
/// invocation because `cook` takes exactly one recipe target per run.
fn build(wd: &Path, simhost: &str) {
    for recipe in ["shared", "hostdep"] {
        let out = Command::new(cook_binary())
            .arg(recipe)
            .env("SIMHOST", simhost)
            .current_dir(wd)
            .output()
            .expect("cook invocation");
        assert!(
            out.status.success(),
            "cook {recipe} (SIMHOST={simhost}) failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
}

/// Number of times the unit behind `runlog` actually executed (a re-run appends
/// one line). A cache HIT leaves the count unchanged.
fn runs(wd: &Path, runlog: &str) -> usize {
    match fs::read_to_string(wd.join("out").join(runlog)) {
        Ok(s) => s.lines().count(),
        Err(_) => 0,
    }
}

#[test]
fn machine_independent_unit_hits_across_host_change_sealed_unit_misses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache tempdir");
    let wd = tmp.path();
    write_fixture(wd, cache.path());

    // Run 1 (cold, SIMHOST=alpha): both units execute fresh.
    build(wd, "alpha");
    assert_eq!(runs(wd, "shared.runlog"), 1, "run1: shared should build cold");
    assert_eq!(runs(wd, "host.runlog"), 1, "run1: hostdep should build cold");

    // Run 2 (warm, SIMHOST=alpha, nothing changed): BOTH hit — including the
    // sealed unit, because the host probe value is unchanged.
    build(wd, "alpha");
    assert_eq!(
        runs(wd, "shared.runlog"),
        1,
        "run2: shared must hit (no input/command change)"
    );
    assert_eq!(
        runs(wd, "host.runlog"),
        1,
        "run2: sealed hostdep must hit when the sealed host value is unchanged"
    );

    // Run 3 (SIMHOST=beta — a simulated host change): the host probe re-runs and
    // its value changes.
    //   - shared HITS: it declared no host determinant, so its key is unchanged.
    //   - hostdep MISSES: `seal host` folded the changed host value into its key.
    build(wd, "beta");
    assert_eq!(
        runs(wd, "shared.runlog"),
        1,
        "run3: machine-independent `shared` MUST hit across the host change — \
         host identity is not in its key"
    );
    assert_eq!(
        runs(wd, "host.runlog"),
        2,
        "run3: `seal host` hostdep MUST miss and rebuild — the sealed host \
         probe value changed and folds into the unit's single cache key"
    );

    // Run 4 (back to SIMHOST=alpha): the local cache index keeps only the most
    // recent StepEntry per cache_key, which now holds the *beta* seal value, so
    // the local check misses on seal_contribution. Under COOK-162 §3 sharing a
    // cold local miss on a NON-`local` unit consults the shared store by
    // recomputing the unit's one key from the *current* (alpha) seal value — and
    // the alpha-keyed artifact is still in the shared backend from runs 1/2. So
    // the unit is served by a cold fetch-by-key (host stays 2), NOT rebuilt.
    // This is the single-key sharing contract: the key is a pure function of the
    // sealed value, and any previously-published artifact for that key is reused
    // fleet-wide regardless of what the local index last persisted.
    build(wd, "alpha");
    assert_eq!(
        runs(wd, "host.runlog"),
        2,
        "run4: returning to the alpha host value HITS via COOK-162 cold \
         fetch-by-key — the alpha-keyed artifact is still in the shared store"
    );
    assert_eq!(runs(wd, "shared.runlog"), 1, "run4: shared still hits");

    // Run 5 (SIMHOST=alpha again, now warm on alpha): the entry persisted in run4
    // is the alpha-keyed one, so the sealed unit hits locally — host stays 2.
    build(wd, "alpha");
    assert_eq!(
        runs(wd, "host.runlog"),
        2,
        "run5: warm on alpha must hit — the alpha-keyed entry is the freshest \
         persisted one"
    );
}
