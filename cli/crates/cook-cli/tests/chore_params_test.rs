//! Integration tests for chore parameter binding (COOK-36 Task 4).
//!
//! Exercises the argv → `__cook_params` table plumbing end-to-end:
//!   * Required param supplied: bound as a local in the chore body.
//!   * Defaulted param absent: falls back to the declared default.
//!   * Required param missing: clean diagnostic at invocation time.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // deps → debug (or release)
    path.pop(); // debug → target
    path.push("cook");
    if !path.exists() {
        panic!(
            "cook binary not found at {} — run `cargo build --bin cook` first",
            path.display()
        );
    }
    path
}

fn run_cook_raw(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(cook_binary())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to spawn cook binary")
}

/// `cook greet world` where `chore greet msg` uses `msg` in the body.
/// The body runs `print("hello " .. msg)` and we assert stdout contains
/// "hello world".
#[test]
fn chore_required_param_visible_as_lua_local() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet msg\n    > print(\"hello \" .. msg)\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["greet", "world"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "cook greet world failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("hello world"),
        "expected stdout to contain 'hello world'\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// `cook greet` (no argv) where `chore greet msg="world"` declares a
/// defaulted parameter. The default must bind and the body must print
/// "hello world".
#[test]
fn chore_defaulted_param_falls_back_when_argv_absent() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet msg=\"world\"\n    > print(\"hello \" .. msg)\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["greet"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "cook greet (no argv) failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("hello world"),
        "expected stdout to contain 'hello world'\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// `cook greet` (no argv) where `chore greet msg` declares a *required*
/// parameter. The invocation must fail and stderr must contain the canonical
/// "requires parameter 'msg'" diagnostic.
#[test]
fn chore_missing_required_argv_errors() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet msg\n    > print(msg)\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["greet"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "cook greet (no argv) should have failed\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("requires parameter 'msg'"),
        "expected stderr to contain \"requires parameter 'msg'\"\nstderr: {stderr}"
    );
}

/// `cook greet a b` where `chore greet msg` declares one required parameter.
/// The extra argv must surface the "takes K parameter(s) but M supplied"
/// diagnostic from §7.1.2.
#[test]
fn chore_too_many_argv_errors() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet msg\n    > print(msg)\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["greet", "hello", "world"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "cook greet hello world (1 declared, 2 supplied) should have failed\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("takes 1 parameter") && stderr.contains("2 positional"),
        "expected stderr to contain the takes/supplied diagnostic\nstderr: {stderr}"
    );
}

/// `cook build foo` where `recipe build` declares no parameters. Recipes never
/// take parameters (§6.1); the extra argv must raise the canonical diagnostic.
#[test]
fn recipe_with_argv_errors() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe build\n    > print(\"building\")\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["build", "foo"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "cook build foo should have failed (recipes take no params)\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("recipes do not take parameters")
            || stderr.contains("recipe 'build'"),
        "expected stderr to mention recipes-take-no-params\nstderr: {stderr}"
    );
}

/// `cook lint a.lua b.lua` where `chore lint +files` declares a variadic
/// parameter. The body runs `print(table.concat(files, ","))` and we assert
/// stdout contains "a.lua,b.lua".
#[test]
fn chore_variadic_plus_collects_argv_into_lua_table() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore lint +files\n    > print(table.concat(files, \",\"))\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["lint", "a.lua", "b.lua"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    assert!(stdout.contains("a.lua,b.lua"), "stdout: {stdout}");
}

/// `cook fmt` (no argv) where `chore fmt *files` declares a zero-or-more
/// variadic. The body runs `print("count=" .. #files)` and we assert
/// stdout contains "count=0".
#[test]
fn chore_variadic_star_with_zero_argv_binds_empty_table() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore fmt *files\n    > print(\"count=\" .. #files)\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["fmt"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("count=0"), "stdout: {stdout}");
}

/// `cook lint` (no argv) where `chore lint +files` declares a one-or-more
/// variadic. The invocation must fail with the variadic-empty diagnostic.
#[test]
fn chore_variadic_plus_with_zero_argv_errors() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore lint +files\n    > print(table.concat(files, \",\"))\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["lint"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("requires one or more values for variadic '+files'"),
        "stderr: {stderr}"
    );
}

/// `cook list` on a Cookfile that has a parametric chore must not crash with
/// a nil-index Lua error. This guards against regressing the no-target branch
/// of the register-engine chore dispatch.
#[test]
fn cook_list_does_not_crash_on_parametric_chore() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet msg\n    > print(msg)\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["list"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "cook list with a parametric chore should succeed (the chore body must be skipped, not invoked)\nstdout: {stdout}\nstderr: {stderr}"
    );
}

