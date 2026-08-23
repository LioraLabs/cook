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
    run_with_args(root, &[target])
}

fn run_with_args(root: &Path, args: &[&str]) -> Output {
    Command::new(PathBuf::from(env!("CARGO_BIN_EXE_cook")))
        .args(["--output", "plain"])
        .args(args)
        .current_dir(root)
        .env("XDG_CACHE_HOME", root.join(".xdg-cache"))
        .output()
        .unwrap()
}

fn run_raw(root: &Path, args: &[&str]) -> Output {
    Command::new(PathBuf::from(env!("CARGO_BIN_EXE_cook")))
        .args(args)
        .current_dir(root)
        .env("XDG_CACHE_HOME", root.join(".xdg-cache"))
        .output()
        .unwrap()
}

fn shared_cache_config(root: &Path, shared: &Path, publish: bool) {
    write(
        root,
        ".cook/cloud.toml",
        &format!(
            "[cache]\ncache_dir = {:?}\n\n[cloud]\npublish = {publish}\n",
            shared.to_string_lossy()
        ),
    );
}

fn shared_materializer_fixture(root: &Path) {
    write(
        root,
        ".cook/modules/share/lua/5.4/fixture.lua",
        r#"
local M = {}
function M.install()
  local graph = cook.materialize("graph", {}, function()
    if fs.exists("producer-ran") then error("producer must not run") end
    fs.write("producer-ran", "ran")
    return { target = "generated", value = "shared" }
  end)
  cook.recipe(graph.target, {}, function()
    cook.add_unit({ output = "result.txt", command = "printf '" .. graph.value .. "' > result.txt" })
  end)
end
return M
"#,
    );
    write(root, "Cookfile", "use fixture\n\nfixture.install()\n");
}

fn regular_file_count(root: &Path) -> usize {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .count()
}

#[test]
fn materialized_values_restore_across_clients_and_respect_no_publish() {
    let shared = TempDir::new().unwrap();
    let publisher = TempDir::new().unwrap();
    shared_cache_config(publisher.path(), shared.path(), true);
    shared_materializer_fixture(publisher.path());

    let first = run(publisher.path(), "generated");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(publisher.path().join("result.txt")).unwrap(),
        "shared"
    );

    let consumer = TempDir::new().unwrap();
    shared_cache_config(consumer.path(), shared.path(), true);
    shared_materializer_fixture(consumer.path());
    write(consumer.path(), "producer-ran", "fail if executed");
    let restored = run_with_args(consumer.path(), &["--no-publish", "generated"]);
    assert!(
        restored.status.success(),
        "cold client ran producer instead of restoring: {}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(consumer.path().join("result.txt")).unwrap(),
        "shared"
    );

    let no_publish_store = TempDir::new().unwrap();
    let no_publish_client = TempDir::new().unwrap();
    shared_cache_config(no_publish_client.path(), no_publish_store.path(), true);
    shared_materializer_fixture(no_publish_client.path());
    let module = no_publish_client
        .path()
        .join(".cook/modules/share/lua/5.4/fixture.lua");
    let source = std::fs::read_to_string(&module).unwrap().replace(
        "if fs.exists(\"producer-ran\") then error(\"producer must not run\") end",
        "",
    );
    std::fs::write(module, source).unwrap();
    let before = regular_file_count(no_publish_store.path());
    let output = run_with_args(no_publish_client.path(), &["--no-publish", "generated"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(regular_file_count(no_publish_store.path()), before);
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
fn imported_materializer_invalidates_on_workspace_root_input() {
    let root = TempDir::new().unwrap();
    write(root.path(), "seed.txt", "one");
    write(
        root.path(),
        "Cookfile",
        "import api ./api\n\nrecipe build: api.generated\n",
    );
    write(
        root.path(),
        "api/Cookfile",
        "cook.materialize(\"graph\", { files = { \"//seed.txt\" } }, function() local runs = fs.exists(\"runs\") and tonumber(fs.read(\"runs\")) or 0; fs.write(\"runs\", tostring(runs + 1)); return true end)\nrecipe generated\n",
    );

    let first = run(root.path(), "build");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(run(root.path(), "build").status.success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("api/runs")).unwrap(),
        "1"
    );

    write(root.path(), "seed.txt", "two");
    let changed = run(root.path(), "build");
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("api/runs")).unwrap(),
        "2"
    );
}

#[test]
fn plain_output_reports_qualified_materializer_runs_and_restores() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        ".cook/modules/share/lua/5.4/fixture.lua",
        r#"
local M = {}
function M.install()
  cook.materialize("graph", {}, function() return { target = "build" } end)
end
return M
"#,
    );
    write(
        root.path(),
        "Cookfile",
        "use fixture\nfixture.install()\nrecipe build\n",
    );

    let first = run(root.path(), "build");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        String::from_utf8_lossy(&first.stderr).contains("cook materialize fixture.graph ran"),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let warm = run(root.path(), "build");
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(
        String::from_utf8_lossy(&warm.stderr).contains("cook materialize fixture.graph restored"),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
}

