//! Regression: one feature (module-registered recipe consumed via
//! `$<NAME>`) must behave identically on a bare single Cookfile and on the
//! same Cookfile mounted as a workspace member via `import`. This is the
//! class of bug where features work single-file but silently break on the
//! workspace path (or vice versa).

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

const TOOLMOD_LUA: &str = r#"
local M = {}

function M.tool(name, opts)
    cook.recipe(name, {}, function()
        cook.add_unit({
            inputs  = {},
            output  = opts.output,
            command = opts.command,
        })
    end)
end

return M
"#;

const MEMBER_COOKFILE: &str = r#"use toolmod

toolmod.tool("greeter", { output = "build/greeting.txt", command = "mkdir -p build && printf hi > build/greeting.txt" })

recipe consume
    cook "build/out.txt" {
        mkdir -p build && cat $<greeter> > $<out>
    }
"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("mkdir -p");
    }
    std::fs::write(p, body).expect("write file");
}

fn run_cook(dir: &std::path::Path, target: &str) -> (bool, String) {
    let out = Command::new(cook_bin())
        .arg(target)
        .current_dir(dir)
        .output()
        .expect("invoke cook");
    let diag = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), diag)
}

/// Bare single Cookfile: `cook consume` resolves the module-registered
/// `greeter` via `$<greeter>` (§10.2 step 2 single path — already worked
/// before the dual-path collapse).
#[test]
fn module_recipe_dep_ref_on_bare_cookfile() {
    let tmp = TempDir::new().expect("tempdir");
    write(tmp.path(), "cook_modules/toolmod.lua", TOOLMOD_LUA);
    write(tmp.path(), "Cookfile", MEMBER_COOKFILE);

    let (ok, diag) = run_cook(tmp.path(), "consume");
    assert!(ok, "bare cook consume failed:\n{diag}");
    let produced =
        std::fs::read_to_string(tmp.path().join("build/out.txt")).expect("out.txt");
    assert_eq!(produced, "hi");
}

/// The SAME Cookfile mounted under `import sub ./sub`: `cook sub.consume`
/// must behave identically. Before the dual-path collapse this failed — the
/// workspace path never ran the §10.2 step 2 discovery re-codegen, so
/// `$<greeter>` mis-lowered to `cook.require_env("greeter")`.
#[test]
fn module_recipe_dep_ref_equivalent_when_mounted_via_import() {
    let tmp = TempDir::new().expect("tempdir");
    write(tmp.path(), "sub/cook_modules/toolmod.lua", TOOLMOD_LUA);
    write(tmp.path(), "sub/Cookfile", MEMBER_COOKFILE);
    write(
        tmp.path(),
        "Cookfile",
        "import sub ./sub\nrecipe top: sub.consume\n    echo done\n",
    );

    let (ok, diag) = run_cook(tmp.path(), "sub.consume");
    assert!(
        !diag.contains("was not declared in any config block"),
        "$<greeter> mis-lowered to require_env on the workspace path:\n{diag}"
    );
    assert!(ok, "mounted cook sub.consume failed:\n{diag}");
    let produced =
        std::fs::read_to_string(tmp.path().join("sub/build/out.txt")).expect("sub out.txt");
    assert_eq!(produced, "hi");
}
