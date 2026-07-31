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

// `json_to_lua_value` moved to `cook-lua-stdlib` alongside the codecs that
// depend on it (CS-0123), so the register VM and every execute-phase worker
// VM share one implementation. Re-exported here so the historical
// `crate::module_loader::json_to_lua_value` path (used by `export_api.rs`)
// keeps working.
pub use cook_lua_stdlib::json_to_lua_value;

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
        // (for cook.probes scoping).
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
// cook.probes.* API
// ---------------------------------------------------------------------------

/// COOK-64 §22.5.10: register-phase store of resolved member-source probe
/// values, keyed by probe key. COOK-190 / §22.5.10: for an `ingredients
/// <probe>` source ref resolved as a `key:field` selector, the pre-pass also
/// stashes the selected array under the verbatim ref (e.g. `"cards:list"`)
/// alongside the whole value stored under the bare probe key — a source ref
/// that names a probe exactly needs no such extra entry. The pre-pass
/// (`engine.rs`) populates it after top-level load but before any recipe body
/// runs; `cook.probes.get` consults it *before* the module-cache path so a
/// member-fanout recipe body's `local _items = cook.probes.get("<ref>")` sees the
/// resolved array instead of erroring "outside of a module context". Empty
/// for non-member-source sessions, so the module-cache behaviour is unchanged.
/// Values are decoded `serde_json::Value`s — JSON-native since CS-0102.
pub type SharedPrepassStore = Rc<RefCell<BTreeMap<String, serde_json::Value>>>;

pub fn register_cache_api(
    lua: &Lua,
    state: SharedModuleLoaderState,
    prepass: SharedPrepassStore,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let cache_tbl = lua.create_table()?;

    // cook.probes.get(key)
    let s = state.clone();
    let prepass_get = prepass.clone();
    let get_fn = lua.create_function(move |lua, key: String| {
        // COOK-64: a member-source probe value resolved by the pre-pass
        // takes precedence. These keys are probe keys, which do not collide
        // with module-cache keys, so the module-context path below is reached
        // unchanged for every non-member-source lookup.
        if let Some(val) = prepass_get.borrow().get(&key) {
            return crate::probe_value::json_to_lua(lua, val);
        }
        let state = s.borrow();
        let module_name = state.active_module().ok_or_else(|| {
            LuaError::runtime("cook.probes.get called outside of a module context")
        })?.to_string();
        match state.caches.get(&module_name).and_then(|c| c.get(&key)) {
            Some(val) => json_to_lua_value(lua, val.clone()),
            None => Ok(LuaValue::Nil),
        }
    })?;
    cache_tbl.set("get", get_fn)?;

    // cook.__probe_subst(ident) — CS-0195: the register-time rendering of a
    // `$<key:...>` reference in a position that must resolve before execute
    // (a fan-out output pattern; the path must be known to register the
    // unit). Parses the ident through the shared grammar and renders through
    // the CS-0192 law over the pre-pass store's JSON value: a scalar renders
    // as its canonical token, and a composite, null, or absent member is a
    // register-phase diagnostic — the old lowering interpolated Lua's
    // `tostring` of a table, a heap address, into a declared output path.
    let prepass_subst = prepass.clone();
    let subst_fn = lua.create_function(move |_, ident: String| {
        let r = cook_contracts::sigil::probe_ref(&ident).ok_or_else(|| {
            LuaError::runtime(format!("$<{ident}>: not a probe-value reference"))
        })?;
        let store = prepass_subst.borrow();
        let value = store.get(r.key()).ok_or_else(|| {
            LuaError::runtime(format!(
                "$<{ident}>: probe '{}' is not materialised in the register \
                 pre-pass; an output-pattern reference can only name a \
                 member-source probe the pre-pass resolved",
                r.key()
            ))
        })?;
        cook_contracts::sigil::subst::substitute(value, r.path(), &ident)
            .map_err(LuaError::runtime)
    })?;
    cook.set("__probe_subst", subst_fn)?;

    // cook.probes.set(key, value)
    let s2 = state.clone();
    let set_fn = lua.create_function(move |_, (key, value): (String, LuaValue)| {
        let json_val = lua_value_to_json(value);
        let mut state = s2.borrow_mut();
        let module_name = state.active_module().ok_or_else(|| {
            LuaError::runtime("cook.probes.set called outside of a module context")
        })?.to_string();
        if let Some(cache) = state.caches.get_mut(&module_name) {
            cache.set(&key, json_val);
        }
        Ok(())
    })?;
    cache_tbl.set("set", set_fn)?;

    cook.set("probes", cache_tbl)?;
    install_renamed_cache_stub(lua, &cook)?;
    Ok(())
}

/// `cook.cache.*` was renamed to `cook.probes.*` in v1.0 (CS-0136).
/// Install a stub whose every field access is a hard error with a did-you-mean.
fn install_renamed_cache_stub(lua: &Lua, cook: &LuaTable) -> LuaResult<()> {
    let stub = lua.create_table()?;
    let mt = lua.create_table()?;
    let index_fn = lua.create_function(|_, (_t, key): (LuaTable, String)| -> LuaResult<LuaValue> {
        Err(LuaError::runtime(format!(
            "'cook.cache' was renamed to 'cook.probes' in v1.0 (use cook.probes.{key})"
        )))
    })?;
    mt.set("__index", index_fn)?;
    stub.set_metatable(Some(mt));
    cook.set("cache", stub)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/module_loader_tests.rs"]
mod tests;
