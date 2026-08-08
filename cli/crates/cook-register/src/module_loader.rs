use std::cell::RefCell;
use std::collections::BTreeMap;
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

// A third, WEAKER Lua→JSON walker (`lua_value_to_json`) lived here until
// COOK-388: mixed keys silently dropped, array holes compacted, cycles
// overflowed the stack, non-UTF-8 lossy-substituted, NaN became null,
// key iteration unsorted. The module-export path (`cook.export`) and the
// module-context `cook.probes.set` now go through THE validating walker
// in `cook_lua_stdlib::json_codec` — silent loss became a diagnostic.

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
        let cache_dir = cook_contracts::layout::cache_dir(&working_dir);
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

/// The register phase's obligations around the shared loader sequence
/// (`cook_lua_stdlib::install_module_loader`): per-module persistent caches
/// backing `cook.probes.{get,set}`, and the current/last-module tracking
/// that scopes those calls to the module they run in.
struct RegisterLoadHooks {
    state: SharedModuleLoaderState,
}

impl cook_lua_stdlib::ModuleLoadHooks for RegisterLoadHooks {
    fn on_memo_hit(&self, name: &str) {
        // Refresh last_module so post-load cache calls keep resolving.
        self.state.borrow_mut().last_module = Some(name.to_string());
    }

    fn before_eval(&self, name: &str, source: &str) -> LuaResult<()> {
        let source_hash = hash_str(source);
        let mut state = self.state.borrow_mut();
        let cache_dir = state.cache_dir.clone();
        let cache = ModuleCache::load(&cache_dir, name, source_hash);
        state.caches.insert(name.to_string(), cache);
        if let Some(c) = state.caches.get_mut(name) {
            c.set_source_hash(source_hash);
        }
        // For cook.probes scoping during the module's top-level chunk / init().
        state.current_module = Some(name.to_string());
        Ok(())
    }

    fn after_load(&self, name: &str, success: bool) {
        let mut state = self.state.borrow_mut();
        if success {
            if let Some(cache) = state.caches.get(name) {
                let _ = cache.flush();
            }
            state.last_module = Some(name.to_string());
        }
        state.current_module = None;
    }
}

/// Install `cook.load_module` on the register VM: the shared loader
/// sequence from `cook-lua-stdlib` (memoisation, cycle detection,
/// resolution, init(), package-path refresh — one implementation with the
/// worker VMs since COOK-412) over the register phase's hooks.
/// CS-0204: the register VM observes its module loads too.
///
/// Not because a register-phase load is a hole — it is not. A unit's whole
/// register-phase surface is its DECLARATION (command text, outputs, inputs,
/// seal keys), and every term of the declaration is already a determinant, so
/// a module that changes what a maker emits already busts the unit's key.
///
/// The reason is the `ingredients <probe>` pre-pass: it runs a probe's
/// `produce` source on THIS VM (`cook_probe::eval::ProduceRunner`), and a
/// probe's value is keyed by a fingerprint that folds no module source at all.
/// The pre-pass snapshots this observer around the run and folds what it saw
/// into the probe's key, exactly as a worker does for the executor's probes.
///
/// The handle is also installed as VM app data — the idiom this crate already
/// uses for a VM-scoped handle (`CacheContext`, `engine.rs`) — so the pre-pass
/// reaches it without threading a parameter through `install_all_apis` and the
/// eleven-argument recursion below it.
pub fn register_module_loader(
    lua: &Lua,
    state: SharedModuleLoaderState,
    observer: cook_lua_stdlib::ModuleObserver,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let working_dir = state.borrow().working_dir.clone();
    lua.set_app_data(observer.clone());
    cook_lua_stdlib::install_module_loader(
        lua,
        &cook,
        cook_lua_stdlib::WorkingDirSource::Static(working_dir),
        RegisterLoadHooks { state },
        observer.clone(),
    )?;
    cook_lua_stdlib::install_require_observer(lua, observer)
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

/// The register-phase `cook.probes.get` path for a full (possibly
/// `label:`-prefixed) key: the pre-pass store first (COOK-64 member-source
/// values — probe keys, which do not collide with module-cache keys), then
/// the active module's persistent cache.
fn probes_get(
    lua: &Lua,
    state: &SharedModuleLoaderState,
    prepass: &SharedPrepassStore,
    key: &str,
) -> LuaResult<LuaValue> {
    if let Some(val) = prepass.borrow().get(key) {
        return crate::probe_value::json_to_lua(lua, val);
    }
    let state = state.borrow();
    let module_name = state
        .active_module()
        .ok_or_else(|| {
            LuaError::runtime("cook.probes.get called outside of a module context")
        })?
        .to_string();
    match state.caches.get(&module_name).and_then(|c| c.get(key)) {
        Some(val) => json_to_lua_value(lua, val.clone()),
        None => Ok(LuaValue::Nil),
    }
}

/// The register-phase `cook.probes.set` path for a full key: validate the
/// value through THE walker (COOK-388) and store into the active module's
/// persistent cache.
fn probes_set(state: &SharedModuleLoaderState, key: &str, value: &LuaValue) -> LuaResult<()> {
    let json_val = cook_lua_stdlib::json_codec::lua_to_json(value)
        .map_err(|e| LuaError::runtime(format!("cook.probes.set('{key}'): {e}")))?;
    let mut state = state.borrow_mut();
    let module_name = state
        .active_module()
        .ok_or_else(|| {
            LuaError::runtime("cook.probes.set called outside of a module context")
        })?
        .to_string();
    if let Some(cache) = state.caches.get_mut(&module_name) {
        cache.set(key, json_val);
    }
    Ok(())
}

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
    let get_fn =
        lua.create_function(move |lua, key: String| probes_get(lua, &s, &prepass_get, &key))?;
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
    cook.set(cook_contracts::registration::PROBE_SUBST_NAME, subst_fn)?;

    // cook.probes.set(key, value)
    let s2 = state.clone();
    let set_fn = lua
        .create_function(move |_, (key, value): (String, LuaValue)| probes_set(&s2, &key, &value))?;
    cache_tbl.set("set", set_fn)?;

    // cook.probes.scope(label) — §24.4.3: a view whose get/set are the
    // `label:key`-prefixed operations on the same store. Until COOK-412 the
    // register VM never installed this, so the scoped pattern raised a
    // nil-index error at register phase while working at execute phase.
    let s3 = state.clone();
    let prepass_scope = prepass.clone();
    let scope_fn = lua.create_function(move |lua, label: String| {
        if let Some(msg) = cook_contracts::probe_key::scope_label_error(&label) {
            return Err(LuaError::runtime(msg));
        }
        let scoped = lua.create_table()?;

        let s_get = s3.clone();
        let prepass_get = prepass_scope.clone();
        let label_get = label.clone();
        let scoped_get = lua.create_function(move |lua, key: String| {
            let full = cook_contracts::probe_key::scoped_key(&label_get, &key);
            probes_get(lua, &s_get, &prepass_get, &full)
        })?;
        scoped.set("get", scoped_get)?;

        let s_set = s3.clone();
        let label_set = label.clone();
        let scoped_set = lua.create_function(move |_, (key, value): (String, LuaValue)| {
            let full = cook_contracts::probe_key::scoped_key(&label_set, &key);
            probes_set(&s_set, &full, &value)
        })?;
        scoped.set("set", scoped_set)?;

        Ok(scoped)
    })?;
    cache_tbl.set("scope", scope_fn)?;

    cook.set("probes", cache_tbl)?;
    cook_lua_stdlib::install_renamed_cache_stub(lua, &cook)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/module_loader_tests.rs"]
mod tests;
