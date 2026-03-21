use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use mlua::prelude::*;

use crate::hash_str;
use crate::module_cache::ModuleCache;

// ---------------------------------------------------------------------------
// JSON <-> Lua conversion helpers
// ---------------------------------------------------------------------------

pub fn json_to_lua_value(lua: &Lua, val: serde_json::Value) -> LuaResult<LuaValue> {
    match val {
        serde_json::Value::Null => Ok(LuaValue::Nil),
        serde_json::Value::Bool(b) => Ok(LuaValue::Boolean(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LuaValue::Integer(i))
            } else {
                Ok(LuaValue::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(LuaValue::String(lua.create_string(&s)?)),
        serde_json::Value::Array(arr) => {
            let tbl = lua.create_table()?;
            for (i, v) in arr.into_iter().enumerate() {
                tbl.set(i + 1, json_to_lua_value(lua, v)?)?;
            }
            Ok(LuaValue::Table(tbl))
        }
        serde_json::Value::Object(map) => {
            let tbl = lua.create_table()?;
            for (k, v) in map {
                tbl.set(k, json_to_lua_value(lua, v)?)?;
            }
            Ok(LuaValue::Table(tbl))
        }
    }
}

pub fn lua_value_to_json(val: LuaValue) -> serde_json::Value {
    match val {
        LuaValue::Nil => serde_json::Value::Null,
        LuaValue::Boolean(b) => serde_json::json!(b),
        LuaValue::Integer(i) => serde_json::json!(i),
        LuaValue::Number(n) => serde_json::json!(n),
        LuaValue::String(s) => serde_json::json!(s.to_string_lossy()),
        LuaValue::Table(t) => {
            // Try as array first (check if sequential integer keys), fall back to object
            let mut arr = Vec::new();
            let mut is_array = true;
            for pair in t.clone().pairs::<LuaValue, LuaValue>() {
                if let Ok((k, v)) = pair {
                    if let LuaValue::Integer(_) = k {
                        arr.push(lua_value_to_json(v));
                    } else {
                        is_array = false;
                        break;
                    }
                }
            }
            if is_array && !arr.is_empty() {
                serde_json::Value::Array(arr)
            } else {
                let mut map = serde_json::Map::new();
                for pair in t.pairs::<String, LuaValue>() {
                    if let Ok((k, v)) = pair {
                        map.insert(k, lua_value_to_json(v));
                    }
                }
                serde_json::Value::Object(map)
            }
        }
        _ => serde_json::Value::Null,
    }
}

// ---------------------------------------------------------------------------
// ModuleLoaderState
// ---------------------------------------------------------------------------

pub struct ModuleLoaderState {
    pub working_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// Set during a `load_module` call; cleared afterwards.
    pub current_module: Option<String>,
    /// Tracks the most recently loaded module so that module-returned functions
    /// can still access the cache after `load_module` has returned.
    pub last_module: Option<String>,
    pub caches: std::collections::HashMap<String, ModuleCache>,
}

pub type SharedModuleLoaderState = Rc<RefCell<ModuleLoaderState>>;

impl ModuleLoaderState {
    pub fn new(working_dir: PathBuf) -> Self {
        let cache_dir = working_dir.join(".cook").join("cache");
        Self {
            working_dir,
            cache_dir,
            current_module: None,
            last_module: None,
            caches: std::collections::HashMap::new(),
        }
    }

    /// Return the active module name: `current_module` during loading,
    /// `last_module` for post-load function calls.
    pub fn active_module(&self) -> Option<&str> {
        self.current_module
            .as_deref()
            .or(self.last_module.as_deref())
    }

    pub fn flush_all(&self) {
        for cache in self.caches.values() {
            let _ = cache.flush();
        }
    }
}

// ---------------------------------------------------------------------------
// cook.load_module(name)
// ---------------------------------------------------------------------------

pub fn register_module_loader(lua: &Lua, state: SharedModuleLoaderState) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    let s = state.clone();
    let load_module_fn = lua.create_function(move |lua, name: String| {
        // 1. Resolve path: working_dir/cook_modules/name.lua or name/init.lua
        let working_dir = s.borrow().working_dir.clone();
        let modules_dir = working_dir.join("cook_modules");

        let flat_path = modules_dir.join(format!("{}.lua", name));
        let init_path = modules_dir.join(&name).join("init.lua");

        let module_path = if flat_path.exists() {
            flat_path
        } else if init_path.exists() {
            init_path
        } else {
            return Err(LuaError::runtime(format!(
                "module not found: {} (tried {}.lua and {}/init.lua)",
                name,
                name,
                name
            )));
        };

        // 2. Read the file, hash with hash_str
        let source = std::fs::read_to_string(&module_path).map_err(|e| {
            LuaError::runtime(format!("failed to read module {}: {}", name, e))
        })?;
        let source_hash = hash_str(&source);

        // 3. Create/load the module's cache, set source hash
        {
            let mut state = s.borrow_mut();
            let cache_dir = state.cache_dir.clone();
            let cache = ModuleCache::load(&cache_dir, &name, source_hash);
            state.caches.insert(name.clone(), cache);
            // Update source hash in cache
            if let Some(c) = state.caches.get_mut(&name) {
                c.set_source_hash(source_hash);
            }
        }

        // 4. Set current_module to the module name (for cook.cache scoping)
        s.borrow_mut().current_module = Some(name.clone());

        // 5. Execute the module file
        let chunk_name = format!("@{}", module_path.display());
        let result: LuaValue = lua.load(&source).set_name(&chunk_name).eval()?;

        // 6. If the returned table has an init() function, call it
        if let LuaValue::Table(ref tbl) = result {
            if let Ok(LuaValue::Function(init_fn)) = tbl.get::<LuaValue>("init") {
                init_fn.call::<()>(())?;
            }
        }

        // 7. Flush cache, clear current_module (but remember as last_module)
        {
            let state = s.borrow();
            if let Some(cache) = state.caches.get(&name) {
                let _ = cache.flush();
            }
        }
        {
            let mut state = s.borrow_mut();
            state.last_module = Some(name.clone());
            state.current_module = None;
        }

        // 8. Return the module table
        Ok(result)
    })?;

    cook.set("load_module", load_module_fn)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// cook.cache.* API
// ---------------------------------------------------------------------------

pub fn register_cache_api(lua: &Lua, state: SharedModuleLoaderState) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let cache_tbl = lua.create_table()?;

    // cook.cache.get(key)
    let s = state.clone();
    let get_fn = lua.create_function(move |lua, key: String| {
        let state = s.borrow();
        let module_name = state.active_module().ok_or_else(|| {
            LuaError::runtime("cook.cache.get called outside of a module context")
        })?.to_string();
        match state.caches.get(&module_name).and_then(|c| c.get(&key)) {
            Some(val) => json_to_lua_value(lua, val.clone()),
            None => Ok(LuaValue::Nil),
        }
    })?;
    cache_tbl.set("get", get_fn)?;

    // cook.cache.set(key, value)
    let s2 = state.clone();
    let set_fn = lua.create_function(move |_, (key, value): (String, LuaValue)| {
        let json_val = lua_value_to_json(value);
        let mut state = s2.borrow_mut();
        let module_name = state.active_module().ok_or_else(|| {
            LuaError::runtime("cook.cache.set called outside of a module context")
        })?.to_string();
        if let Some(cache) = state.caches.get_mut(&module_name) {
            cache.set(&key, json_val);
        }
        Ok(())
    })?;
    cache_tbl.set("set", set_fn)?;

    // cook.cache.invalidate(key)
    let s3 = state.clone();
    let invalidate_fn = lua.create_function(move |_, key: String| {
        let mut state = s3.borrow_mut();
        let module_name = state.active_module().ok_or_else(|| {
            LuaError::runtime("cook.cache.invalidate called outside of a module context")
        })?.to_string();
        if let Some(cache) = state.caches.get_mut(&module_name) {
            cache.invalidate(&key);
        }
        Ok(())
    })?;
    cache_tbl.set("invalidate", invalidate_fn)?;

    // cook.cache.clear()
    let s4 = state.clone();
    let clear_fn = lua.create_function(move |_, ()| {
        let mut state = s4.borrow_mut();
        let module_name = state.active_module().ok_or_else(|| {
            LuaError::runtime("cook.cache.clear called outside of a module context")
        })?.to_string();
        if let Some(cache) = state.caches.get_mut(&module_name) {
            cache.clear();
        }
        Ok(())
    })?;
    cache_tbl.set("clear", clear_fn)?;

    cook.set("cache", cache_tbl)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_with_module(
        module_name: &str,
        module_code: &str,
    ) -> (Lua, TempDir, SharedModuleLoaderState) {
        let dir = TempDir::new().unwrap();
        let modules_dir = dir.path().join("cook_modules");
        std::fs::create_dir_all(&modules_dir).unwrap();
        std::fs::write(modules_dir.join(format!("{}.lua", module_name)), module_code).unwrap();

        let lua = Lua::new();
        let cook = lua.create_table().unwrap();
        lua.globals().set("cook", cook).unwrap();

        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state.clone()).unwrap();
        register_cache_api(&lua, state.clone()).unwrap();
        (lua, dir, state)
    }

    #[test]
    fn test_load_module_returns_table() {
        let (lua, _dir, _) =
            setup_with_module("test_mod", "local m = {} m.value = 42 return m");
        let result: i32 = lua
            .load(r#"local m = cook.load_module("test_mod") return m.value"#)
            .eval()
            .unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_load_module_calls_init() {
        let (lua, _dir, _) = setup_with_module(
            "test_mod",
            "local m = {} m.initialized = false function m.init() m.initialized = true end return m",
        );
        let result: bool = lua
            .load(r#"local m = cook.load_module("test_mod") return m.initialized"#)
            .eval()
            .unwrap();
        assert!(result);
    }

    #[test]
    fn test_load_module_not_found() {
        let dir = TempDir::new().unwrap();
        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state).unwrap();
        let result = lua.load(r#"cook.load_module("nonexistent")"#).exec();
        assert!(result.is_err());
    }

    #[test]
    fn test_load_module_init_lua() {
        let dir = TempDir::new().unwrap();
        let modules_dir = dir.path().join("cook_modules").join("mymod");
        std::fs::create_dir_all(&modules_dir).unwrap();
        std::fs::write(
            modules_dir.join("init.lua"),
            "local m = {} m.from_init = true return m",
        )
        .unwrap();

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state).unwrap();
        let result: bool = lua
            .load(r#"local m = cook.load_module("mymod") return m.from_init"#)
            .eval()
            .unwrap();
        assert!(result);
    }

    #[test]
    fn test_cache_api_in_module() {
        let (lua, dir, state) = setup_with_module(
            "test_mod",
            r#"local m = {}
            function m.init()
                cook.cache.set("greeting", "hello")
            end
            function m.get_greeting()
                return cook.cache.get("greeting")
            end
            return m"#,
        );
        let result: String = lua
            .load(r#"local m = cook.load_module("test_mod") return m.get_greeting()"#)
            .eval()
            .unwrap();
        assert_eq!(result, "hello");
        state.borrow().flush_all();
        let cache_file = dir.path().join(".cook/cache/test_mod.json");
        assert!(cache_file.exists());
    }
}
