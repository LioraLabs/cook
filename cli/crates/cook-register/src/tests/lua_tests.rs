use super::*;

fn setup() -> (Lua, SharedExportStore) {
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    let store: SharedExportStore = Rc::new(RefCell::new(BTreeMap::new()));
    register_export_api(&lua, store.clone()).unwrap();
    (lua, store)
}

#[test]
fn test_export_and_import_lua() {
    let (lua, _) = setup();
    lua.load(r#"cook.export("mylib", { includes = { "include/" }, lib_path = "build/libmylib.a" })"#)
        .exec()
        .unwrap();
    let result: String = lua
        .load(r#"local info = cook.import("mylib") return info.lib_path"#)
        .eval()
        .unwrap();
    assert_eq!(result, "build/libmylib.a");
}

#[test]
fn test_import_missing_returns_nil() {
    let (lua, _) = setup();
    let result: LuaValue = lua
        .load(r#"return cook.import("nonexistent")"#)
        .eval()
        .unwrap();
    assert!(matches!(result, LuaValue::Nil));
}

#[test]
fn test_export_survives_across_store_borrows() {
    let (lua, store) = setup();
    lua.load(r#"cook.export("lib", { path = "build/lib.a" })"#)
        .exec()
        .unwrap();

    // Second VM with same store (simulates second recipe)
    let lua2 = Lua::new();
    lua2.globals()
        .set("cook", lua2.create_table().unwrap())
        .unwrap();
    register_export_api(&lua2, store.clone()).unwrap();
    let result: String = lua2
        .load(r#"local info = cook.import("lib") return info.path"#)
        .eval()
        .unwrap();
    assert_eq!(result, "build/lib.a");
}
