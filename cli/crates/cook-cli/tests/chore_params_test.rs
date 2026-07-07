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

fn run_cook_raw_env(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(cook_binary());
    cmd.args(args).current_dir(dir);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("failed to spawn cook binary")
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

#[test]
fn parametric_chore_env_placeholder_resolves() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "config\n    env.MODE = os.getenv(\"MODE\") or \"dev\"\n\nchore greet who=\"world\"\n    @echo hello $<who>, mode=$<MODE>\n",
    )
    .unwrap();

    let out = run_cook_raw_env(tmp.path(), &["greet", "alex"], &[("MODE", "prod")]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "parametric chore env placeholder failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("hello alex, mode=prod"),
        "expected param + env placeholders in output\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn parametric_chore_recipe_placeholder_creates_edge() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe build\n    cook \"out/tool.txt\" { mkdir -p out && printf tool > $<out> }\n\nchore show what=\"x\"\n    @cat $<build>\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["show"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "parametric chore recipe placeholder failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("tool"),
        "expected chore to build and cat recipe output\nstdout: {stdout}\nstderr: {stderr}"
    );
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

// ── COOK-36 Task 8: chore params exported as env vars to shell children ───────

/// `cook say production` where `chore say target` uses `$target` in a shell step
/// (the env-var form, not the $<target> sigil). The param must be exported as an
/// env var so the child shell can read it.
#[test]
fn chore_param_exported_as_env_var_to_shell_child() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore say target\n    @sh -c 'echo \"env_target=$target\"'\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["say", "production"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("env_target=production"), "stdout: {stdout}");
}

/// `cook lint a.lua b.lua` where `chore lint +files` uses `$files` in a shell step.
/// The variadic must be space-joined into a single flat env-var string.
#[test]
fn variadic_param_exported_as_space_joined_env_var() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore lint +files\n    @sh -c 'echo \"env_files=$files\"'\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["lint", "a.lua", "b.lua"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("env_files=a.lua b.lua"), "stdout: {stdout}");
}

/// `cook say` (no argv) where `chore say target="staging"` uses the default.
/// The env var must carry the default value when no argv is supplied.
#[test]
fn defaulted_param_env_var_uses_default_when_argv_absent() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore say target=\"staging\"\n    @sh -c 'echo \"env=$target\"'\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["say"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("env=staging"), "stdout: {stdout}");
}

// ── COOK-36 Task 9: @PRESET sigil + --config/-c flag + -- separator ──────────

#[test]
fn preset_via_at_sigil() {
    let tmp = TempDir::new().unwrap();
    // Use sh -c with $target (env var form, no quoting artifact) to avoid
    // the shell-quoting that $<target> introduces around the value.
    fs::write(
        tmp.path().join("Cookfile"),
        "config rel\n    env.MODE = \"rel\"\n\nchore show target\n    @sh -c 'echo \"target=$target mode=${MODE:-none}\"'\n",
    ).unwrap();
    let out = run_cook_raw(tmp.path(), &["show", "production", "@rel"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    assert!(stdout.contains("target=production"), "stdout: {stdout}");
    assert!(stdout.contains("mode=rel"), "stdout: {stdout}");
}

#[test]
fn preset_via_long_flag() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "config rel\n    env.MODE = \"rel\"\n\nchore show target\n    @sh -c 'echo \"target=$target mode=${MODE:-none}\"'\n",
    ).unwrap();
    let out = run_cook_raw(tmp.path(), &["show", "production", "--config", "rel"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("target=production"));
    assert!(stdout.contains("mode=rel"));
}

#[test]
fn preset_via_short_flag() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "config rel\n    env.MODE = \"rel\"\n\nchore show target\n    @sh -c 'echo \"target=$target mode=${MODE:-none}\"'\n",
    ).unwrap();
    let out = run_cook_raw(tmp.path(), &["show", "production", "-c", "rel"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("target=production"));
    assert!(stdout.contains("mode=rel"));
}

#[test]
fn end_of_options_separator_treats_at_as_literal() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore show target\n    > print(target)\n",
    ).unwrap();
    let out = run_cook_raw(tmp.path(), &["show", "--", "@latest"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("@latest"), "stdout: {stdout}");
}

#[test]
fn two_presets_via_sigil_errors() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "config a\n\nconfig b\n\nchore noop\n    > print(\"ok\")\n",
    ).unwrap();
    let out = run_cook_raw(tmp.path(), &["noop", "@a", "@b"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("multiple config presets"), "stderr: {stderr}");
}

#[test]
fn mixed_sigil_and_flag_errors() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "config a\n\nchore noop\n    > print(\"ok\")\n",
    ).unwrap();
    let out = run_cook_raw(tmp.path(), &["noop", "@a", "--config", "a"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("supply only one") || stderr.contains("multiple config presets"), "stderr: {stderr}");
}

