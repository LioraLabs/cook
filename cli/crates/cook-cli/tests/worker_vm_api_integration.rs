//! CS-0123: pins the spec-blessed Lua API surface available to
//! execute-phase (worker VM) bodies at the binary level: fs.* per
//! Standard Example 8.4.2.1, and the §24.8 codecs in a `>{ … }` body.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

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

fn run_cook(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    // Isolate the shared cache backend per test dir (see file_ref_integration's
    // run_cook for the rationale). Without this, COOK-162 cold fetch-by-key can
    // serve a previous run's output from the global `~/.cache/cook/cloud` store
    // as a spurious first-run hit, so no local StepEntry is recorded and the
    // `.cook/cache` index assertions below break. The shared store points at a
    // private subdir; the StepEntry index at `.cook/cache` is unaffected.
    let cloud_toml = dir.join(".cook/cloud.toml");
    if !cloud_toml.exists() {
        fs::create_dir_all(dir.join(".cook")).map_err(|e| e.to_string())?;
        let shared = dir.join(".cook/shared-cache");
        fs::write(
            &cloud_toml,
            format!("[cache]\ncache_dir = {:?}\n", shared.to_string_lossy()),
        )
        .map_err(|e| e.to_string())?;
    }
    let out = Command::new(cook_binary())
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "cook failed (exit={:?}): stdout={}, stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(out)
}

/// Standard Example 8.4.2.1: lua-expr output + `>{ fs.write(output, ... fs.read(input)) }`.
#[test]
fn fs_api_in_cook_body_lua_block_end_to_end() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("docs/en")).unwrap();
    fs::write(tmp.path().join("docs/en/a.md"), "hello world\n").unwrap();
    let cookfile = r#"
recipe translate
    ingredients "docs/en/**/*.md"
    cook (input:gsub("/en/", "/fr/")) >{
        fs.write(output, fs.read(input):upper())
    }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();
    run_cook(tmp.path(), &["translate"]).expect("run should succeed");
    let out = fs::read_to_string(tmp.path().join("docs/fr/a.md")).unwrap();
    assert_eq!(out, "HELLO WORLD\n");
}

/// §24.8: cook.json_decode / cook.yaml_decode are callable from an
/// execute-phase `>{ … }` cook-body (worker VM), not only at register phase.
#[test]
fn codecs_in_cook_body_lua_block_end_to_end() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
recipe decode
    cook "build/out.txt" >{
        local j = cook.json_decode('{"greeting":"hi"}')
        local y = cook.yaml_decode("name: cook\n")
        fs.write(output, j.greeting .. "-" .. y.name)
    }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();
    run_cook(tmp.path(), &["decode"]).expect("run should succeed");
    let out = fs::read_to_string(tmp.path().join("build/out.txt")).unwrap();
    assert_eq!(out, "hi-cook");
}
