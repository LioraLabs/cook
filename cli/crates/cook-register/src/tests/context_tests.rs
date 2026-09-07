use super::*;
use tempfile::TempDir;

/// CS-0064: `recipe.inputs` / `cook.resolve_gather` MUST
/// drop sub-directory matches, so a tree with a file and a sibling
/// directory both matched by `*` yields only the file.
#[test]
fn resolve_glob_filters_directories() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("nested")).unwrap();

    let got = cook_cache::resolve_gather_glob(dir.path(), dir.path(), "*").unwrap();
    let expected: BTreeSet<String> = ["a.txt".to_string()].into_iter().collect();
    assert_eq!(got, expected);
}

#[test]
fn excludes_match_lexically_equivalent_include_paths() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("dir")).unwrap();
        std::fs::write(dir.path().join("file"), "").unwrap();
    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    register_resolve_gather(&lua, dir.path(), dir.path()).unwrap();

    for expression in [
        r#"return cook.resolve_gather({"dir/../file"}, {"file"})"#,
        r#"return cook.resolve_gather({"file"}, {"dir/../file"})"#,
    ] {
        let files: LuaTable = lua.load(expression).eval().unwrap();
        assert_eq!(files.raw_len(), 0, "{expression}");
    }
}

#[test]
#[cfg(target_os = "linux")]
fn exclusions_do_not_traverse_unrelated_trees() {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    unsafe extern "C" {
        fn inotify_init1(flags: i32) -> i32;
        fn inotify_add_watch(fd: i32, path: *const std::ffi::c_char, mask: u32) -> i32;
    }
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("input.txt"), "input").unwrap();
    let unrelated = dir.path().join("node_modules");
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(unrelated.join("package.json"), "{}").unwrap();
    // Observe directory opens without wall-time thresholds or global counters.
    // Linux O_NONBLOCK and IN_OPEN; no watcher thread or optional external tool.
    let fd = unsafe { inotify_init1(0x800) };
    assert!(fd >= 0);
    let mut watcher = unsafe { std::fs::File::from_raw_fd(fd) };
    let path = std::ffi::CString::new(unrelated.as_os_str().as_encoded_bytes()).unwrap();
    assert!(unsafe { inotify_add_watch(fd, path.as_ptr(), 0x20) } >= 0);
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    register_resolve_gather(&lua, dir.path(), dir.path()).unwrap();
    let files: Vec<String> = lua.load(r#"return cook.resolve_gather({"input.txt"}, {"**/node_modules/**"})"#).eval().unwrap();
    assert_eq!(files, ["input.txt"]);
    let recipe = gathered_recipe(&lua, &["input.txt"], &["**/node_modules/**"]);
    setup_recipe_context(&lua, &recipe, dir.path(), dir.path(), &Rc::new(RefCell::new(vec![]))).unwrap();
    let result = watcher.read(&mut [0u8; 4096]);
    assert!(matches!(result, Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "matching excludes opened an unrelated tree: {result:?}");
}

fn gathered_recipe(lua: &Lua, inputs: &[&str], excludes: &[&str]) -> RegisteredRecipe {
    RegisteredRecipe {
        name: "gathered".into(),
        function: lua.create_registry_value(lua.create_function(|_, ()| Ok(())).unwrap()).unwrap(),
        metadata: crate::capture::RegisteredMetadata {
            inputs: inputs.iter().map(|s| (*s).into()).collect(),
            excludes: excludes.iter().map(|s| (*s).into()).collect(),
            requires: vec![], params: vec![], description: None, origin: None,
        },
        source: crate::capture::RegistrationSource::Static { line: 1 },
        kind: crate::RecipeKind::Recipe,
        member_source: None,
    }
}

#[test]
fn recipe_and_public_gather_preserve_groups_warnings_and_fresh_membership() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("src/nested")).unwrap();
    for path in ["src/a", "src/skip.txt", "src/nested/b"] {
        std::fs::write(dir.path().join(path), "").unwrap();
    }
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    register_resolve_gather(&lua, dir.path(), dir.path()).unwrap();
    let recipe = gathered_recipe(&lua, &["src/*", "src/nested/**", "src/*", "src/skip.txt", "empty/*"], &["**/skip.txt"]);
    let warnings = Rc::new(RefCell::new(Vec::new()));
    setup_recipe_context(&lua, &recipe, dir.path(), dir.path(), &warnings).unwrap();
    let groups: Vec<Vec<String>> = lua.load("return recipe.inputs").eval().unwrap();
    assert_eq!(groups, vec![vec!["src/a"], vec!["src/nested/b"], vec!["src/a"], vec![], vec![]]);
    assert_eq!(warnings.borrow().len(), 1);
    assert!(warnings.borrow()[0].contains("empty/*"));
    let public: Vec<String> = lua.load(r#"return cook.resolve_gather({"src/*", "src/nested/**", "src/*", "src/skip.txt", "empty/*"}, {"**/skip.txt"})"#).eval().unwrap();
    assert_eq!(public, groups.into_iter().flatten().collect::<Vec<_>>());
    std::fs::write(dir.path().join("src/new"), "").unwrap();
    std::fs::remove_file(dir.path().join("src/a")).unwrap();
    let fresh: Vec<String> = lua.load(r#"return cook.resolve_gather({"src/*"}, {"**/skip.txt"})"#).eval().unwrap();
    assert_eq!(fresh, ["src/new"]);
}

#[test]
#[cfg(unix)]
fn unrelated_physical_alias_cannot_exclude_a_logical_gathered_path() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/file.txt"), "included").unwrap();
    std::fs::write(dir.path().join("src\\file.txt"), "unrelated alias").unwrap();
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    register_resolve_gather(&lua, dir.path(), dir.path()).unwrap();
    let files: Vec<String> = lua.load(r#"return cook.resolve_gather({"src/file.txt"}, {"*.txt"})"#).eval().unwrap();
    assert_eq!(files, ["src/file.txt"]);
    let recipe = gathered_recipe(&lua, &["src/file.txt"], &["*.txt"]);
    setup_recipe_context(&lua, &recipe, dir.path(), dir.path(), &Rc::new(RefCell::new(vec![]))).unwrap();
    let groups: Vec<Vec<String>> = lua.load("return recipe.inputs").eval().unwrap();
    assert_eq!(groups, vec![vec!["src/file.txt"]]);
}