#[test]
fn legacy_second_positional_emits_migration_hint() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "config release\n\nchore noop\n    > print(\"ok\")\n",
    ).unwrap();
    let out = run_cook_raw(tmp.path(), &["noop", "release"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    // Diagnostic should suggest @release or --config release
    assert!(
        stderr.contains("@release") || stderr.contains("--config release"),
        "expected migration hint in stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// COOK-36 Task 11 — end-to-end smoke
// ---------------------------------------------------------------------------
//
// A single Cookfile exercises every chore-parameter surface in one go:
//   - Required parameter (`target`)
//   - Defaulted-string parameter (`host="prod"`)
//   - Lua-expression-default parameter (`version=("v0")`)
//   - Zero-or-more variadic (`*extras`)
// And every binding surface:
//   - Inline Lua (`>>`) — register-phase locals
//   - Shell `$<name>` placeholder substitution (scalar + variadic)
//   - Shell env-var export
//   - Execute-phase Lua (`>`) locals (via the prelude prepended to LuaChunk units)
//
// Two invocations: one where defaults fire (small argv) and one where every
// position is explicitly supplied (full argv including variadic).

#[test]
fn comprehensive_chore_params_smoke_defaults_fire() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore demo target host=\"prod\" version=(\"v0\") *extras\n\
         \x20\x20\x20\x20>> print(\"register: target=\" .. target)\n\
         \x20\x20\x20\x20@echo \"shell-sub: $<target> $<host> $<version> $<extras>\"\n\
         \x20\x20\x20\x20@sh -c 'echo \"env: $target/$host/$version/$extras\"'\n\
         \x20\x20\x20\x20> print(\"exec-lua: \" .. target .. \" \" .. host .. \" \" .. version .. \" #extras=\" .. #extras)\n",
    ).unwrap();

    let out = run_cook_raw(tmp.path(), &["demo", "production"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}\nstdout: {stdout}");

    // Inline-Lua (`>>`) sees the register-phase locals.
    assert!(stdout.contains("register: target=production"), "stdout: {stdout}");
    // Shell placeholders resolve through the unified sigil path; declared
    // params are quoted via cook.__quote_param.
    assert!(stdout.contains("shell-sub: 'production' 'prod' 'v0' "), "stdout: {stdout}");
    // Env-vars: defaults fire when argv is exhausted.
    assert!(stdout.contains("env: production/prod/v0/"), "stdout: {stdout}");
    // Execute-phase Lua sees the prelude-injected locals.
    assert!(stdout.contains("exec-lua: production prod v0 #extras=0"), "stdout: {stdout}");
}

#[test]
fn comprehensive_chore_params_smoke_argv_overrides_defaults() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore demo target host=\"prod\" version=(\"v0\") *extras\n\
         \x20\x20\x20\x20>> print(\"register: target=\" .. target)\n\
         \x20\x20\x20\x20@echo \"shell-sub: $<target> $<host> $<version> $<extras>\"\n\
         \x20\x20\x20\x20@sh -c 'echo \"env: $target/$host/$version/$extras\"'\n\
         \x20\x20\x20\x20> print(\"exec-lua: \" .. target .. \" \" .. host .. \" \" .. version .. \" extras=\" .. table.concat(extras, \",\"))\n",
    ).unwrap();

    // argv: target, host, version, then two variadic elements.
    let out = run_cook_raw(tmp.path(), &[
        "demo", "production", "myhost", "v1.2.3", "a.lua", "b.lua",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}\nstdout: {stdout}");

    assert!(stdout.contains("register: target=production"), "stdout: {stdout}");
    // Variadic placeholder is shell-quoted per element.
    assert!(
        stdout.contains("shell-sub: 'production' 'myhost' 'v1.2.3' 'a.lua' 'b.lua'"),
        "stdout: {stdout}"
    );
    // Variadic env-var is space-joined.
    assert!(stdout.contains("env: production/myhost/v1.2.3/a.lua b.lua"), "stdout: {stdout}");
    assert!(stdout.contains("exec-lua: production myhost v1.2.3 extras=a.lua,b.lua"), "stdout: {stdout}");
}

/// Regression: a chore that depends on another paramless chore must run
/// the dep's body. Before COOK-36's `a52063d` fix, the non-target chore was
/// silently skipped because the register pass cleared its body to avoid a
/// nil `__cook_params` crash. The post-fix dispatcher only skips the body
/// when the chore actually has params and no argv to bind (caught by
/// `build_chore_params_table`).
#[test]
fn paramless_chore_dependency_body_runs() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore a: b\n    @echo a-runs\nchore b\n    @echo b-runs\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["a"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "cook a failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("a-runs"), "stdout: {stdout}");
    assert!(
        stdout.contains("b-runs"),
        "dependency chore body did not run\nstdout: {stdout}"
    );
}

/// Parametric chore depended on by a recipe runs with defaults bound when
/// argv is unsupplied (spec S.5: parametric dependencies with explicit argv
/// are deferred to COOK-44; today the chore must have defaults for every
/// declared parameter).
#[test]
fn parametric_chore_dependency_runs_with_defaults() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore main: helper\n    @echo main-runs\nchore helper target=\"prod\"\n    @echo helper-target=$<target>\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["main"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "cook main failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("main-runs"), "stdout: {stdout}");
    assert!(
        stdout.contains("helper-target=prod"),
        "dependency chore body did not bind default\nstdout: {stdout}"
    );
}

