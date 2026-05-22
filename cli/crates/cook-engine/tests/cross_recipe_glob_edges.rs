//! CS-0085 §22.1.2 terminal-output rule: a recipe whose outputs[] contains
//! a glob pattern MUST NOT have any of those patterns syntactically match a
//! literal inputs[] entry declared by any other recipe in the same workspace.
//! Detection is purely syntactic at register time (no filesystem access).

use std::fs;

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

/// Cookfile where recipe `a` has a globbed output `build/**` and recipe `b`
/// lists `build/foo.o` as a literal input. This MUST fail at register time.
fn write_cookfile_glob_then_file_input(wd: &std::path::Path) {
    fs::write(
        wd.join("Cookfile"),
        r#"recipe a
    >>{
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/**" },
            command = "mkdir -p build && cp src.c build/foo.o",
        })
    }

recipe b
    >>{
        cook.add_unit({
            inputs  = { "build/foo.o" },
            outputs = { "downstream.bin" },
            command = "cat build/foo.o > downstream.bin",
        })
    }
"#,
    )
    .unwrap();
    fs::write(wd.join("src.c"), b"x").unwrap();
}

/// Cookfile where recipe `b` depends on recipe `a` via a `requires` edge
/// (colon syntax). Recipe `b` does NOT list any of `a`'s glob output paths
/// as a literal input — it only carries a `requires` ordering edge.
/// This MUST succeed at register time.
fn write_cookfile_glob_then_requires_only(wd: &std::path::Path) {
    fs::write(
        wd.join("Cookfile"),
        r#"recipe a
    >>{
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "build/**" },
            command = "mkdir -p build && cp src.c build/foo.o",
        })
    }

recipe b: a
    >>{
        cook.add_unit({
            inputs  = { "src.c" },
            outputs = { "downstream.bin" },
            command = "echo ok > downstream.bin",
        })
    }
"#,
    )
    .unwrap();
    fs::write(wd.join("src.c"), b"x").unwrap();
}

/// Point the cook cache at a private per-test directory so test runs sharing
/// the same source content / command hash do not collide on artifact keys in
/// the system-wide local backend (~/.cache/cook/cloud).
fn isolate_cache(wd: &std::path::Path) {
    let cache_dir = wd.join(".cook/local-cache");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

#[test]
fn cross_recipe_file_input_to_globbed_output_errors_at_register() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    write_cookfile_glob_then_file_input(wd);
    isolate_cache(wd);

    let out = std::process::Command::new(cook_binary())
        .arg("+b")
        .current_dir(wd)
        .output()
        .expect("cook invocation");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{stdout}{stderr}");

    assert!(
        !out.status.success(),
        "register-time error MUST fail the run; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        combined.contains("a"),
        "diagnostic must name upstream recipe 'a': {combined}"
    );
    assert!(
        combined.contains("b"),
        "diagnostic must name downstream recipe 'b': {combined}"
    );
    assert!(
        combined.contains("build/foo.o"),
        "diagnostic must name the offending input path 'build/foo.o': {combined}"
    );
    assert!(
        combined.contains("build/**"),
        "diagnostic must name the matching pattern 'build/**': {combined}"
    );
}

#[test]
fn cross_recipe_requires_edge_to_globbed_output_is_allowed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wd = tmp.path();
    write_cookfile_glob_then_requires_only(wd);
    isolate_cache(wd);

    let out = std::process::Command::new(cook_binary())
        .arg("+b")
        .current_dir(wd)
        .output()
        .expect("cook invocation");

    assert!(
        out.status.success(),
        "a requires-only edge to a globbed-output recipe MUST be allowed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
