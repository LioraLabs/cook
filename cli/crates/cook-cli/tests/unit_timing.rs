//! End-to-end proof that a unit's completion line reports the *measured*
//! per-unit wall time instead of a hardcoded `0.00s`.
//!
//! Root cause (pre-fix): `EngineEvent::NodeCompleted.elapsed` was hardcoded
//! `Duration::ZERO` at the emission sites that follow a real `WorkResult`
//! from the worker pool, because `WorkResult` carried no duration field.
//! `TestOutput.duration` already measured this correctly for test units;
//! this proves the same measurement now reaches plain (non-test) cook
//! units' completion lines.
//!
//! Scenario: a single cook unit whose command sleeps ~1s. Its own
//! completion line (`<recipe>/<output path>    <T>s`) must show `T >= 0.9`
//! — not `0.00s` — while the recipe-total line (a separate, always-real
//! measurement) is unaffected. The plain (non-TTY) renderer writes these
//! lines to stderr (`cook-cli/src/progress.rs`: non-tty/CI ⇒ `PlainRenderer`
//! on stderr), so this test spawns the built binary with piped stderr and
//! regexes the transcript — the surface e2e harness has no regex/negation
//! support for this kind of numeric assertion.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn cook_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

/// One cook unit whose command sleeps ~1s before writing its declared
/// output. Long enough that a `0.00s` regression is unambiguous, short
/// enough to keep the test fast.
const COOKFILE: &str = r#"
recipe build
        cook.add_unit({
            name    = "slow-step",
            outputs = {"out.txt"},
            command = "sleep 1 && echo done > out.txt",
        })
"#;

#[test]
fn unit_completion_line_reports_measured_wall_time_not_zero() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Cookfile"), COOKFILE).unwrap();

    // Point the shared cache at a private, per-test subdir. Without this,
    // the host-wide persistent cache (~/.cache/cook/cloud) can serve this
    // deterministic sleep+echo unit as a cache hit on a second run (no
    // sleep, no measured duration, and the "cached" line has no timing at
    // all) — masking the very regression this test targets.
    fs::create_dir_all(dir.path().join(".cook")).unwrap();
    let shared = dir.path().join(".cook/shared-cache");
    fs::write(
        dir.path().join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", shared.to_string_lossy()),
    )
    .unwrap();

    let output = Command::new(cook_bin())
        .current_dir(dir.path())
        .arg("build")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "cook build failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The unit's own completion line: "  build/out.txt    <T>s" (the label
    // is the recipe name + the unit's full declared output path — CS-0134
    // era `EngineEvent::NodeCompleted`/unit labeling). Exclude the
    // recipe-total "done" line, which carries its own (already-correct)
    // elapsed time in a differently shaped line.
    let unit_line_re = regex::Regex::new(r"(?m)^\s*build/out\.txt\s+(\d+\.\d+)s\s*$").unwrap();
    let caps = unit_line_re.captures(&stderr).unwrap_or_else(|| {
        panic!("no unit completion line matched `build/out.txt ... <T>s` in stderr:\n{stderr}")
    });
    let unit_secs: f64 = caps[1].parse().unwrap();
    assert!(
        unit_secs >= 0.9,
        "unit completion line must report the measured ~1s sleep, not a hardcoded 0.00s; \
         got {unit_secs}s. Full stderr:\n{stderr}"
    );

    // Sanity: the recipe-total line was already correct before this fix and
    // must remain so — it is a distinct measurement (recipe tracker start
    // time), not derived from the per-unit WorkResult duration this test
    // targets.
    let recipe_line_re =
        regex::Regex::new(r"(?m)^\s*build\s+done\s+\([^)]*\)\s+(\d+\.\d+)s\s*$").unwrap();
    let recipe_caps = recipe_line_re
        .captures(&stderr)
        .unwrap_or_else(|| panic!("no recipe-total completion line found in stderr:\n{stderr}"));
    let recipe_secs: f64 = recipe_caps[1].parse().unwrap();
    assert!(
        recipe_secs >= 0.9,
        "recipe-total line's timing must be unaffected by this fix; got {recipe_secs}s. \
         Full stderr:\n{stderr}"
    );
}
