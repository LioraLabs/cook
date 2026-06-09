//! Regression: a native `recipe` that references a recipe registered by a
//! top-level **module call** (e.g. `cook_cc.bin("x")`) via the `$<NAME>`
//! cross-recipe placeholder.
//!
//! Per Standard §10.2 step 2, `$<greeter>` MUST resolve to the recipe in
//! scope (the module-registered `greeter`) and substitute its terminal
//! output — NOT fall through to the env-var rule and hard-error.
//!
//! The bug: codegen classified `$<NAME>` against only the *statically*
//! parsed `recipe` blocks, so a name registered at register-phase by a
//! module target-maker was invisible and `$<greeter>` mis-lowered to
//! `cook.require_env("greeter")`, producing
//! `placeholder $<greeter>: env var 'greeter' was not declared …`.
//!
//! This exercises the build path end-to-end (`cook consume`), which is what
//! the dhewm3 `maps` recipe (`$<dhewm3>`) hits.

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

/// A minimal local module exposing a `tool(name, opts)` target-maker that
/// registers a named recipe via `cook.recipe` — the same shape `cook_cc.bin`
/// uses. The recipe's single unit writes `opts.output`.
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

/// `use toolmod` + a top-level `toolmod.tool(...)` call registers `greeter`
/// at register-phase. The native `consume` recipe references it via
/// `$<greeter>`.
const COOKFILE: &str = r#"use toolmod

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

#[test]
fn native_recipe_resolves_module_registered_recipe_via_dep_ref() {
    let tmp = TempDir::new().expect("tempdir");
    write(tmp.path(), "cook_modules/toolmod.lua", TOOLMOD_LUA);
    write(tmp.path(), "Cookfile", COOKFILE);

    let out = Command::new(cook_bin())
        .arg("consume")
        .current_dir(tmp.path())
        .output()
        .expect("invoke cook consume");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);

    // The specific failure mode this guards against.
    assert!(
        !stderr.contains("was not declared in any config block"),
        "$<greeter> mis-lowered to require_env (the bug):\nstderr={stderr}\nstdout={stdout}",
    );
    assert!(
        out.status.success(),
        "cook consume failed:\nstderr={stderr}\nstdout={stdout}",
    );

    // The dep ref resolved to greeter's output and the edge ordered the
    // build: greeter ran first, so consume's output carries greeter's bytes.
    let produced = std::fs::read_to_string(tmp.path().join("build/out.txt"))
        .expect("build/out.txt should exist");
    assert_eq!(produced, "hi", "consume did not consume greeter's output");
}