/// A chore depending on a parametric chore that has a required parameter
/// with no default is a configuration error: the dep cannot supply argv
/// (COOK-44 deferred), so the required param cannot be satisfied.
#[test]
fn parametric_chore_dependency_with_required_no_default_errors() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore main: helper\n    @echo main-runs\nchore helper target\n    @echo helper-target=$<target>\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["main"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected failure on dep with required param + no default"
    );
    assert!(
        stderr.contains("helper") && stderr.contains("target"),
        "diagnostic should name the chore and the unsatisfied parameter\nstderr: {stderr}"
    );
}

/// Lua-expression default that returns a number is coerced via Lua tostring
/// rules to its string form (spec §7.1.2).
#[test]
fn chore_lua_default_number_return_coerces_to_string() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore deploy version=( 42 )\n    > print(\"v=\" .. version)\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["deploy"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}\nstdout: {stdout}");
    assert!(stdout.contains("v=42"), "stdout: {stdout}");
}

/// Lua-expression default that returns a boolean is coerced to its
/// Lua-tostring form ("true" / "false").
#[test]
fn chore_lua_default_boolean_return_coerces_to_string() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore flag enabled=( true )\n    > print(\"enabled=\" .. enabled)\n",
    )
    .unwrap();
    let out = run_cook_raw(tmp.path(), &["flag"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}\nstdout: {stdout}");
    assert!(stdout.contains("enabled=true"), "stdout: {stdout}");
}

/// COOK-61 regression: invoking a chore must not surface a sibling chore's
/// required-no-default param error. Before the fix, every parametric chore in
/// the file was treated as a potential dep of the target and run with empty
/// argv during register-phase, surfacing `ChoreParamMissing` for any required
/// param on any sibling. Per §7.5.1, that rule only applies to actual
/// dep-graph-reachable chores, not arbitrary siblings.
#[test]
fn sibling_chore_required_param_does_not_block_unrelated_target() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore greet who\n    @echo hello $<who>\nchore demo target host=\"prod\"\n    @echo demo $<target> $<host>\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["greet", "alice"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "cook greet alice must not error on unrelated sibling 'demo'\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("hello alice"), "stdout: {stdout}");
    assert!(
        !stderr.contains("requires parameter 'target'"),
        "sibling 'demo' must not surface its required-param error\nstderr: {stderr}"
    );
}

/// COOK-61 regression: the original repro from the fixture. `cook greet alice`
/// must succeed in `cli/e2e-fixtures/chore_param_benchmarks/`-shaped Cookfiles where
/// a sibling chore (`demo`) declares a required param. Stand-in fixture, not
/// the canonical one.
#[test]
fn many_sibling_parametric_chores_do_not_block_target() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        // Mix: targeted chore, required-no-default sibling, defaulted sibling,
        // variadic-plus sibling. None of them are reachable from `greet`.
        "chore greet who\n    @echo hello $<who>\n\
         chore demo target host=\"prod\" version=(\"v0\") *extras\n    @echo demo $<target>\n\
         chore deploy target host=\"prod.example.com\"\n    @echo deploy $<target>\n\
         chore lint +files\n    @echo lint $<files>\n",
    )
    .unwrap();

    let out = run_cook_raw(tmp.path(), &["greet", "alice"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "cook greet alice failed with sibling parametric chores present\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("hello alice"), "stdout: {stdout}");
}

#[test]
fn chore_variadic_star_with_one_argv_binds_single_element_table() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "chore fmt *files\n    > print(\"count=\" .. #files .. \" first=\" .. (files[1] or \"<nil>\"))\n",
    ).unwrap();
    let out = run_cook_raw(tmp.path(), &["fmt", "main.lua"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("count=1"), "stdout: {stdout}");
    assert!(stdout.contains("first=main.lua"), "stdout: {stdout}");
}
