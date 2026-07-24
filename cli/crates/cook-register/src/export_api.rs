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
#[path = "tests/lua_tests.rs"]
mod lua_tests;

#[cfg(test)]
#[path = "tests/export_api_tests.rs"]
mod tests;
