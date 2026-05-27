use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
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
    /// Modules whose load is in flight on this VM. Used to detect
    /// `cook.load_module` cycles (§6.3.4): if a re-entrant call names a module
    /// already in this set, the loader raises a diagnostic naming the cycle.
    pub currently_loading: BTreeSet<String>,
    /// Ordered stack of in-flight module names, parallel to `currently_loading`.
    /// Used to render the cycle path `a -> b -> a` in the diagnostic.
    pub loading_stack: Vec<String>,
    /// Memoization table for successful module loads (§6.3.4): a second
    /// `cook.load_module(name)` MUST return the same Lua value without
    /// re-reading or re-evaluating the module file. Keyed by module name;
    /// values are registry keys held by the parent VM.
    pub loaded: BTreeMap<String, LuaRegistryKey>,
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
            currently_loading: BTreeSet::new(),
            loading_stack: Vec::new(),
            loaded: BTreeMap::new(),
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
        // 0. Memoization (§6.3.4): a second cook.load_module("name") on this VM
        // returns the same value without re-reading or re-evaluating the file.
        let cached_value: Option<LuaValue> = {
            let st = s.borrow();
            if let Some(key) = st.loaded.get(&name) {
                Some(lua.registry_value(key)?)
            } else {
                None
            }
        };
        if let Some(cached) = cached_value {
            // Refresh last_module so post-load cache calls keep resolving.
            s.borrow_mut().last_module = Some(name.clone());
            return Ok(cached);
        }

        // 0a. Cycle detection (§6.3.4): if `name` is already in the in-flight
        // set, raise a diagnostic naming the cycle path so authors can locate
        // the offending edge.
        {
            let st = s.borrow();
            if st.currently_loading.contains(&name) {
                let mut path = st.loading_stack.clone();
                path.push(name.clone());
                let rendered = path.join(" -> ");
                return Err(LuaError::runtime(format!(
                    "module cycle detected: {}",
                    rendered
                )));
            }
        }

        // 1. Resolve path: hand-vendored wins over LuaRocks-installed.
        //    Order mirrors cook-luaotp/src/pool.rs:616 (Standard §7).
        let working_dir = s.borrow().working_dir.clone();
        let modules_dir = working_dir.join("cook_modules");
        let share_dir = modules_dir.join("share/lua/5.4");

        let candidates = [
            modules_dir.join(format!("{}.lua", name)),
            modules_dir.join(&name).join("init.lua"),
            share_dir.join(format!("{}.lua", name)),
            share_dir.join(&name).join("init.lua"),
        ];

        let module_path = match candidates.iter().find(|p| p.exists()) {
            Some(p) => p.clone(),
            None => {
                return Err(LuaError::runtime(format!(
                    "module not found: {} (tried {}.lua, {}/init.lua, \
                     share/lua/5.4/{}.lua, share/lua/5.4/{}/init.lua)",
                    name, name, name, name, name
                )));
            }
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

        // 4. Mark as in-flight (for cycle detection) and set current_module
        // (for cook.cache scoping).
        {
            let mut state = s.borrow_mut();
            state.currently_loading.insert(name.clone());
            state.loading_stack.push(name.clone());
            state.current_module = Some(name.clone());
        }

        // Helper: drop in-flight marker on every exit path (success or error)
        // so cycle detection survives recoverable errors.
        let pop_inflight = |s: &SharedModuleLoaderState, name: &str| {
            let mut state = s.borrow_mut();
            state.currently_loading.remove(name);
            if let Some(top) = state.loading_stack.last() {
                if top == name {
                    state.loading_stack.pop();
                }
            }
            state.current_module = None;
        };

        // 4b. Extend package.path / package.cpath so that sub-requires within
        // a multi-file rock (e.g. `require("cook_cc.toolchain")` inside
        // cook_cc/init.lua, or `require("lpeg")` for a C extension) resolve
        // against cook_modules/. Mirrors the execute-phase logic in
        // cook-luaotp/src/pool.rs:refresh_package_search_paths.  We prepend
        // the cook_modules paths once per VM using the same stash-and-prepend
        // idiom as the execute-phase so repeated load_module calls on the same
        // VM are idempotent.
        {
            if let Ok(LuaValue::Table(pkg)) = lua.globals().get::<LuaValue>("package") {
                let cm = modules_dir.display().to_string();
                let so_ext = if cfg!(target_os = "windows") { "dll" } else { "so" };

                // -- package.path (pure-Lua modules) --
                let original_path: String = match pkg.get::<LuaValue>("_cook_original_path") {
                    Ok(LuaValue::String(s)) => s.to_string_lossy(),
                    _ => {
                        let cur: String = pkg.get::<String>("path").unwrap_or_default();
                        let _ = pkg.set("_cook_original_path", cur.clone());
                        cur
                    }
                };
                let new_path = format!(
                    "{cm}/?.lua;{cm}/?/init.lua;\
                     {cm}/share/lua/5.4/?.lua;{cm}/share/lua/5.4/?/init.lua;\
                     {orig}",
                    cm = cm,
                    orig = original_path,
                );
                let _ = pkg.set("path", new_path);

                // -- package.cpath (C extension modules) --
                let original_cpath: String = match pkg.get::<LuaValue>("_cook_original_cpath") {
                    Ok(LuaValue::String(s)) => s.to_string_lossy(),
                    _ => {
                        let cur: String = pkg.get::<String>("cpath").unwrap_or_default();
                        let _ = pkg.set("_cook_original_cpath", cur.clone());
                        cur
                    }
                };
                let new_cpath = format!(
                    "{cm}/?.{ext};{cm}/lib/lua/5.4/?.{ext};{orig}",
                    cm = cm,
                    ext = so_ext,
                    orig = original_cpath,
                );
                let _ = pkg.set("cpath", new_cpath);
            }
        }

        // 5. Execute the module file
        let chunk_name = format!("@{}", module_path.display());
        let result: LuaValue = match lua.load(&source).set_name(&chunk_name).eval() {
            Ok(v) => v,
            Err(e) => {
                pop_inflight(&s, &name);
                return Err(e);
            }
        };

        // 6. If the returned table has an init() function, call it
        if let LuaValue::Table(ref tbl) = result {
            if let Ok(LuaValue::Function(init_fn)) = tbl.get::<LuaValue>("init") {
                if let Err(e) = init_fn.call::<()>(()) {
                    pop_inflight(&s, &name);
                    return Err(e);
                }
            }
        }

        // 7. Flush cache, clear in-flight marker (but remember as last_module)
        {
            let state = s.borrow();
            if let Some(cache) = state.caches.get(&name) {
                let _ = cache.flush();
            }
        }
        {
            let mut state = s.borrow_mut();
            state.currently_loading.remove(&name);
            if let Some(top) = state.loading_stack.last() {
                if top == &name {
                    state.loading_stack.pop();
                }
            }
            state.last_module = Some(name.clone());
            state.current_module = None;
        }

        // 7a. Memoize successful load so subsequent cook.load_module(name)
        // calls return the same Lua value (§6.3.4).
        let key = lua.create_registry_value(result.clone())?;
        s.borrow_mut().loaded.insert(name.clone(), key);

        // 8. Return the module table
        Ok(result)
    })?;

    cook.set("load_module", load_module_fn)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// cook.cache.* API
