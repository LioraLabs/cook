//! A root-wildcard output declaration (`**`, `*`, `*.env`, `.*`) is warned
//! about once per declaration, on every command that registers the recipe.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const MARKER: &str = "is a wildcard at the project root";

fn cook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn cook(root: &Path, args: &[&str]) -> Output {
    Command::new(cook_bin())
        .args(args)
        .current_dir(root)
        .output()
        .expect("run cook")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        stderr(output)
    );
}

fn line(pattern: &str, recipe: &str) -> String {
    format!(
        "cook: warning: output \"{pattern}\" (recipe {recipe}) is a wildcard at the project root: it can match files there such as the Cookfile, dotfiles (.env, .envrc), CI and hook files, which a shared-cache restore may then write; narrow it to a directory, e.g. \"dist/**\""
    )
}

fn project(cookfile: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "Cookfile", cookfile);
    tmp
}

#[test]
fn root_wildcard_outputs_warn_once_each() {
    for pattern in ["**", "*", "*.env", ".*"] {
        let tmp = project(&format!(
            "recipe build\n    cook \"{pattern}\" {{ touch a.env }}\n"
        ));
        let output = cook(tmp.path(), &["build"]);
        assert_ok(&output);
        assert_eq!(
            stderr(&output).matches(&line(pattern, "build")).count(),
            1,
            "{pattern}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn narrow_and_literal_outputs_do_not_warn() {
    for pattern in ["dist/**", "out/*.o", "out.txt"] {
        let tmp = project(&format!(
            "recipe build\n    cook \"{pattern}\" {{ mkdir -p dist out && touch dist/a out/a.o out.txt }}\n"
        ));
        let output = cook(tmp.path(), &["build"]);
        assert_ok(&output);
        assert!(!stderr(&output).contains(MARKER), "{pattern}: {}", stderr(&output));
    }
}

/// A gather-driven step lowers one declaration into one unit per member.
#[test]
fn static_fan_out_declaration_warns_once() {
    let tmp = project("recipe build\n    gather \"src/*.c\"\n    cook \"*.o\" { cat $<in> >/dev/null; touch x.o }\n");
    write(tmp.path(), "src/a.c", "a");
    write(tmp.path(), "src/b.c", "b");
    let output = cook(tmp.path(), &["build"]);
    assert_ok(&output);
    assert_eq!(stderr(&output).matches(&line("*.o", "build")).count(), 1, "{}", stderr(&output));
    assert_eq!(stderr(&output).matches(MARKER).count(), 1, "{}", stderr(&output));
}

/// The pattern expands differently per member; still one declaration.
#[test]
fn templated_fan_out_declaration_warns_once() {
    let tmp = project("recipe build\n    gather \"src/*.c\"\n    cook \"$<in.stem>*.o\" { touch x.o }\n");
    write(tmp.path(), "src/a.c", "a");
    write(tmp.path(), "src/b.c", "b");
    let output = cook(tmp.path(), &["build"]);
    assert_ok(&output);
    assert_eq!(stderr(&output).matches(MARKER).count(), 1, "{}", stderr(&output));
}

#[test]
fn same_pattern_in_two_steps_and_multi_output_warn_per_declaration() {
    let tmp = project("recipe build\n    cook \"*.o\" { touch a.o }\n    cook \"*.o\" { touch b.o }\n    cook \"a*\" \"b*\" { touch a1 b1 }\n");
    let output = cook(tmp.path(), &["build"]);
    assert_ok(&output);
    assert_eq!(stderr(&output).matches(MARKER).count(), 4, "{}", stderr(&output));
}

#[test]
fn distinct_declarations_warn_separately() {
    let tmp = project(
        "recipe one\n    cook \"*.env\" { touch a.env }\n\nrecipe two\n    cook \"**\" { touch b }\n\nrecipe both : one two\n",
    );
    let output = cook(tmp.path(), &["both"]);
    assert_ok(&output);
    let err = stderr(&output);
    assert_eq!(err.matches(MARKER).count(), 2, "{err}");
    assert_eq!(err.matches(&line("*.env", "one")).count(), 1, "{err}");
    assert_eq!(err.matches(&line("**", "two")).count(), 1, "{err}");
}

#[test]
fn workspace_member_names_qualified_recipe() {
    let tmp = project("import member ./member\n");
    write(tmp.path(), "member/Cookfile", "recipe build\n    cook \"*\" { touch a }\n");
    let output = cook(tmp.path(), &["member.build"]);
    assert_ok(&output);
    assert_eq!(
        stderr(&output).matches(&line("*", "member.build")).count(),
        1,
        "{}",
        stderr(&output)
    );
}

#[test]
fn module_minted_unit_warns() {
    let tmp = project("use toolmod\n\ntoolmod.tool(\"gen\")\n");
    let rel = format!(
        "{}/{}/{}/toolmod.lua",
        cook_contracts::layout::DOT_COOK_DIR,
        cook_contracts::layout::MODULES_SUBDIR,
        cook_contracts::layout::MODULES_SHARE_LUA_SUBDIR,
    );
    write(
        tmp.path(),
        &rel,
        "local M = {}\nfunction M.tool(name)\n    cook.recipe(name, {}, function()\n        cook.add_unit({ inputs = {}, outputs = { \"*\" }, command = \"touch a\" })\n    end)\nend\nreturn M\n",
    );
    let output = cook(tmp.path(), &["gen"]);
    assert_ok(&output);
    assert_eq!(stderr(&output).matches(&line("*", "gen")).count(), 1, "{}", stderr(&output));
}

#[test]
fn why_warns_once_and_keeps_stdout_json() {
    let tmp = project("recipe build\n    cook \"*.env\" { touch a.env }\n");
    assert_ok(&cook(tmp.path(), &["build"]));
    let why = cook(tmp.path(), &["why", "build", "--format", "json"]);
    assert_ok(&why);
    serde_json::from_slice::<serde_json::Value>(&why.stdout).unwrap();
    assert_eq!(stderr(&why).matches(&line("*.env", "build")).count(), 1, "{}", stderr(&why));
}

/// One step group can hold distinct declarations; each is warned.
#[test]
fn step_group_with_distinct_declarations_warns_for_each() {
    let tmp = project("use toolmod\n\ntoolmod.tool(\"gen\")\n");
    let rel = format!(
        "{}/{}/{}/toolmod.lua",
        cook_contracts::layout::DOT_COOK_DIR,
        cook_contracts::layout::MODULES_SUBDIR,
        cook_contracts::layout::MODULES_SHARE_LUA_SUBDIR,
    );
    write(
        tmp.path(),
        &rel,
        "local M = {}\nfunction M.tool(name)\n    cook.recipe(name, {}, function()\n        cook.step_group(function()\n            cook.add_unit({ inputs = {}, outputs = { \"a*\" }, command = \"touch a1\" })\n            cook.add_unit({ inputs = {}, outputs = { \"b*\" }, command = \"touch b1\" })\n        end)\n    end)\nend\nreturn M\n",
    );
    let output = cook(tmp.path(), &["gen"]);
    assert_ok(&output);
    let err = stderr(&output);
    assert_eq!(err.matches(MARKER).count(), 2, "{err}");
    assert_eq!(err.matches(&line("a*", "gen")).count(), 1, "{err}");
    assert_eq!(err.matches(&line("b*", "gen")).count(), 1, "{err}");
}

/// A `toolmod.tool("gen")` project whose module body (inside one step group) is `group`.
fn step_group_module(prelude: &str, group: &str) -> TempDir {
    let tmp = project("use toolmod\n\ntoolmod.tool(\"gen\")\n");
    let rel = format!(
        "{}/{}/{}/toolmod.lua",
        cook_contracts::layout::DOT_COOK_DIR,
        cook_contracts::layout::MODULES_SUBDIR,
        cook_contracts::layout::MODULES_SHARE_LUA_SUBDIR,
    );
    write(
        tmp.path(),
        &rel,
        &format!("local M = {{}}\n{prelude}\nfunction M.tool(name)\n    cook.recipe(name, {{}}, function()\n        cook.step_group(function()\n{group}\n        end)\n    end)\nend\nreturn M\n"),
    );
    tmp
}

fn assert_two_warnings(tmp: &TempDir, a: &str, b: &str) {
    let output = cook(tmp.path(), &["gen"]);
    assert_ok(&output);
    let err = stderr(&output);
    assert_eq!(err.matches(MARKER).count(), 2, "{err}");
    assert_eq!(err.matches(&line(a, "gen")).count(), 1, "{err}");
    assert_eq!(err.matches(&line(b, "gen")).count(), 1, "{err}");
}

/// A shared helper called twice inside one step group: each call is a declaration.
#[test]
fn helper_called_twice_in_step_group_warns_for_each() {
    let tmp = step_group_module(
        "function M.obj(pat)\n    cook.add_unit({ inputs = {}, outputs = { pat }, command = \"true\" })\nend",
        "            M.obj(\"a*\")\n            M.obj(\"b*\")",
    );
    assert_two_warnings(&tmp, "a*", "b*");
}

/// Two pcall(cook.add_unit, ...) on different lines share the C frame; the caller line differs.
#[test]
fn pcall_add_unit_twice_in_step_group_warns_for_each() {
    let tmp = step_group_module(
        "",
        "            pcall(cook.add_unit, { inputs = {}, outputs = { \"c*\" }, command = \"true\" })\n            pcall(cook.add_unit, { inputs = {}, outputs = { \"d*\" }, command = \"true\" })",
    );
    assert_two_warnings(&tmp, "c*", "d*");
}
