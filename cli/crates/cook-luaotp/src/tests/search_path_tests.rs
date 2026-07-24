use super::*;
use std::path::PathBuf;

#[test]
fn refresh_sets_path_and_cpath_with_rock_tree_entries() {
    let lua = mlua::Lua::new();
    let cwd = PathBuf::from("/tmp/fake-project");
    refresh_package_search_paths(&lua, &cwd).expect("refresh");
    let pkg: mlua::Table = lua.globals().get("package").unwrap();
    let path: String = pkg.get("path").unwrap();
    let cpath: String = pkg.get("cpath").unwrap();

    assert!(path.contains("/tmp/fake-project/cook_modules/?.lua"));
    assert!(path.contains("/tmp/fake-project/cook_modules/?/init.lua"));
    assert!(path.contains("/tmp/fake-project/cook_modules/share/lua/5.4/?.lua"));
    assert!(path.contains("/tmp/fake-project/cook_modules/share/lua/5.4/?/init.lua"));

    assert!(cpath.contains("/tmp/fake-project/cook_modules/?."));
    assert!(cpath.contains("/tmp/fake-project/cook_modules/lib/lua/5.4/?."));
}

#[test]
fn refresh_is_idempotent() {
    let lua = mlua::Lua::new();
    let cwd = PathBuf::from("/tmp/fake-project");
    refresh_package_search_paths(&lua, &cwd).expect("first");
    let pkg: mlua::Table = lua.globals().get("package").unwrap();
    let first_path: String = pkg.get("path").unwrap();
    let first_cpath: String = pkg.get("cpath").unwrap();
    refresh_package_search_paths(&lua, &cwd).expect("second");
    let second_path: String = pkg.get("path").unwrap();
    let second_cpath: String = pkg.get("cpath").unwrap();
    assert_eq!(first_path, second_path, "path must not grow on repeated refresh");
    assert_eq!(first_cpath, second_cpath, "cpath must not grow on repeated refresh");
}