#[test]
fn auto_output_reports_materialization_without_touching_stdout() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        "Cookfile",
        "cook.materialize(\"graph\", {}, function() return true end)\nrecipe build\n",
    );
    let output = run_raw(root.path(), &["build"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cook materialize graph ran"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("cook materialize"),
        "materialization reporting must not corrupt stdout"
    );
}

#[test]
fn json_output_keeps_materialization_on_the_jsonl_wire() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        "Cookfile",
        "cook.materialize(\"graph\", {}, function() return true end)\nrecipe build\n",
    );
    for outcome in ["ran", "restored"] {
        let output = run_raw(root.path(), &["--output", "json", "build"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let events: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stderr)
            .lines()
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("invalid JSONL `{line}`: {error}"))
            })
            .collect();
        assert!(
            events.iter().any(|event| event["type"] == "materialized"
                && event["qualified_key"] == "graph"
                && event["outcome"] == outcome
                && event["v"].is_number()
                && event["ts"].is_string()),
            "{events:#?}"
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("cook materialize"),
            "materialization reporting must not corrupt stdout"
        );
    }
}

#[test]
fn materializer_determinant_failures_name_key_and_declaration_site() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        ".cook/modules/share/lua/5.4/fixture.lua",
        r#"
local M = {}
function M.install()
  cook.materialize("graph", { tools = { "definitely-not-a-cook-tool" } }, function() return true end)
end
return M
"#,
    );
    write(
        root.path(),
        "Cookfile",
        "use fixture\nfixture.install()\nrecipe build\n",
    );

    let output = run(root.path(), "build");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cook.materialize graph (fixture.graph) declared at"),
        "{stderr}"
    );
    assert!(stderr.contains("fixture.lua:4"), "{stderr}");
    assert!(
        stderr.contains("tool \"definitely-not-a-cook-tool\" was not found"),
        "{stderr}"
    );
}