// ── COOK-36 Task 6: Lua-expression chore param defaults ─────────────────────

/// `cook greet` (no argv) where `chore greet who=(os.getenv("USER") or "world")`
/// declares a Lua-expression default. The default evaluates and the body runs.
#[test]
fn chore_lua_default_evaluates_when_argv_absent() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet who=(os.getenv(\"USER\") or \"world\")\n    > print(\"hello \" .. who)\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["greet"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {stderr}");
    // Either USER env var value or "world" fallback — both are valid.
    assert!(stdout.contains("hello "), "stdout: {stdout}");
}

/// `cook greet alice` where `chore greet who=("fallback")` declares a
/// Lua-expression default. The explicit argv overrides the default.
#[test]
fn chore_lua_default_overridden_by_explicit_argv() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet who=(\"fallback\")\n    > print(\"hello \" .. who)\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["greet", "alice"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    assert!(stdout.contains("hello alice"), "stdout: {stdout}");
    assert!(!stdout.contains("fallback"), "stdout should not contain 'fallback': {stdout}");
}

/// `cook greet` (no argv) where the default expression calls `error("boom")`.
/// The runtime MUST surface a diagnostic containing "default for parameter 'who' raised a Lua error".
#[test]
fn chore_lua_default_error_surfaces_diagnostic() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet who=(error(\"boom\"))\n    > print(who)\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["greet"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "expected failure, stderr: {stderr}");
    assert!(
        stderr.contains("default for parameter 'who' raised a Lua error"),
        "stderr: {stderr}"
    );
}

/// `cook greet` (no argv) where the default expression returns a non-string (`{1,2,3}`).
/// The runtime MUST surface a diagnostic containing "must evaluate to a string".
#[test]
fn chore_lua_default_non_string_surfaces_diagnostic() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet who=({1,2,3})\n    > print(who)\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["greet"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "expected failure, stderr: {stderr}");
    assert!(
        stderr.contains("must evaluate to a string"),
        "stderr: {stderr}"
    );
}

// ── COOK-36 Task 7: $<param-name> placeholder substitution in chore shell steps ──

/// `cook say hello` where `chore say target` uses `$<target>` in a shell step.
/// The shell step must substitute the bound param value (single-quoted for
/// POSIX-shell safety, so `hello` → `'hello'`).
#[test]
fn shell_step_substitutes_param_placeholders() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        // Use printf to get clean output unaffected by echo quoting rules.
        "chore say target\n    @printf 'got: %s\\n' $<target>\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["say", "hello"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("got: hello"), "stdout: {stdout}");
}

/// `cook lint a.lua "b lua"` where `chore lint +files` uses `$<files>` in a shell step.
/// The variadic must expand to individually shell-quoted, space-separated values.
/// A value containing a space must remain a single word when printf expands it.
#[test]
fn shell_step_substitutes_variadic_placeholder_shell_quoted() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore lint +files\n    @printf '%s\\n' $<files>\n",
    )
    .unwrap();
    // Last arg has a space — verify quoting preserves it as one word.
    let out = run_cook_raw(tmp.path(), &["lint", "a.lua", "b lua"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("a.lua\n"), "stdout: {stdout}");
    assert!(stdout.contains("b lua\n"), "stdout: {stdout}");
}

/// `cook say hello` where `chore say target` uses `$<unknown>` in a shell step.
/// The runtime MUST surface a diagnostic about the unknown placeholder.
#[test]
fn shell_step_with_unknown_sigil_in_chore_errors() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore say target\n    @echo $<unknown>\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["say", "hello"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("unknown") || stderr.contains("placeholder"), "stderr: {stderr}");
}
