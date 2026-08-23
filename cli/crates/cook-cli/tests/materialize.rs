use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn run(root: &Path, target: &str) -> Output {
    Command::new(PathBuf::from(env!("CARGO_BIN_EXE_cook")))
        .args(["--output", "plain", target])
        .current_dir(root)
        .env("XDG_CACHE_HOME", root.join(".xdg-cache"))
        .output()
        .unwrap()
}

#[test]
fn registration_value_is_cached_invalidated_and_separate_from_targets() {
    let root = TempDir::new().unwrap();
    write(root.path(), "seed.txt", "first");
    write(
        root.path(),
        ".cook/modules/share/lua/5.4/fixture.lua",
        r#"
local M = {}
local settings = { target = "generated", count = 3 }
local function discover(seed)
    return { target = settings.target, nested = { ok = true, count = settings.count }, seed = seed }
end
function M.install()
local graph = cook.materialize("graph", { files = { "seed.txt" } }, function()
    assert(cook.materialize == nil, "producer ran in registration VM")
    local count = 0
    if fs.exists("executions.txt") then count = tonumber(fs.read("executions.txt")) end
    fs.write("executions.txt", tostring(count + 1))
    return discover(fs.read("seed.txt"))
end)

cook.recipe(graph.target, {}, function()
    cook.add_unit({ output = "result.txt", command = "printf '" .. graph.seed .. "' > result.txt" })
end)
cook.recipe("graph", {}, function() cook.add_unit({ output = "collision.txt", command = "printf collision > collision.txt" }) end)
cook.recipe("materialize", {}, function() cook.add_unit({ output = "named.txt", command = "printf named > named.txt" }) end)
end
return M
"#,
    );
    write(
        root.path(),
        "Cookfile",
        "use fixture\n\nfixture.install()\n",
    );

    let first = run(root.path(), "generated");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("executions.txt")).unwrap(),
        "1"
    );

    assert!(run(root.path(), "menu").status.success());
    assert!(run(root.path(), "list").status.success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("executions.txt")).unwrap(),
        "1",
        "warm menu/listing must share the materializer cache"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("result.txt")).unwrap(),
        "first"
    );

    std::fs::remove_file(root.path().join("result.txt")).unwrap();
    let warm = run(root.path(), "generated");
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("executions.txt")).unwrap(),
        "1"
    );

    write(root.path(), "unrelated.txt", "ignored");
    assert!(run(root.path(), "generated").status.success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("executions.txt")).unwrap(),
        "1"
    );

    write(root.path(), "seed.txt", "second");
    assert!(run(root.path(), "generated").status.success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("executions.txt")).unwrap(),
        "2"
    );

    let module = root.path().join(".cook/modules/share/lua/5.4/fixture.lua");
    let changed = std::fs::read_to_string(&module)
        .unwrap()
        .replace("count = 3", "count = 4");
    std::fs::write(&module, changed).unwrap();
    assert!(run(root.path(), "generated").status.success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("executions.txt")).unwrap(),
        "3",
        "captured serializable values are cache determinants"
    );
    let changed = std::fs::read_to_string(&module)
        .unwrap()
        .replace("seed = seed }", "seed = seed .. \"!\" }");
    std::fs::write(module, changed).unwrap();
    assert!(run(root.path(), "generated").status.success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("executions.txt")).unwrap(),
        "4",
        "captured helper bodies are cache determinants"
    );
    assert!(run(root.path(), "graph").status.success());
    assert!(run(root.path(), "materialize").status.success());
}

#[test]
fn materializer_key_is_not_a_target_and_invalid_values_fail_clearly() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        ".cook/modules/share/lua/5.4/fixture.lua",
        r#"
local M = {}
function M.install()
cook.materialize("private", {}, function() return { bad = function() end } end)
cook.recipe("build", {}, function() cook.add_unit({ output = "out", command = "touch out" }) end)
end
return M
"#,
    );
    write(
        root.path(),
        "Cookfile",
        "use fixture\n\nfixture.install()\n",
    );
    let invalid = run(root.path(), "build");
    assert!(!invalid.status.success());
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(stderr.contains("cook.materialize private"), "{stderr}");
    assert!(stderr.contains("non-serialisable"), "{stderr}");

    write(
        root.path(),
        ".cook/modules/share/lua/5.4/fixture.lua",
        "local M = {}\nfunction M.install() cook.materialize(\"private\", {}, function() return true end) end\nreturn M\n",
    );
    let private = run(root.path(), "private");
    assert!(!private.status.success());
    assert!(String::from_utf8_lossy(&private.stderr).contains("private"));
}

#[test]
fn dependency_chains_are_rejected() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        ".cook/modules/share/lua/5.4/fixture.lua",
        "local M = {}\nfunction M.install() cook.materialize(\"a\", { requires = { \"b\" } }, function() return true end) end\nreturn M\n",
    );
    write(
        root.path(),
        "Cookfile",
        "use fixture\n\nfixture.install()\n",
    );
    let output = run(root.path(), "build");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot depend"));
}

#[test]
fn aliased_mutable_captures_are_rejected() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        ".cook/modules/share/lua/5.4/fixture.lua",
        r#"
local M = {}
local state = { n = 0 }
local function increment() state.n = state.n + 1 end
function M.install()
  cook.materialize("graph", {}, function()
    increment()
    state.n = state.n + 1
    return state.n
  end)
end
return M
"#,
    );
    write(
        root.path(),
        "Cookfile",
        "use fixture\n\nfixture.install()\n",
    );

    let output = run(root.path(), "build");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cook.materialize"), "{stderr}");
    assert!(
        stderr.contains("shared/aliased mutable capture"),
        "{stderr}"
    );
}