#[test]
fn root_materializer_diagnostic_uses_its_actual_cookfile_line() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        "Cookfile",
        "config\n    var.COOK_SOURCE_MAP_TEST = \"ok\"\n\nrecipe build\ncook.materialize(\"graph\", { tools = { \"definitely-not-a-cook-tool\" } }, function() return true end)\n",
    );

    let output = run(root.path(), "build");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Cookfile:5"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn register_materializer_diagnostic_uses_its_actual_cookfile_line() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        "Cookfile",
        "recipe build\nregister\n    local graph = cook.materialize(\"graph\", { tools = { \"definitely-not-a-cook-tool\" } }, function() return true end)\n",
    );

    let output = run(root.path(), "build");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Cookfile:3"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn materializer_env_and_seal_failures_name_key_and_declaration_site() {
    for (spec, cause) in [
        (
            "env = { \"MISSING\" }",
            "var.MISSING: no config block declares 'MISSING'",
        ),
        (
            "seals = { \"missing\" }",
            "seal \"missing\": runtime error: cook.probes.get called outside of a module context",
        ),
    ] {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "Cookfile",
            &format!(
                "recipe build\ncook.materialize(\"graph\", {{ {spec} }}, function() return true end)\n"
            ),
        );

        let output = run(root.path(), "build");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cook.materialize graph (graph) declared at")
                && stderr.contains("Cookfile:2"),
            "{stderr}"
        );
        assert!(stderr.contains(cause), "{stderr}");
    }
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
fn independent_workspace_materializers_overlap() {
    // Exercise fresh coordinators repeatedly so result handoff between concurrent
    // registration threads cannot strand a waiter.
    for _ in 0..8 {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "Cookfile",
            "import alpha ./alpha\nimport beta ./beta\nrecipe build\n",
        );
        for (member, other) in [("alpha", "beta"), ("beta", "alpha")] {
            let marker = root.path().join(format!("{member}.started"));
            let other_marker = root.path().join(format!("{other}.started"));
            write(
                root.path(),
                &format!("{member}/.cook/modules/share/lua/5.4/fixture.lua"),
                &format!(
                    r#"
local M = {{}}
function M.install()
  cook.materialize("graph", {{}}, function()
    fs.write("{}", "yes")
    for _ = 1, 1000000 do
      if fs.exists("{}") then return "{member}" end
    end
    error("materializers ran sequentially")
  end)
end
return M
"#,
                    marker.display(),
                    other_marker.display(),
                ),
            );
            write(
                root.path(),
                &format!("{member}/Cookfile"),
                "use fixture\n\nfixture.install()\nrecipe build\n",
            );
        }

        let output = run(root.path(), "build");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn multi_member_no_publish_reuses_prepass_materializations() {
    let root = TempDir::new().unwrap();
    shared_cache_config(root.path(), &root.path().join("shared"), true);
    write(
        root.path(),
        "Cookfile",
        "import member ./member\nrecipe build\n",
    );
    write(
        root.path(),
        "member/Cookfile",
        "use fixture\n\nfixture.install()\n",
    );
    write(
        root.path(),
        "member/.cook/modules/share/lua/5.4/fixture.lua",
        r#"local M = {}
function M.install()
  local graph = cook.materialize("graph", {}, function()
  if fs.exists("producer-ran") then error("producer reran") end
  fs.write("producer-ran", "yes")
  return { target = "generated" }
  end)
  cook.recipe(graph.target, {}, function() end)
end
return M
"#,
    );

    let output = run_with_args(root.path(), &["--no-publish", "build"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn module_identity_is_the_caller_and_repeated_declarations_are_single_flight() {
    let root = TempDir::new().unwrap();
    for module in ["alpha", "beta"] {
        write(
            root.path(),
            &format!(".cook/modules/share/lua/5.4/{module}.lua"),
            &format!(
                r#"
local M = {{}}
local function produce()
  local path = "{module}.runs"
  local n = fs.exists(path) and tonumber(fs.read(path)) or 0
  fs.write(path, tostring(n + 1))
  return "{module}"
end
function M.install()
  local first = cook.materialize("graph", {{}}, produce)
  local second = cook.materialize("graph", {{}}, produce)
  assert(first == second)
end
return M
"#
            ),
        );
    }
    write(
        root.path(),
        "Cookfile",
        r#"use alpha
use beta

alpha.install()
beta.install()
cook.recipe("build", {}, function() cook.add_unit({ output = "done", command = "printf done > done" }) end)
"#,
    );

    let output = run(root.path(), "build");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("alpha.runs")).unwrap(),
        "1"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("beta.runs")).unwrap(),
        "1"
    );
}

#[test]
fn module_identity_follows_functions_defined_in_required_subfiles() {
    let root = TempDir::new().unwrap();
    for module in ["alpha", "beta"] {
        write(
            root.path(),
            &format!(".cook/modules/share/lua/5.4/{module}/init.lua"),
            &format!("return require('{module}.impl')\n"),
        );
        write(
            root.path(),
            &format!(".cook/modules/share/lua/5.4/{module}/impl.lua"),
            &format!(
                r#"
local M = {{}}
function M.install()
  local value = cook.materialize("graph", {{}}, function() return "{module}" end)
  assert(value == "{module}")
end
return M
"#
            ),
        );
    }
    write(
        root.path(),
        "Cookfile",
        "use alpha\nuse beta\n\nalpha.install()\nbeta.install()\nrecipe build\n",
    );

    let output = run(root.path(), "build");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn path_module_identity_follows_functions_defined_in_required_subfiles() {
    let root = TempDir::new().unwrap();
    for module in ["alpha", "beta"] {
        write(
            root.path(),
            &format!("mods/{module}.lua"),
            &format!("return require('mods.{module}_impl')\n"),
        );
        write(
            root.path(),
            &format!("mods/{module}_impl.lua"),
            &format!(
                r#"
local M = {{}}
function M.install()
  local value = cook.materialize("graph", {{}}, function() return "{module}" end)
  assert(value == "{module}")
end
return M
"#
            ),
        );
    }
    write(
        root.path(),
        "Cookfile",
        "use ./mods/alpha.lua\nuse ./mods/beta.lua\n\nalpha.install()\nbeta.install()\nrecipe build\n",
    );

    let output = run(root.path(), "build");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn nested_path_module_load_restores_required_source_owner() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        "mods/alpha.lua",
        "local beta = cook.load_module('./mods/beta.lua')\nlocal alpha = require('mods.alpha_impl')\nreturn { install = function() alpha.install(); beta.install() end }\n",
    );
    write(
        root.path(),
        "mods/beta.lua",
        "return require('mods.beta_impl')\n",
    );
    for module in ["alpha", "beta"] {
        write(
            root.path(),
            &format!("mods/{module}_impl.lua"),
            &format!(
                "local M = {{}}\nfunction M.install() assert(cook.materialize('graph', {{}}, function() return '{module}' end) == '{module}') end\nreturn M\n"
            ),
        );
    }
    write(
        root.path(),
        "Cookfile",
        "use ./mods/alpha.lua\n\nalpha.install()\nrecipe build\n",
    );

    let output = run(root.path(), "build");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn conflicting_repeated_declaration_is_rejected() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        ".cook/modules/share/lua/5.4/fixture.lua",
        r#"
local M = {}
local function one() return 1 end
local function two() return 2 end
function M.install()
cook.materialize("graph", {}, one)
cook.materialize("graph", {}, two)
end
return M
"#,
    );
    write(
        root.path(),
        "Cookfile",
        "use fixture\n\nfixture.install()\nrecipe build\n",
    );
    let output = run(root.path(), "build");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conflicting declaration"), "{stderr}");
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