// ---------------------------------------------------------------------------

/// COOK-64 §22.5.9: register-phase store of resolved `for_each`-feeding probe
/// values, keyed by probe key. The pre-pass (`engine.rs`) populates it after
/// top-level load but before any recipe body runs; `cook.cache.get` consults
/// it *before* the module-cache path so a `for_each` recipe body's
/// `local _items = cook.cache.get("cards")` sees the resolved array instead of
/// erroring "outside of a module context". Empty for non-`for_each` sessions,
/// so the module-cache behaviour is unchanged.
pub type SharedPrepassStore = Rc<RefCell<BTreeMap<String, rmpv::Value>>>;

pub fn register_cache_api(
    lua: &Lua,
    state: SharedModuleLoaderState,
    prepass: SharedPrepassStore,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let cache_tbl = lua.create_table()?;

    // cook.cache.get(key)
    let s = state.clone();
    let prepass_get = prepass.clone();
    let get_fn = lua.create_function(move |lua, key: String| {
        // COOK-64: a `for_each`-feeding probe value resolved by the pre-pass
        // takes precedence. These keys are probe keys, which do not collide
        // with module-cache keys, so the module-context path below is reached
        // unchanged for every non-`for_each` lookup.
        if let Some(val) = prepass_get.borrow().get(&key) {
            return crate::probe_value::msgpack_to_lua(lua, val);
        }
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
        register_cache_api(&lua, state.clone(), Rc::new(RefCell::new(BTreeMap::new()))).unwrap();
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
    fn test_load_module_memoized_returns_same_table() {
        // §6.3.4: a second cook.load_module(name) MUST return the same Lua
        // value without re-evaluating the module file. We verify by mutating
        // the table after first load and observing the mutation on the second
        // load (which would be reset if the file were re-evaluated).
        let (lua, _dir, _) = setup_with_module(
            "test_mod",
            "local m = {} m.value = 1 return m",
        );
        let result: i32 = lua
            .load(
                r#"local a = cook.load_module("test_mod")
                a.value = 99
                local b = cook.load_module("test_mod")
                return b.value"#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, 99, "memoization must return the same table instance");
    }

    #[test]
    fn test_load_module_init_runs_once_when_memoized() {
        // §6.3.4 corollary: if the module table is reused, init() must not
        // run again either. We track invocation count via a global counter.
        let (lua, _dir, _) = setup_with_module(
            "test_mod",
            r#"local m = {}
            function m.init()
                _G.init_calls = (_G.init_calls or 0) + 1
            end
            return m"#,
        );
        let calls: i32 = lua
            .load(
                r#"cook.load_module("test_mod")
                cook.load_module("test_mod")
                cook.load_module("test_mod")
                return _G.init_calls"#,
            )
            .eval()
            .unwrap();
        assert_eq!(calls, 1, "init must run exactly once across repeated loads");
    }

    #[test]
    fn test_load_module_cycle_two_modules_raises() {
        // §6.3.4 cycle detection: a cycle a -> b -> a MUST raise a diagnostic
        // naming the cycle, not stack-overflow.
        let dir = TempDir::new().unwrap();
        let modules_dir = dir.path().join("cook_modules");
        std::fs::create_dir_all(&modules_dir).unwrap();
        std::fs::write(
            modules_dir.join("a.lua"),
            r#"local m = {}
            cook.load_module("b")
            return m"#,
        )
        .unwrap();
        std::fs::write(
            modules_dir.join("b.lua"),
            r#"local m = {}
            cook.load_module("a")
            return m"#,
        )
        .unwrap();

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state).unwrap();

        let err = lua
            .load(r#"cook.load_module("a")"#)
            .exec()
            .expect_err("cycle must raise");
        let msg = format!("{}", err);
        assert!(
            msg.contains("module cycle detected"),
            "diagnostic must say `module cycle detected`, got: {}",
            msg
        );
        assert!(
            msg.contains("a -> b -> a"),
            "diagnostic must render the cycle path, got: {}",
            msg
        );
    }

    #[test]
    fn test_load_module_self_cycle_raises() {
        // A module that loads itself must surface the same diagnostic.
        let dir = TempDir::new().unwrap();
        let modules_dir = dir.path().join("cook_modules");
        std::fs::create_dir_all(&modules_dir).unwrap();
        std::fs::write(
            modules_dir.join("solo.lua"),
            r#"local m = {}
            cook.load_module("solo")
            return m"#,
        )
        .unwrap();

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state).unwrap();

        let err = lua
            .load(r#"cook.load_module("solo")"#)
            .exec()
            .expect_err("self-cycle must raise");
        let msg = format!("{}", err);
        assert!(msg.contains("solo -> solo"), "got: {}", msg);
    }

    #[test]
    fn test_load_module_recovers_after_error() {
        // After a module load fails, the in-flight set must be cleaned up so
        // a subsequent retry can proceed (cycle detection survives recoverable
        // errors).
        let dir = TempDir::new().unwrap();
        let modules_dir = dir.path().join("cook_modules");
        std::fs::create_dir_all(&modules_dir).unwrap();
        std::fs::write(
            modules_dir.join("boom.lua"),
            r#"error("intentional")"#,
        )
        .unwrap();

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state.clone()).unwrap();

        let _ = lua.load(r#"cook.load_module("boom")"#).exec();
        // After the failure the in-flight set must be empty.
        assert!(state.borrow().currently_loading.is_empty());
        assert!(state.borrow().loading_stack.is_empty());
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

    #[test]
    fn test_load_module_resolves_share_lua_flat() {
        let dir = TempDir::new().unwrap();
        let share_dir = dir.path().join("cook_modules/share/lua/5.4");
        std::fs::create_dir_all(&share_dir).unwrap();
        std::fs::write(
            share_dir.join("rockmod.lua"),
            "local m = {} m.tag = 'share-flat' return m",
        )
        .unwrap();

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state).unwrap();

        let tag: String = lua
            .load(r#"local m = cook.load_module("rockmod") return m.tag"#)
            .eval()
            .unwrap();
        assert_eq!(tag, "share-flat");
    }

    #[test]
    fn test_load_module_resolves_share_lua_init() {
        let dir = TempDir::new().unwrap();
        let share_dir = dir.path().join("cook_modules/share/lua/5.4/rockmod");
        std::fs::create_dir_all(&share_dir).unwrap();
        std::fs::write(
            share_dir.join("init.lua"),
            "local m = {} m.tag = 'share-init' return m",
        )
        .unwrap();

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state).unwrap();

        let tag: String = lua
            .load(r#"local m = cook.load_module("rockmod") return m.tag"#)
            .eval()
            .unwrap();
        assert_eq!(tag, "share-init");
    }

    #[test]
    fn test_load_module_top_level_wins_over_share_lua() {
        let dir = TempDir::new().unwrap();
        let modules_dir = dir.path().join("cook_modules");
        let share_dir = modules_dir.join("share/lua/5.4");
        std::fs::create_dir_all(&share_dir).unwrap();

        // hand-vendored at top level
        std::fs::write(modules_dir.join("rockmod.lua"), "return { tag = 'top' }").unwrap();
        // also installed under share/lua/5.4 — top-level must win
        std::fs::write(share_dir.join("rockmod.lua"), "return { tag = 'share' }").unwrap();

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state).unwrap();

        let tag: String = lua
            .load(r#"local m = cook.load_module("rockmod") return m.tag"#)
            .eval()
            .unwrap();
        assert_eq!(tag, "top");
    }
}
