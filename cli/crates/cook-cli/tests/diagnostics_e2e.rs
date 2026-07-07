//! COOK-191 Task 3 / CS-0126 repro 1: a config-block bare `NAME "value"`
//! statement (the pre-CS-0011 VarDecl shape) must fail fast at parse time
//! with a source-mapped did-you-mean diagnostic — never reaching the Lua
//! VM, and never printing an implementation traceback by default.

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

#[test]
fn config_bare_value_gets_did_you_mean_and_no_traceback() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(
        tmp.path().join("Cookfile"),
        "config\n    OUTDIR \"build\"\n\nrecipe hello\n    echo hi\n",
    )
    .expect("write Cookfile");
    let out = Command::new(cook_bin())
        .arg("hello")
        .current_dir(tmp.path())
        .output()
        .expect("invoke cook");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("config values are Lua assignments"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("OUTDIR = \"build\""), "stderr: {stderr}");
    assert!(!stderr.contains("stack traceback"), "stderr: {stderr}");
    assert!(
        !stderr.contains("__cook_run_config_blocks"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("attempt to call a nil value"),
        "stderr: {stderr}"
    );
}

/// COOK-191 Task 5 / CS-0126 repro 2: an execute-phase `>` Lua step that
/// errors must report `Cookfile:LINE:` — not the opaque
/// `[string "crates/cook-luaotp/src/pool.rs:..."]` chunk name — and must
/// not print a traceback by default.
#[test]
fn execute_phase_lua_error_is_source_mapped_and_clean() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(
        tmp.path().join("Cookfile"),
        "recipe boom\n    > error(\"kaboom from lua\")\n",
    )
    .expect("write Cookfile");
    let out = Command::new(cook_bin())
        .arg("boom")
        .current_dir(tmp.path())
        .output()
        .expect("invoke cook");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("Cookfile:2:"), "stderr: {stderr}");
    assert!(stderr.contains("kaboom from lua"), "stderr: {stderr}");
    assert!(!stderr.contains("[string \""), "stderr: {stderr}");
    assert!(!stderr.contains("pool.rs"), "stderr: {stderr}");
    assert!(!stderr.contains("stack traceback"), "stderr: {stderr}");
}

/// COOK-191 Task 1: `COOK_BACKTRACE=1` restores the full Lua traceback for
/// an execute-phase error, opting back into implementation detail.
#[test]
fn cook_backtrace_optin_restores_traceback() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(
        tmp.path().join("Cookfile"),
        "recipe boom\n    > error(\"kaboom from lua\")\n",
    )
    .expect("write Cookfile");
    let out = Command::new(cook_bin())
        .arg("boom")
        .env("COOK_BACKTRACE", "1")
        .current_dir(tmp.path())
        .output()
        .expect("invoke cook");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("stack traceback:"), "stderr: {stderr}");
}
