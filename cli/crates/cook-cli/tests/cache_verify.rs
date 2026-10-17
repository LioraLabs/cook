//! End-to-end tests for `cook cache verify` (COOK-167) — proves the bar:
//! a deterministic step verifies clean, a hidden determinant is flagged as a
//! divergence under a matching key, and a `record` step is byte-exempt.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn write_cookfile(body: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Cookfile"), body).unwrap();
    dir
}

fn run_verify(dir: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["cache", "verify"];
    args.extend_from_slice(extra);
    // Verify sandboxes and the artifact store go under the user cache; keep
    // them out of the real one.
    let xdg = TempDir::new().unwrap();
    Command::new(cook_bin())
        .args(&args)
        .env("XDG_CACHE_HOME", xdg.path())
        .current_dir(dir)
        .output()
        .expect("run cook cache verify")
}

#[test]
fn deterministic_step_verifies_clean() {
    let dir = write_cookfile("recipe build\n    cook \"out.txt\" { printf 'stable-bytes' > out.txt }\n");
    let out = run_verify(dir.path(), &[]);
    let combined = format!(
        "STDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "deterministic step must verify clean.\n{combined}");
    assert!(!combined.contains("DIVERGENCE"), "no divergence expected.\n{combined}");
}

#[test]
fn hidden_determinant_is_flagged_as_divergence() {
    let dir = write_cookfile("recipe build\n    cook \"out.txt\" { date +%s%N > out.txt }\n");
    let out = run_verify(dir.path(), &[]);
    let combined = format!(
        "STDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "hidden determinant must yield non-zero exit.\n{combined}");
    assert!(combined.contains("DIVERGENCE"), "expected DIVERGENCE.\n{combined}");
}

#[test]
fn record_step_is_byte_exempt() {
    let dir = write_cookfile("recipe build\n    cook \"gen.txt\" { date +%s%N > gen.txt } nondet\n");
    let out = run_verify(dir.path(), &[]);
    let combined = format!(
        "STDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "nondet step must NOT fail verify.\n{combined}");
    assert!(
        combined.contains("nondet") || combined.contains("record") || combined.contains("waived"),
        "nondet exemption should be visible.\n{combined}"
    );
}

fn verify_clean(cookfile: &str, recipe: &str, what: &str) {
    let dir = write_cookfile(cookfile);
    let out = run_verify(dir.path(), &[recipe]);
    let combined = format!(
        "STDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{what} must verify clean.\n{combined}");
    assert!(!combined.contains("ERROR") && !combined.contains("DIVERGENCE"), "{what}.\n{combined}");
    assert!(
        combined.contains("1 unit(s)") && combined.contains("pass"),
        "{what}: the unit must actually have been re-run.\n{combined}"
    );
}

#[test]
fn probe_sigil_in_shell_unit_verifies_clean() {
    verify_clean(
        "probe greet:who\n    { echo world }\nrecipe hello\n    cook \"out/hello.txt\" { echo $<greet:who> > $<out> }\n",
        "hello",
        "a shell unit consuming a probe sigil",
    );
}

#[test]
fn tool_path_sigil_verifies_clean() {
    // Tool paths live only in the populate run's read view, never on disk.
    verify_clean(
        "tools demo:tc\n    sh\n\nrecipe showpath\n    cook.add_unit({\n        name = \"showpath\", inputs = {}, outputs = {\"path.txt\"},\n        probes = {\"demo:tc\"},\n        command = \"echo $<demo:tc.sh.path> > path.txt\",\n    })\n",
        "showpath",
        "a shell unit consuming a tool-path sigil",
    );
}
