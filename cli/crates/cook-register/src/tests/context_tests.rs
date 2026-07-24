use super::*;
use tempfile::TempDir;

/// CS-0064: `recipe.ingredients` / `cook.resolve_ingredients` MUST
/// drop sub-directory matches, so a tree with a file and a sibling
/// directory both matched by `*` yields only the file.
#[test]
fn resolve_glob_filters_directories() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("nested")).unwrap();

    let got = cook_fingerprint::resolve_ingredient_glob(dir.path(), dir.path(), "*").unwrap();
    let expected: BTreeSet<String> = ["a.txt".to_string()].into_iter().collect();
    assert_eq!(got, expected);
}

#[test]
fn excludes_match_lexically_equivalent_include_paths() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("dir")).unwrap();
        std::fs::write(dir.path().join("file"), "").unwrap();
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    register_resolve_ingredients(&lua, dir.path(), dir.path()).unwrap();

    for expression in [
        r#"return cook.resolve_ingredients({"dir/../file"}, {"file"})"#,
        r#"return cook.resolve_ingredients({"file"}, {"dir/../file"})"#,
    ] {
        let files: LuaTable = lua.load(expression).eval().unwrap();
        assert_eq!(files.raw_len(), 0, "{expression}");
    }
}
