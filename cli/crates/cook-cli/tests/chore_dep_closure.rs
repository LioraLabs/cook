//! Regression (COOK-74): a chore whose body references a recipe via `$<NAME>`
//! — with NO explicit `: dep` — must pull that recipe into the build closure.
//!
//! Per §10.6, a name reference establishes a cross-recipe dependency edge.
//! CS-0094 fixed the *path* resolution (`$<app>` lowers to
//! `cook.dep_output("app")` instead of `cook.require_env`), but the referent
//! was never scheduled: `cook run` queued only the chore node and the body
//! failed with exit 127 against the missing artifact. The explicit
//! `chore run: app` workaround proved the closure machinery itself works —
//! only the inferred edge was missing.

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

const COOKFILE: &str = r#"recipe app
    ingredients "src/hello.txt"
    cook "build/app.txt" {
        mkdir -p build && cat $<in> > $<out>
    }

chore run
    cat $<app>
"#;

#[test]
fn chore_dep_ref_pulls_recipe_into_build_closure() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("src")).expect("mkdir src");
    std::fs::write(tmp.path().join("src/hello.txt"), "hi from app\n").expect("write input");
    std::fs::write(tmp.path().join("Cookfile"), COOKFILE).expect("write Cookfile");

    let out = Command::new(cook_bin())
        .arg("run")
        .current_dir(tmp.path())
        .output()
        .expect("invoke cook run");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "cook run failed — $<app> did not pull `app` into the build closure:\nstderr={stderr}\nstdout={stdout}",
    );
    assert!(
        tmp.path().join("build/app.txt").exists(),
        "app never built despite the $<app> body reference\nstderr={stderr}\nstdout={stdout}",
    );
}
