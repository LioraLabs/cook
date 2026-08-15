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
/// The reason is the `inputs <probe>` pre-pass: it runs a probe's
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
/// values, keyed by probe key. COOK-190 / §22.5.10: for an `inputs
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
/// `label:`-prefixed) key. Three steps, in order (CS-0219):
///
/// 1. The register-phase probe-value store, if the key is already there —
///    put there by the `inputs <probe>` pre-pass, or by an earlier read
///    through step 2.
/// 2. Otherwise, if the key names a probe DECLARED in this pass, resolve it
///    now: run the same `cook_probe::eval` sequence the pre-pass and the
///    executor run, publish the value into the same store, and answer from it.
///    Resolving lazily, at the moment of the read, is what keeps §22.5.7's
///    demand-driven rule intact without a second reachability analysis — a
///    body only runs because its recipe is reachable, and a key is only
///    resolved because a body asked for it. Evaluating every declared probe up
///    front would be simpler and would put the cost of every probe on every
///    run (COOK-295).
/// 3. Otherwise the active module's persistent cache, which is what this name
///    meant before probes existed and still means for a key no probe declares.
///    A miss there is `nil`, unchanged: that store's whole contract is that a
///    missing key reads as absent.
fn probes_get(
    lua: &Lua,
    state: &SharedModuleLoaderState,
    resolver: &crate::engine::RegisterProbeResolver,
    key: &str,
) -> LuaResult<LuaValue> {
    if resolver.resolves(key) {
        // §22.5.4: a read made from inside a `produce` body may only name a key
        // that body declared in `inputs.requires`. Checked before the store
        // hit, not only before resolution — a key some earlier body already
        // resolved is just as far outside this probe's fingerprint chain as an
        // unresolved one.
        resolver.check_produce_read(key).map_err(|e| LuaError::runtime(e.to_string()))?;
        if let Some(val) = resolver.store().borrow().get(key) {
            return crate::probe_value::json_to_lua(lua, val);
        }
        resolver
            .resolve(lua, key)
            .map_err(|e| LuaError::runtime(e.to_string()))?;
        if let Some(val) = resolver.store().borrow().get(key) {
            return crate::probe_value::json_to_lua(lua, val);
        }
    } else if let Some(val) = resolver.store().borrow().get(key) {
        // A `key:field` selector alias the pre-pass stashed under its verbatim
        // ref, which is not itself a declared probe key.
        return crate::probe_value::json_to_lua(lua, val);
    }
    let state = state.borrow();
    let module_name = state
        .active_module()
        .ok_or_else(|| LuaError::runtime("cook.probes.get called outside of a module context"))?
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
        .ok_or_else(|| LuaError::runtime("cook.probes.set called outside of a module context"))?
        .to_string();
    if let Some(cache) = state.caches.get_mut(&module_name) {
        cache.set(key, json_val);
    }
    Ok(())
}

pub fn register_cache_api(
    lua: &Lua,
    state: SharedModuleLoaderState,
    resolver: Rc<crate::engine::RegisterProbeResolver>,
) -> LuaResult<()> {
    let prepass = resolver.store().clone();
    let cook: LuaTable = lua.globals().get("cook")?;

    // cook.__probe_subst(ident) — CS-0195: the register-time rendering of a
    // `$<key:...>` reference in a position that must resolve before execute
    // (a fan-out output pattern; the path must be known to register the
    // unit). Parses the ident through the shared grammar and renders through
    // the CS-0192 law over the pre-pass store's JSON value: a scalar renders
    // as its canonical token, and a composite, null, or absent member is a
    // register-phase diagnostic — the old lowering interpolated Lua's
    // `tostring` of a table, a heap address, into a declared output path.
    let prepass_subst = prepass.clone();
    let resolver_subst = resolver.clone();
    let subst_fn = lua.create_function(move |lua, ident: String| {
        let r = cook_contracts::sigil::probe_ref(&ident)
            .ok_or_else(|| LuaError::runtime(format!("$<{ident}>: not a probe-value reference")))?;
        // CS-0219: resolve on demand, exactly as a `cook.probes.get` read does.
        // Reading the store alone made this succeed or fail on whether some
        // unrelated recipe registered earlier and happened to have read the
        // same probe — an output path that registers or does not depending on
        // a neighbour's body order.
        if resolver_subst.resolves(r.key()) && !prepass_subst.borrow().contains_key(r.key()) {
            resolver_subst
                .resolve(lua, r.key())
                .map_err(|e| LuaError::runtime(e.to_string()))?;
        }
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

    // cook.probes.{get,set,scope} — §6.3.4, §24.4.3. The table, the scope view
    // and the `label:key` prefixing are `cook_lua_stdlib::install_probes_api`,
    // one implementation with the worker VM (COOK-439); the two operations
    // below are what this phase means by a probe read and a probe write.
    //
    // The register phase's `set` WRITES, into the active module's persistent
    // cache. The execute phase's raises (CS-0074). That difference is
    // specified, and passing both in as arguments is what makes it visible
    // instead of buried in a copy of the scaffolding: before this, the scope
    // view was built twice and only its innermost setter differed.
    //
    // Until COOK-412 the register VM never installed `scope` at all, so the
    // scoped pattern raised a nil-index error at register phase while working
    // at execute phase — the same class of failure, from the same cause.
    let s_get = state.clone();
    let resolver_get = resolver.clone();
    let s_set = state.clone();
    cook_lua_stdlib::install_probes_api(
        lua,
        &cook,
        move |lua, key: &str| probes_get(lua, &s_get, &resolver_get, key),
        move |_lua, key: &str, value: &LuaValue| probes_set(&s_set, key, value),
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/module_loader_tests.rs"]
mod tests;
