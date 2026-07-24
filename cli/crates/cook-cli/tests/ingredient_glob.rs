use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

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

#[test]
fn absent_ingredient_warns_once_with_raw_pattern_and_recipe() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "Cookfile",
        "recipe build\n    ingredients \"missing/**\"\n    cook \"out/$<in.name>\" { cp $<in> $<out> }\n",
    );

    let output = cook(tmp.path(), &["build"]);
    assert_ok(&output);
    let diagnostic = "cook: warning: ingredient \"missing/**\" matched 0 files (recipe build)";
    assert_eq!(stderr(&output).matches(diagnostic).count(), 1);
}

#[test]
fn member_root_ingredient_is_tracked_by_why() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "root-file", "root\n");
    write(tmp.path(), "Cookfile", "import member ./member\n");
    write(
        tmp.path(),
        "member/Cookfile",
        "recipe build\n    ingredients \"//root-file\"\n    cook \"out.txt\" { cp $<in> $<out> }\n",
    );

    let build = cook(tmp.path(), &["member.build"]);
    assert_ok(&build);
    let why = cook(tmp.path(), &["why", "member.build", "--json"]);
    assert_ok(&why);
    let json: serde_json::Value = serde_json::from_slice(&why.stdout).unwrap();
    let inputs = json["units"][0]["determinants"]["inputs"]
        .as_object()
        .unwrap();
    assert!(inputs.contains_key("../root-file"), "why json: {json}");
    assert!(!stderr(&build).contains("matched 0 files"));
}

#[test]
fn malformed_root_ingredient_fails_loudly() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "Cookfile",
        "recipe build\n    ingredients \"//../escape\"\n    cook \"out.txt\" { true }\n",
    );

    let output = cook(tmp.path(), &["build"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("//.."),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn empty_output_glob_warns_with_recipe_attribution() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "Cookfile",
        r#"recipe assets
        cook.add_unit({
            inputs = {},
            outputs = { "dist/**" },
            command = "true",
        })
"#,
    );

    let output = cook(tmp.path(), &["assets"]);
    assert_ok(&output);
    assert_eq!(
        stderr(&output)
            .matches("cook: warning: output \"dist/**\" matched 0 files (recipe assets)")
            .count(),
        1
    );
}

#[test]
fn quiet_does_not_suppress_empty_output_glob_warning() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "Cookfile",
        r#"recipe assets
        cook.add_unit({ inputs = {}, outputs = { "dist/**" }, command = "true" })
"#,
    );

    let output = cook(tmp.path(), &["--quiet", "assets"]);
    assert_ok(&output);
    assert_eq!(
        stderr(&output)
            .matches("cook: warning: output \"dist/**\" matched 0 files (recipe assets)")
            .count(),
        1
    );
}

#[test]
fn duplicate_output_declarations_each_warn_once() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "Cookfile",
        r#"recipe assets
        cook.add_unit({ inputs = {}, outputs = { "dist/**" }, command = "true", cache_key = "one" })
        cook.add_unit({ inputs = {}, outputs = { "dist/**" }, command = "true", cache_key = "two" })
"#,
    );

    let output = cook(tmp.path(), &["assets"]);
    assert_ok(&output);
    assert_eq!(
        stderr(&output)
            .matches("cook: warning: output \"dist/**\" matched 0 files (recipe assets)")
            .count(),
        2
    );
}

#[test]
fn uncached_output_declaration_warns_when_glob_is_empty() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "Cookfile",
        r#"recipe assets
        cook.add_unit({ cache = false, outputs = { "dist/**" }, command = "true" })
"#,
    );

    let output = cook(tmp.path(), &["assets"]);
    assert_ok(&output);
    assert_eq!(
        stderr(&output)
            .matches("cook: warning: output \"dist/**\" matched 0 files (recipe assets)")
            .count(),
        1
    );
}

#[test]
fn member_relative_ingredient_cannot_escape_member_root() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "outside.txt", "outside\n");
    write(tmp.path(), "Cookfile", "import member ./member\n");
    write(
        tmp.path(),
        "member/Cookfile",
        "recipe build\n    ingredients \"../outside.txt\"\n    cook \"out.txt\" { true }\n",
    );

    let output = cook(tmp.path(), &["member.build"]);
    assert!(!output.status.success());
    let diagnostic = stderr(&output);
    assert!(diagnostic.contains("../outside.txt"), "stderr: {diagnostic}");
    assert!(diagnostic.contains("escape"), "stderr: {diagnostic}");
}

#[test]
fn migration_and_root_lockfile_repro_tracks_every_file_without_warning() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "Cargo.toml", "[workspace]\n");
    write(tmp.path(), "Cargo.lock", "# lock\n");
    write(tmp.path(), "member/migrations/one.sql", "select 1;\n");
    write(
        tmp.path(),
        "member/migrations/nested/two.sql",
        "select 2;\n",
    );
    write(tmp.path(), "Cookfile", "import member ./member\n");
    write(
        tmp.path(),
        "member/Cookfile",
        "recipe build\n    ingredients \"migrations/**\" \"//Cargo.toml\" \"//Cargo.lock\"\n    cook \"stamp\" { touch $<out> }\n",
    );

    let build = cook(tmp.path(), &["member.build"]);
    assert_ok(&build);
    assert!(
        !stderr(&build).contains("matched 0 files"),
        "{}",
        stderr(&build)
    );
    let why = cook(tmp.path(), &["why", "member.build", "--json"]);
    assert_ok(&why);
    let text = String::from_utf8_lossy(&why.stdout);
    for expected in [
        "migrations/one.sql",
        "migrations/nested/two.sql",
        "Cargo.toml",
        "Cargo.lock",
    ] {
        assert!(text.contains(expected), "missing {expected} in {text}");
    }
}
