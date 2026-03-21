use std::collections::BTreeMap;
use std::cell::RefCell;
use std::rc::Rc;

use mlua::prelude::*;

/// Export store is a simple BTreeMap passed in by the caller.
/// ExportStore as a proper type belongs to cook-engine.
pub type SharedExportStore = Rc<RefCell<BTreeMap<String, serde_json::Value>>>;

/// Register `cook.export(name, table)` and `cook.import(name)` on the cook table.
pub fn register_export_api(lua: &Lua, store: SharedExportStore) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    let s = store.clone();
    let export_fn = lua.create_function(move |_, (name, value): (String, LuaValue)| {
        let json_val = crate::module_loader::lua_value_to_json(value);
        s.borrow_mut().insert(name, json_val);
        Ok(())
    })?;
    cook.set("export", export_fn)?;

    let s2 = store.clone();
    let import_fn = lua.create_function(move |lua, name: String| {
        let store = s2.borrow();
        match store.get(&name) {
            Some(val) => crate::module_loader::json_to_lua_value(lua, val.clone()),
            None => Ok(LuaValue::Nil),
        }
    })?;
    cook.set("import", import_fn)?;

    Ok(())
}

#[cfg(test)]
mod lua_tests {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_and_import() {
        let mut store = BTreeMap::new();
        let value = serde_json::json!({
            "includes": ["/usr/include"],
            "lib_path": "/usr/lib"
        });
        store.insert("mylib".to_string(), value.clone());
        let imported = store.get("mylib").unwrap();
        assert_eq!(imported, &value);
        assert_eq!(imported["includes"][0], serde_json::json!("/usr/include"));
        assert_eq!(imported["lib_path"], serde_json::json!("/usr/lib"));
    }

    #[test]
    fn test_import_missing_returns_none() {
        let store: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_export_overwrites() {
        let mut store = BTreeMap::new();
        store.insert("key".to_string(), serde_json::json!("first"));
        store.insert("key".to_string(), serde_json::json!("second"));
        let val = store.get("key").unwrap();
        assert_eq!(val, &serde_json::json!("second"));
    }
}
