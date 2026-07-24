use super::*;

fn lua_with_api() -> (Lua, ()) {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    register_tools_api(&lua, &cook).unwrap();
    lua.globals().set("cook", cook).unwrap();
    (lua, ())
}

#[test]
fn id_resolves_a_real_tool_with_hash_and_path() {
    let (lua, _) = lua_with_api();
    // `sh` exists on every platform we test on.
    let (hash, path): (String, String) = lua
        .load("local t = cook.tools.id('sh'); return t.hash, t.path")
        .eval()
        .unwrap();
    assert_eq!(hash.len(), 64, "lowercase-hex sha256");
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(path.ends_with("sh"), "resolved path, got {path}");
}

#[test]
fn id_returns_nil_for_missing_tool() {
    let (lua, _) = lua_with_api();
    let is_nil: bool = lua
        .load("return cook.tools.id('definitely-not-a-tool-xyz') == nil")
        .eval()
        .unwrap();
    assert!(is_nil);
}
