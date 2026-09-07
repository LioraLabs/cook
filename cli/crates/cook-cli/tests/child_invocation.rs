use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn binary() -> String {
    std::env::var("COOK_CHILD_TEST_BINARY").unwrap_or_else(|_| env!("CARGO_BIN_EXE_cook").into())
}
fn write(root: &Path, name: &str, content: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
fn run(binary: &str, root: &Path, args: &[&str]) -> Output {
    Command::new(binary)
        .args(args)
        .current_dir(root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("wrong-path").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("XDG_CACHE_HOME", root.join("cache-home"))
        .output()
        .unwrap()
}
fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn files(path: &Path) -> usize {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| {
            if entry.path().is_dir() {
                files(&entry.path())
            } else {
                1
            }
        })
        .sum()
}
const DRIVER: &str = r#"
local function quote(value) return "'" .. value:gsub("'", "'\\''") .. "'" end
cook.recipe("selected", {}, function()
    cook.add_unit({outputs={"selected.txt"}, command="test \"$(cat ready)\" = ready || exit 93; printf '%s\\n' " .. quote(var.VALUE) .. " " .. quote(var.EMPTY) .. " " .. quote(var.MEMBER) .. " > selected.txt; printf run >> executions"})
end)
cook.recipe("unrelated", {}, function()
    cook.add_unit({command="touch unrelated-executed; exit 91", cache=false})
end)
cook.recipe("failure", {}, function() cook.add_unit({command="exit 37",cache=false}) end)
cook.recipe("prepare", {}, function() cook.add_unit({outputs={"ready"}, command="printf ready > ready"}) end)
cook.chore("driver.stage", {requires={"prepare"}}, function()
    local command = cook.child_command("selected")
    cook.add_unit({command="printf %s " .. quote(command) .. " > child-command.txt", cache=false})
    cook.add_unit({command=command .. " && pwd > after-directory.txt", cache=false, interactive=true})
end)
cook.chore("driver.fail", {}, function()
    cook.add_unit({command=cook.child_command("failure"), cache=false, interactive=true})
end)
return {}
"#;

#[test]
fn faithful_child_preserves_source_binary_preset_overrides_and_publish_policy() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write(root, "driver.lua", DRIVER);
    write(root, "Custom ' entry", "use driver ./driver.lua\nconfig\n    var.VALUE = \"default\"\n    var.EMPTY = \"default\"\n    var.MEMBER = \"base\"\nconfig release\n    var.MEMBER = \"release\"\n");
    write(root, "Cookfile", "chore wrong\n    exit 92\n");
    write(
        root,
        "wrong-path/cook",
        "#!/bin/sh\ntouch wrong-binary; exit 97\n",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            root.join("wrong-path/cook"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let shared = root.join("shared");
    write(
        root,
        ".cook/cloud.toml",
        &format!(
            "[cache]\ncache_dir = {:?}\n[cloud]\npublish = true\n",
            shared.to_str().unwrap()
        ),
    );
    let value = "VALUE=space ' quote $(touch injected); *";
    success(run(
        &binary(),
        root,
        &[
            "--file",
            "Custom ' entry",
            "--no-publish",
            "--no-prune",
            "--no-auto-gc",
            "--jobs",
            "2",
            "--color",
            "never",
            "--set",
            value,
            "--set",
            "EMPTY=",
            "driver.stage",
            "@release",
        ],
    ));
    assert_eq!(
        fs::read_to_string(root.join("selected.txt")).unwrap(),
        "space ' quote $(touch injected); *\n\nrelease\n"
    );
    assert_eq!(files(&shared), 0, "child published despite --no-publish");
    let executions = fs::read(root.join("executions")).unwrap();
    success(run(
        &binary(),
        root,
        &[
            "--file",
            "Custom ' entry",
            "--no-publish",
            "--no-prune",
            "--no-auto-gc",
            "--jobs",
            "2",
            "--color",
            "never",
            "--set",
            value,
            "--set",
            "EMPTY=",
            "driver.stage",
            "@release",
        ],
    ));
    assert_eq!(
        fs::read(root.join("executions")).unwrap(),
        executions,
        "child cached target executed twice"
    );
    assert!(
        root.join("ready").is_file(),
        "parent cached prerequisite did not execute"
    );

    let first = fs::read_to_string(root.join("child-command.txt")).unwrap();
    for flag in [
        "--no-publish",
        "--no-prune",
        "--no-auto-gc",
        "--jobs",
        "@release",
    ] {
        assert!(first.contains(flag), "{first}");
    }
    for sentinel in ["wrong-binary", "injected", "unrelated-executed"] {
        assert!(!root.join(sentinel).exists());
    }
    let alternate = root.join("alternate cook ' binary");
    fs::copy(binary(), &alternate).unwrap();
    success(run(
        alternate.to_str().unwrap(),
        root,
        &[
            "--file",
            "Custom ' entry",
            "--set",
            "VALUE=changed",
            "--set",
            "EMPTY=",
            "driver.stage",
            "@release",
        ],
    ));
    assert!(files(&shared) > 0, "publish policy did not refresh");
    let second = fs::read_to_string(root.join("child-command.txt")).unwrap();
    assert!(!second.contains("--no-publish"));
    assert!(second.contains("alternate cook"));
    let failed = run(
        &binary(),
        root,
        &["--file", "Custom ' entry", "driver.fail"],
    );
    assert_eq!(failed.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("37"));
}

#[test]
fn imported_chore_resolves_member_target_without_leaking_member_configuration() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write(root, "driver.lua", DRIVER);
    write(root, "Cookfile", "import sub ./sub\nconfig\n    var.VALUE = \"root\"\n    var.EMPTY = \"\"\n    var.MEMBER = \"root\"\n");
    write(root, "sub/driver.lua", DRIVER);
    write(root, "sub/Cookfile", "use driver ./driver.lua\nconfig\n    var.VALUE = \"member\"\n    var.EMPTY = \"\"\n    var.MEMBER = \"member-base\"\nconfig release\n    var.MEMBER = \"member-release\"\n");
    success(run(&binary(), root, &["sub.driver.stage", "@release"]));
    assert_eq!(
        fs::read_to_string(root.join("sub/selected.txt")).unwrap(),
        "member\n\nmember-release\n"
    );
    assert!(!root.join("selected.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("sub/after-directory.txt"))
            .unwrap()
            .trim(),
        root.join("sub").to_str().unwrap()
    );
    assert!(fs::read_to_string(root.join("sub/child-command.txt"))
        .unwrap()
        .contains("+sub.selected"));
}
