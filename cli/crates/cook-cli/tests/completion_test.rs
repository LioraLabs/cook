//! End-to-end shell-completion tests.
//!
//! These drive the real binary the way a shell does — `COMPLETE=<shell>` plus
//! `_CLAP_COMPLETE_INDEX` for the cursor position — because the interesting
//! behavior (recipes reaching the completer at all, the `+` escape, the
//! per-subcommand namespaces) only exists in the assembled command and cannot
//! be observed from a unit test.
//!
//! `CARGO_BIN_EXE_cook` is the binary cargo builds for *this* test run, so
//! these can never silently assert against a stale `target/debug/cook`.

use std::path::Path;
use std::process::Command;

const COOK: &str = env!("CARGO_BIN_EXE_cook");

/// Ask the binary for candidates at `index`, exactly as a shell would.
fn complete(dir: &Path, args: &[&str], index: usize) -> Vec<String> {
    let output = Command::new(COOK)
        .current_dir(dir)
        .env("COMPLETE", "fish")
        .env("_CLAP_COMPLETE_INDEX", index.to_string())
        .arg("--")
        .arg("cook")
        .args(args)
        .output()
        .expect("failed to run cook");
    assert!(
        output.stderr.is_empty(),
        "completion wrote to stderr, which corrupts the shell's display: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        // fish candidates are `value\thelp`; keep the value.
        .map(|line| line.split('\t').next().unwrap_or_default().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.in"), "hi\n").unwrap();
    // The qualified name `ns.unit` — which `cook test` must offer a `ns`
    // namespace for — can only arise from an import; dotted names are rejected
    // at the declaration site.
    std::fs::create_dir_all(dir.path().join("ns/src")).unwrap();
    std::fs::write(dir.path().join("ns/src/b.in"), "hi\n").unwrap();
    std::fs::write(
        dir.path().join("ns/Cookfile"),
        r#"recipe unit
    ingredients "src/b.in"
    cook "out/n.txt" { cp $<in> $<out> }
"#,
    )
    .unwrap();
    // `test` collides with a builtin and must complete `+`-escaped; `deploy`
    // does not and must complete bare.
    std::fs::write(
        dir.path().join("Cookfile"),
        r#"import ns ./ns

config
    env.MODE = "debug"

config release
    env.MODE = "release"

recipe deploy
    ingredients "src/a.in"
    cook "out/d.txt" { cp $<in> $<out> }

recipe test
    ingredients "src/a.in"
    cook "out/t.txt" { cp $<in> $<out> }

chore tidy
    echo tidy
"#,
    )
    .unwrap();
    dir
}

#[test]
fn bare_target_offers_recipes_and_chores() {
    let dir = fixture();
    let got = complete(dir.path(), &[""], 1);
    assert!(got.contains(&"deploy".to_string()), "recipes: {got:?}");
    assert!(got.contains(&"tidy".to_string()), "chores: {got:?}");
    // Builtins stay reachable.
    assert!(got.contains(&"menu".to_string()), "builtins: {got:?}");
}

#[test]
fn recipes_shadowed_by_a_builtin_complete_with_the_plus_escape() {
    let dir = fixture();
    let got = complete(dir.path(), &[""], 1);
    // `cook test` runs the test runner, so the recipe is only reachable as
    // `+test` — completion has to say so or the name is undiscoverable.
    assert!(
        got.contains(&"+test".to_string()),
        "expected +test in {got:?}"
    );
    // A non-colliding recipe must NOT be escaped.
    assert!(
        !got.contains(&"+deploy".to_string()),
        "over-escaped: {got:?}"
    );
}

#[test]
fn bare_recipe_offers_presets_with_the_at_sigil() {
    let dir = fixture();
    let got = complete(dir.path(), &["deploy", ""], 2);
    // After a bare recipe the preset needs `@`; a bare positional is rejected
    // as a chore param.
    assert!(
        got.contains(&"@release".to_string()),
        "expected @release in {got:?}"
    );
    // The unnamed base config block is not selectable.
    assert!(
        !got.iter().any(|c| c == "@"),
        "unnamed config offered: {got:?}"
    );
}

#[test]
fn why_offers_recipes_but_not_chores() {
    let dir = fixture();
    let got = complete(dir.path(), &["why", ""], 2);
    assert!(got.contains(&"deploy".to_string()), "recipes: {got:?}");
    assert!(
        !got.contains(&"tidy".to_string()),
        "chore leaked into why: {got:?}"
    );
}

#[test]
fn test_scope_offers_namespaces_and_excludes_chores() {
    let dir = fixture();
    let got = complete(dir.path(), &["test", ""], 2);
    assert!(got.contains(&"ns.unit".to_string()), "recipes: {got:?}");
    // `resolve_test_scope` accepts a namespace prefix, which is implied by a
    // dotted name and never appears as a name of its own.
    assert!(got.contains(&"ns".to_string()), "namespace: {got:?}");
    // `cook test <chore>` is an error, so offering one would be a lie.
    assert!(
        !got.contains(&"tidy".to_string()),
        "chore leaked into test: {got:?}"
    );
}

#[test]
fn root_anchored_targets_are_never_offered() {
    let dir = fixture();
    // §20.2.4 reserves `//<name>` and requires it be rejected, so a candidate
    // starting with `//` would always be a guaranteed-failing invocation.
    for args in [vec![""], vec!["why", ""]] {
        let index = args.len();
        let got = complete(dir.path(), &args, index);
        assert!(
            !got.iter().any(|c| c.starts_with("//")),
            "offered a reserved root-anchored target: {got:?}"
        );
    }
}

#[test]
fn module_internal_recipes_are_not_offered_but_stay_runnable() {
    // Modules synthesise `__`-prefixed recipes for their own bookkeeping (see
    // cook_cc's cc.config_header); callers reference them through a Lua return
    // value, never by typing. Proposing them is noise. Registering one here
    // directly mirrors what a module does at register time.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.in"), "hi\n").unwrap();
    std::fs::write(
        dir.path().join("Cookfile"),
        r#"recipe deploy
    ingredients "src/a.in"
    cook "out/d.txt" { cp $<in> $<out> }

recipe __internal_helper
    ingredients "src/a.in"
    cook "out/h.txt" { cp $<in> $<out> }
"#,
    )
    .unwrap();

    let got = complete(dir.path(), &[""], 1);
    assert!(got.contains(&"deploy".to_string()), "recipes: {got:?}");
    assert!(
        !got.iter().any(|c| c.contains("__internal_helper")),
        "module-internal recipe was offered: {got:?}"
    );

    // Hidden from completion, but not removed from the CLI: `cook list` still
    // prints it and it still builds.
    let run = Command::new(COOK)
        .current_dir(dir.path())
        .arg("__internal_helper")
        .output()
        .expect("failed to run cook");
    assert!(
        run.status.success(),
        "hiding a name from completion must not make it unrunnable: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn emitting_the_registration_script_does_not_run_the_cookfile() {
    // The registration script is sourced from a shell's startup file, so it
    // runs on every new shell. It needs only the binary's name — if it built
    // the augmented command it would load the workspace, and starting a shell
    // anywhere near a Cookfile would execute that Cookfile's register-phase
    // Lua. Proven with a side effect rather than a timing, which would be
    // flaky.
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("register-ran");
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.in"), "hi\n").unwrap();
    std::fs::write(
        dir.path().join("Cookfile"),
        format!(
            r#"config
    env.MODE = (function()
        local f = io.open("{}", "w")
        f:write("ran")
        f:close()
        return "debug"
    end)()

recipe deploy
    ingredients "src/a.in"
    cook "out/d.txt" {{ cp $<in> $<out> }}
"#,
            marker.display()
        ),
    )
    .unwrap();

    let registration = Command::new(COOK)
        .current_dir(dir.path())
        .env("COMPLETE", "fish")
        .output()
        .expect("failed to run cook");
    assert!(
        !registration.stdout.is_empty(),
        "registration emitted no script"
    );
    assert!(
        !marker.exists(),
        "emitting the registration script executed the Cookfile's Lua"
    );

    // ...whereas actually completing must.
    let got = complete(dir.path(), &[""], 1);
    assert!(got.contains(&"deploy".to_string()), "recipes: {got:?}");
    assert!(marker.exists(), "completing did not run the register phase");
}

#[test]
fn a_directory_with_no_cookfile_completes_builtins_and_does_not_fail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let got = complete(dir.path(), &[""], 1);
    assert!(got.contains(&"init".to_string()), "builtins: {got:?}");
}

#[test]
fn an_unparseable_cookfile_yields_no_recipes_rather_than_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Cookfile"),
        "recipe build\n  !!! not cook\n",
    )
    .unwrap();
    // Completion must degrade to builtins, silently: a diagnostic here would
    // land in the middle of the user's prompt.
    let got = complete(dir.path(), &[""], 1);
    assert!(got.contains(&"menu".to_string()), "builtins: {got:?}");
}
