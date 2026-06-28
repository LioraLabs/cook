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
    Command::new(cook_bin())
        .args(&args)
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
