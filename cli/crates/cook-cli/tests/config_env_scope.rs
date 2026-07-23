//! R1 (COOK-309 / CS-0164): a config block's `var.*` value is reachable ONLY
//! through `$<NAME>` and is NEVER injected into a step's process environment.
//!
//! This pins the 0.8.0 silent-stale-hit hole: a step that read a config value
//! as a *shell* variable (`$VIA_SHELL`) picked it up from the process
//! environment, a determinant the cache key could not see — so switching the
//! selected overlay served the wrong bytes as a clean HIT. Under R1 the shell
//! read sees the value UNSET, while `$<VIA_PLACEHOLDER>` still resolves and
//! re-keys the consuming step. Chore parameter exports still reach the child.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // deps → debug
    path.pop(); // debug → target
    path.push("cook");
    assert!(
        path.exists(),
        "cook binary not found at {} — run `cargo build --bin cook` first",
        path.display()
    );
    path
}

/// Run `cook` in `dir` with the persistent cache isolated into `dir/cache`
/// and publishing off, so the test never touches the user's shared cache.
fn run(dir: &Path, args: &[&str]) -> std::process::Output {
    fs::create_dir_all(dir.join(".cook")).expect("mkdir .cook");
    fs::write(
        dir.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = \"{}\"\n", dir.join("cache").display()),
    )
    .expect("write cloud.toml");
    Command::new(cook_binary())
        .args(args)
        .current_dir(dir)
        .env("COOK_NO_PUBLISH", "1")
        // Deliberately DO NOT set VIA_SHELL in the ambient environment: the
        // only place VIA_SHELL is declared is the config block, so a non-empty
        // shell read could come only from config-env injection.
        .env_remove("VIA_SHELL")
        .output()
        .expect("spawn cook")
}

const COOKFILE: &str = r#"config
    var.VIA_SHELL = "base"
    var.VIA_PLACEHOLDER = "pbase"

config alt
    var.VIA_SHELL = "alt"
    var.VIA_PLACEHOLDER = "palt"

recipe shellread
    cook "out/shell.txt" { printf 'shell=[%s]' "$VIA_SHELL" > $<out> }

recipe placeholder
    cook "out/ph.txt" { printf '%s' "$<VIA_PLACEHOLDER>" > $<out> }

chore param who="ada"
    sh -c 'printf "who=[%s]" "$who" > out/param.txt'
"#;

fn write_cookfile(dir: &Path) {
    fs::create_dir_all(dir.join("out")).unwrap();
    fs::write(dir.join("Cookfile"), COOKFILE).unwrap();
}

fn ok(out: &std::process::Output, ctx: &str) {
    assert!(
        out.status.success(),
        "{ctx} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// R1 core: a config value read as a shell variable is UNSET — not injected
/// into the step's process environment — for both the base and an overlay.
#[test]
fn config_value_is_not_injected_into_shell_env() {
    let tmp = TempDir::new().unwrap();
    write_cookfile(tmp.path());

    let out = run(tmp.path(), &["shellread"]);
    ok(&out, "shellread (base)");
    let s = fs::read_to_string(tmp.path().join("out/shell.txt")).unwrap();
    assert_eq!(
        s, "shell=[]",
        "config VIA_SHELL must NOT reach the process env (base); got {s:?}"
    );

    // Selecting @alt must not change the shell read either — the value is
    // simply unreachable via the process environment, not overlay-dependent.
    let out = run(tmp.path(), &["shellread", "@alt"]);
    ok(&out, "shellread (@alt)");
    let s = fs::read_to_string(tmp.path().join("out/shell.txt")).unwrap();
    assert_eq!(
        s, "shell=[]",
        "config VIA_SHELL must NOT reach the process env (@alt); got {s:?}"
    );
}

/// `$<NAME>` still resolves the config value, and two overlays that produce
/// different values re-key the consuming step: @alt rebuilds, and going back
/// to base is a correct HIT (the right bytes, not a stale wrong HIT).
#[test]
fn placeholder_resolves_and_rekeys_across_overlays() {
    let tmp = TempDir::new().unwrap();
    write_cookfile(tmp.path());

    let out = run(tmp.path(), &["placeholder"]);
    ok(&out, "placeholder (base)");
    assert_eq!(
        fs::read_to_string(tmp.path().join("out/ph.txt")).unwrap(),
        "pbase"
    );

    // Overlay changes the resolved $<VIA_PLACEHOLDER>; the step must re-key
    // and rebuild to the new value, not serve the cached base artifact.
    let out = run(tmp.path(), &["placeholder", "@alt"]);
    ok(&out, "placeholder (@alt)");
    assert_eq!(
        fs::read_to_string(tmp.path().join("out/ph.txt")).unwrap(),
        "palt",
        "overlay must re-key the consuming step (no stale HIT)"
    );

    // Back to base: a correct HIT restores the base bytes.
    let out = run(tmp.path(), &["placeholder"]);
    ok(&out, "placeholder (base again)");
    assert_eq!(
        fs::read_to_string(tmp.path().join("out/ph.txt")).unwrap(),
        "pbase",
        "returning to base must restore the base value (correct HIT)"
    );
}

/// Chore parameter exports (`unit_env_vars`) still reach the child process —
/// R1 removes only the config-env channel, not per-unit param exports.
#[test]
fn chore_param_still_reaches_child_env() {
    let tmp = TempDir::new().unwrap();
    write_cookfile(tmp.path());

    let out = run(tmp.path(), &["param", "grace"]);
    ok(&out, "param chore");
    assert_eq!(
        fs::read_to_string(tmp.path().join("out/param.txt")).unwrap(),
        "who=[grace]",
        "a bound chore parameter must still be exported to the child process env"
    );
}
