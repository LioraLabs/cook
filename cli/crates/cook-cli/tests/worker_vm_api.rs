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
    // Isolate the shared cache backend per test dir. Without this, a warm
    // global `~/.cache/cook/cloud` store could serve these steps' outputs as
    // first-run cache hits — the Lua body would never execute on the worker
    // VM and the tests would pass vacuously. Isolation guarantees a cold
    // first run so the `>{ … }` bodies actually exercise the worker-VM API.
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

/// CS-0200: `cook.export` / `cook.import` are register-phase only, and the
/// execute-phase VM refuses both by name.
///
/// The refusal is deliberately not a conformance fixture: the corpus harness
/// stops after register, so a fixture there would pass against the broken
/// implementation and pin nothing. This runs the binary and reads the
/// diagnostic a user actually sees.
#[test]
fn cook_import_in_a_worker_body_is_refused_by_name() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
register
    cook.export("thing", { a = "hello-from-register" })

recipe build
    cook "out.txt" >{
        fs.write(output, tostring(cook.import("thing")))
    }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    let err = run_cook(tmp.path(), &["build"]).expect_err("execute-phase import must fail");
    assert!(
        err.contains("cook.import: register-phase only"),
        "diagnostic must name the function and the phase: {err}"
    );
    assert!(err.contains("CS-0200"), "diagnostic must cite the change: {err}");

    // The point of the change: before CS-0200 this SUCCEEDED and wrote "nil",
    // because the worker's export table was built empty and never seeded from
    // the register-phase store. A silent nil is what §12.3.4 forbade and what
    // §24.5 permitted; the contradiction is why the surface was withdrawn.
    assert!(
        !tmp.path().join("out.txt").exists(),
        "the body must not have run to completion"
    );
}

#[test]
fn cook_export_in_a_worker_body_is_refused_by_name() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
recipe build
    cook "out.txt" >{
        cook.export("late", { a = 1 })
        fs.write(output, "unreachable")
    }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    let err = run_cook(tmp.path(), &["build"]).expect_err("execute-phase export must fail");
    assert!(
        err.contains("cook.export: register-phase only"),
        "diagnostic must name the function and the phase: {err}"
    );
}
