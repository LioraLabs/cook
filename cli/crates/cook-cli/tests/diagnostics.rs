//! COOK-191 Task 3 / CS-0126 repro 1: a config-block bare `NAME "value"`
//! statement (the pre-CS-0011 VarDecl shape) must fail fast at parse time
//! with a source-mapped did-you-mean diagnostic — never reaching the Lua
//! VM, and never printing an implementation traceback by default.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
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
/// `[string "crates/cook-execute/src/pool.rs:..."]` chunk name — and must
/// not print a traceback by default.
#[test]
fn execute_phase_lua_error_is_source_mapped_and_clean() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(
        tmp.path().join("Cookfile"),
        "recipe boom\n    cook \"out.txt\" >{ error(\"kaboom from lua\") }\n",
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
        "recipe boom\n    cook \"out.txt\" >{ error(\"kaboom from lua\") }\n",
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

#[test]
fn output_json_emits_structured_diagnostic() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(
        tmp.path().join("Cookfile"),
        "config\n    OUTDIR \"build\"\n\nrecipe hello\n    echo hi\n",
    )
    .expect("write Cookfile");
    let out = Command::new(cook_bin())
        .arg("--output")
        .arg("json")
        .arg("hello")
        .current_dir(tmp.path())
        .output()
        .expect("invoke cook");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    let line = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let diagnostic: Value = serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("stderr was not json ({e}): {stderr}"));
    assert_eq!(diagnostic["type"], "diagnostic");
    assert_eq!(diagnostic["code"], "parse-error");
    assert_eq!(diagnostic["file"], "Cookfile");
    assert_eq!(diagnostic["line"], 2);
    assert!(
        diagnostic["message"]
            .as_str()
            .unwrap_or("")
            .contains("did you mean"),
        "diagnostic: {diagnostic}"
    );
}

/// Everything `cook` prints for one failing command, as a user sees it:
/// the progress line and the final diagnostic are both output, and a defect
/// in either is a defect in the output.
fn everything_printed(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// COOK-426 / CS-0215: a failing shell block is reported as the body the
/// author wrote. The `set -e` prelude is `shell_block::compose`'s, and it
/// reached the user through the progress line while the final diagnostic
/// one row below stripped it — the same two-renderer split CS-0211 closed
/// for the failure's location.
#[test]
fn a_failing_block_is_reported_without_the_compose_prelude() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(tmp.path().join("src.txt"), "hi\n").expect("write source");
    std::fs::write(
        tmp.path().join("Cookfile"),
        "recipe build\n    gather \"src.txt\"\n    cook \"out/$<in.stem>.o\" {\n        echo working\n        false\n    }\n",
    )
    .expect("write Cookfile");
    let out = Command::new(cook_bin())
        .arg("build")
        .current_dir(tmp.path())
        .output()
        .expect("invoke cook");
    let printed = everything_printed(&out);
    assert!(!out.status.success());
    // The body is quoted, so the author can see what failed…
    assert!(printed.contains("echo working"), "printed: {printed}");
    // …and nothing quotes the prelude they did not write.
    assert!(!printed.contains("set -e"), "printed: {printed}");
    assert!(!printed.contains("COOK_CMD_FAILED"), "printed: {printed}");
}

/// COOK-426 / CS-0215: `COOK_CMD_FAILED` is how a failure travels from a Lua
/// host to the crate that prints it. It reached the terminal instead: a
/// register-phase `cook.sh` failure has no `TaskFailures` to be decoded out
/// of, and the register path had no decoding end at all, so the user was
/// shown the protocol rather than the message.
///
/// The location is CS-0211's, unmet on this side until now: the register
/// phase supplied a constant zero to `cook.sh` while the execute phase
/// walked the stack — the reverse of what §{lua.cook-sh} recorded.
#[test]
fn a_register_phase_command_failure_reads_as_a_failure_not_as_a_wire_format() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(
        tmp.path().join("Cookfile"),
        "register\n    cook.sh(\"false\")\n\nrecipe build\n    cook \"out.txt\" { touch $<out> }\n",
    )
    .expect("write Cookfile");
    let out = Command::new(cook_bin())
        .arg("build")
        .current_dir(tmp.path())
        .output()
        .expect("invoke cook");
    let printed = everything_printed(&out);
    assert!(!out.status.success());
    assert!(!printed.contains("COOK_CMD_FAILED"), "printed: {printed}");
    assert!(
        printed.contains("Cookfile:2: command failed (exit 1): false"),
        "printed: {printed}"
    );
}

/// The walk is the shared one, so a `cook.sh` inside a module reports the
/// Cookfile line that ENTERED the module — the line a reader can act on —
/// rather than a line in a file they did not write, and never `Cookfile:0`.
/// Pinned as an exact string: an assertion that merely forbids `Cookfile:0`
/// passes under two different rules and can therefore document the wrong one.
#[test]
fn a_register_phase_failure_from_a_module_names_the_line_that_entered_it() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("lua")).expect("module dir");
    std::fs::write(
        tmp.path().join("lua/boom.lua"),
        "local M = {}\nfunction M.go()\n    cook.sh(\"false\")\nend\nreturn M\n",
    )
    .expect("write module");
    std::fs::write(
        tmp.path().join("Cookfile"),
        "use ./lua/boom.lua\n\nregister\n    boom.go()\n\nrecipe build\n    cook \"out.txt\" { touch $<out> }\n",
    )
    .expect("write Cookfile");
    let out = Command::new(cook_bin())
        .arg("build")
        .current_dir(tmp.path())
        .output()
        .expect("invoke cook");
    let printed = everything_printed(&out);
    assert!(!out.status.success());
    assert!(!printed.contains("COOK_CMD_FAILED"), "printed: {printed}");
    // Line 4 is `boom.go()` in the Cookfile, not line 3 of the module.
    assert!(
        printed.contains("Cookfile:4: command failed (exit 1): false"),
        "printed: {printed}"
    );
}
