//! The shared `cook.load_module` mechanics (§12.3, §24.2).
//!
//! Until COOK-412 this sequence existed twice: `cook-register`'s loader
//! memoized by module name with Rust-side state, `cook-luaotp`'s worker
//! loader memoized by `<cwd>::<name>` in Lua globals, and the two carried
//! independent cycle detection sharing only a string. COOK-393 had already
//! unified the candidate list and search-path composition around them; this
//! module unifies the loader itself. The decisions inside it — memo key,
//! diagnostics, chunk naming — are `cook_contracts::layout` law; what lives
//! here is the mlua-bearing mechanics.
//!
//! The two phases differ in exactly two ways, both parameterized:
//!
//! - **Working directory.** The register VM has one for its lifetime
//!   ([`WorkingDirSource::Static`]); a worker VM is reused across Cookfiles
//!   and resolves it per call ([`WorkingDirSource::Live`]). Memoisation and
//!   cycle detection key by `(working_dir, name)` per §12.3.2–12.3.3, so a
//!   worker serving two Cookfiles keeps their same-named modules distinct.
//! - **Register-only obligations.** The register phase wraps each load in
//!   module-cache and current-module bookkeeping ([`ModuleLoadHooks`]); the
//!   execute phase installs [`NoHooks`].

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::rc::Rc;

use mlua::prelude::*;

use crate::module_observer::ModuleObserver;
use crate::WorkingDirSource;

/// Register-only obligations around a module load. Every method defaults to
/// a no-op — the execute phase installs [`NoHooks`] and gets the bare
/// sequence; `cook-register` implements these to keep its module-cache and
/// current-module bookkeeping exactly where it always ran.
pub trait ModuleLoadHooks {
    /// A memoized load returned the cached value without re-evaluation.
    fn on_memo_hit(&self, _name: &str) {}

    /// After the module source is read, before its chunk is evaluated.
    /// An error here fails the load before any Lua runs.
    fn before_eval(&self, _name: &str, _source: &str) -> LuaResult<()> {
        Ok(())
    }

    /// After evaluation (and `init()`, when present) — on every exit path
    /// that reached `before_eval`, success and failure alike.
    fn after_load(&self, _name: &str, _success: bool) {}
}

/// The execute phase's hooks: nothing beyond the shared sequence.
pub struct NoHooks;
impl ModuleLoadHooks for NoHooks {}

/// Per-VM loader state: memoized loads (registry keys held by the VM) and
/// the in-flight set + ordered stack behind §12.3.3 cycle detection. All
/// keyed by `cook_contracts::layout::module_memo_key`; the stack holds bare
/// names because that is what the cycle diagnostic renders.
struct LoaderCore {
    loaded: BTreeMap<String, LuaRegistryKey>,
    /// The path each memoized load resolved to, under the same key.
    ///
    /// Kept beside `loaded` rather than derived on demand because a memo hit
    /// short-circuits resolution: without this the second unit served by a
    /// reused worker VM would load the module and record nothing, and would
    /// then be keyed on a module the first unit paid for (CS-0204).
    paths: BTreeMap<String, std::path::PathBuf>,
    in_flight: BTreeSet<String>,
    stack: Vec<String>,
}

/// Install `cook.load_module` on `cook`.
///
/// The full sequence, identical in both phases: memo lookup → cycle check →
/// §7 four-candidate resolution → source read → `hooks.before_eval` →
/// in-flight marking → `package.path`/`cpath` refresh → chunk eval →
/// `init()` → in-flight cleanup → `hooks.after_load` → memoize.
///
/// Cleanup runs on every exit path so detection survives recoverable errors
/// (§12.3.3): a module whose body or `init()` raised can be retried on the
/// same VM, and a genuine later cycle is still caught.
pub fn install_module_loader(
    lua: &Lua,
    cook: &LuaTable,
    working_dir: WorkingDirSource,
    hooks: impl ModuleLoadHooks + 'static,
    observer: ModuleObserver,
) -> LuaResult<()> {
    let core = Rc::new(RefCell::new(LoaderCore {
        loaded: BTreeMap::new(),
        paths: BTreeMap::new(),
        in_flight: BTreeSet::new(),
        stack: Vec::new(),
    }));

    let load_module_fn = lua.create_function(move |lua, raw_target: String| {
        let cwd = working_dir.resolve();

        // CS-0206: a target ending in `.lua` is a PATH, and is normalised
        // before anything else happens to it.
        //
        // Both halves of that are load-bearing. Normalising first is what
        // makes `./build/x.lua` and `build/x.lua` one memo key and therefore
        // one module instance (§12.3.2) — key first and they are two, and the
        // file's top-level chunk runs twice. Refusing here rather than at the
        // search is what makes §12.2.1 a property of the system: a `use`
        // declaration is checked by the parser, but this function takes a
        // string a body computed, and a rule enforced only where the parser
        // can see it constrains spellings rather than loads.
        let is_path = cook_contracts::module_binding::is_path_target(&raw_target);
        let name = if is_path {
            match cook_contracts::layout::normalise_use_path(&raw_target) {
                Ok(normalised) => normalised,
                Err(rejection) => {
                    return Err(LuaError::runtime(
                        cook_contracts::layout::use_path_rejected_message(&raw_target, rejection),
                    ));
                }
            }
        } else {
            raw_target.clone()
        };

        let memo_key = cook_contracts::layout::module_memo_key(&cwd, &name);

        // Memoisation (§12.3.2): a second load of (cwd, name) on this VM
        // returns the same Lua value without re-reading or re-evaluating.
        let cached: Option<(LuaValue, Option<std::path::PathBuf>)> = {
            let c = core.borrow();
            match c.loaded.get(&memo_key) {
                Some(key) => {
                    Some((lua.registry_value(key)?, c.paths.get(&memo_key).cloned()))
                }
                None => None,
            }
        };
        if let Some((v, path)) = cached {
            // CS-0204: the memo suppresses re-evaluation, not the dependency.
            if let Some(p) = path {
                observer.record(&p);
            }
            hooks.on_memo_hit(&name);
            return Ok(v);
        }

        // Cycle detection (§12.3.3): a re-entrant load of an in-flight
        // (cwd, name) raises the diagnostic naming the cycle path.
        {
            let c = core.borrow();
            if c.in_flight.contains(&memo_key) {
                return Err(LuaError::runtime(
                    cook_contracts::layout::module_cycle_message(&c.stack, &name),
                ));
            }
        }

        let module_path = if is_path {
            // §12.5 (CS-0206): a path-addressed module has no candidate list.
            // The declaration named one file; either it is there or resolution
            // fails.
            let candidate = cook_contracts::layout::module_path_candidate(&cwd, &name);
            if !candidate.exists() {
                return Err(LuaError::runtime(
                    cook_contracts::layout::module_path_not_found_message(
                        &cwd,
                        &raw_target,
                        &candidate,
                    ),
                ));
            }
            // The third containment check, and the only one the parser cannot
            // make: `build/helpers.lua` may be a symlink out of the tree.
            // Ordered after the existence check so a missing file reports as
            // missing rather than as an escape.
            if !cook_contracts::layout::contains_resolved_module_path(&cwd, &candidate) {
                return Err(LuaError::runtime(
                    cook_contracts::layout::module_path_escaped_message(
                        &cwd,
                        &raw_target,
                        &candidate,
                    ),
                ));
            }
            // Recorded and evaluated under the JOINED path, not the
            // canonicalised one: CS-0204 records it by stripping the working
            // directory prefix, and a canonicalised path can carry a different
            // prefix (`/tmp` -> `/private/tmp`) that no longer strips.
            candidate
        } else {
            // §7 / CS-0069 four-candidate resolution: hand-vendored wins over
            // LuaRocks-installed; the ONE list both phases probe (COOK-393).
            let candidates = cook_contracts::layout::module_candidates(&cwd, &name);
            match candidates.iter().find(|p| p.exists()) {
                Some(p) => p.clone(),
                None => {
                    return Err(LuaError::runtime(
                        cook_contracts::layout::module_not_found_message(&cwd, &name),
                    ));
                }
            }
        };

        // CS-0204: a resolved candidate is about to have its content
        // evaluated, so it is a determinant of whatever is running — whether
        // or not the chunk goes on to raise. Recorded here rather than at the
        // memoisation below so that a module which fails halfway still keys
        // the retry on the file that failed.
        observer.record(&module_path);

        let source = std::fs::read_to_string(&module_path).map_err(|e| {
            LuaError::runtime(cook_contracts::layout::module_read_failed_message(
                &name,
                &module_path,
                &e.to_string(),
            ))
        })?;

        hooks.before_eval(&name, &source)?;

        {
            let mut c = core.borrow_mut();
            c.in_flight.insert(memo_key.clone());
            c.stack.push(name.clone());
        }

        // Drop the in-flight marker on every exit path past this point.
        let finish = |core: &Rc<RefCell<LoaderCore>>| {
            let mut c = core.borrow_mut();
            c.in_flight.remove(&memo_key);
            if c.stack.last() == Some(&name) {
                c.stack.pop();
            }
        };

        // Sub-requires within a multi-file rock resolve against
        // cook_modules/ (§24.2). Idempotent per cwd via the stash keys, so
        // nested and repeated loads compose.
        refresh_package_search_paths(lua, &cwd)?;

        let chunk_name = cook_contracts::layout::module_chunk_name(&module_path);
        let result: LuaValue = match lua.load(&source).set_name(&chunk_name).eval() {
            Ok(v) => v,
            Err(e) => {
                finish(&core);
                hooks.after_load(&name, false);
                return Err(e);
            }
        };

        // init(), once, immediately after the top-level chunk (§12.3.1).
        if let LuaValue::Table(ref tbl) = result {
            if let Ok(LuaValue::Function(init_fn)) = tbl.get::<LuaValue>("init") {
                if let Err(e) = init_fn.call::<()>(()) {
                    finish(&core);
                    hooks.after_load(&name, false);
                    return Err(e);
                }
            }
        }

        finish(&core);
        hooks.after_load(&name, true);

        let key = lua.create_registry_value(result.clone())?;
        {
            let mut c = core.borrow_mut();
            c.paths.insert(memo_key.clone(), module_path.clone());
            c.loaded.insert(memo_key, key);
        }
        Ok(result)
    })?;

    // CS-0205: the field name is law, not a local choice — `cook-luagen`
    // COMPOSES `cook.load_module("…")` into every `use` binding it emits, and a
    // rename on either side would leave the alias binding through something
    // this loader (and therefore the CS-0204 observer inside it) never sees.
    cook.set(cook_contracts::module_binding::LOAD_MODULE_FN, load_module_fn)?;
    Ok(())
}

/// Refresh `package.path` / `package.cpath` so `require("foo")` resolves
/// against `<cwd>/cook_modules/` (§24.2, Standard §7 order).
///
/// The originals are stashed on the `package` table under the
/// `cook_contracts::layout` stash keys on first mutation, and every refresh
/// re-composes from the stash — so the call is idempotent per cwd and
/// correct across cwd changes (a worker VM serving units from different
/// Cookfiles). The composition itself is
/// `cook_contracts::layout::compose_lua_search_paths`, the one composer
/// both phases use (COOK-393); until COOK-412 this stash-and-recompose
/// idiom around it was spelled once per phase.
pub fn refresh_package_search_paths(lua: &Lua, cwd: &Path) -> LuaResult<()> {
    use cook_contracts::layout::{PACKAGE_CPATH_STASH_KEY, PACKAGE_PATH_STASH_KEY};
    let pkg: LuaTable = match lua.globals().get::<LuaValue>("package")? {
        LuaValue::Table(t) => t,
        _ => return Ok(()),
    };

    let original_path: String = match pkg.get::<LuaValue>(PACKAGE_PATH_STASH_KEY)? {
        LuaValue::String(s) => s.to_str()?.to_string(),
        _ => {
            let cur: String = pkg.get::<String>("path").unwrap_or_default();
            pkg.set(PACKAGE_PATH_STASH_KEY, cur.clone())?;
            cur
        }
    };
    let original_cpath: String = match pkg.get::<LuaValue>(PACKAGE_CPATH_STASH_KEY)? {
        LuaValue::String(s) => s.to_str()?.to_string(),
        _ => {
            let cur: String = pkg.get::<String>("cpath").unwrap_or_default();
            pkg.set(PACKAGE_CPATH_STASH_KEY, cur.clone())?;
            cur
        }
    };

    let composed =
        cook_contracts::layout::compose_lua_search_paths(cwd, &original_path, &original_cpath);
    pkg.set("path", composed.path)?;
    pkg.set("cpath", composed.cpath)?;
    Ok(())
}

/// `cook.cache.*` was renamed to `cook.probes.*` in v1.0 (CS-0136). Install
/// a stub whose every field access is a hard error with a did-you-mean.
/// Both VMs install this one stub; the literal used to be spelled
/// character-identically in each.
pub fn install_renamed_cache_stub(lua: &Lua, cook: &LuaTable) -> LuaResult<()> {
    let stub = lua.create_table()?;
    let mt = lua.create_table()?;
    let index_fn =
        lua.create_function(|_, (_t, key): (LuaTable, String)| -> LuaResult<LuaValue> {
            Err(LuaError::runtime(format!(
                "'cook.cache' was renamed to 'cook.probes' in v1.0 (use cook.probes.{key})"
            )))
        })?;
    mt.set("__index", index_fn)?;
    stub.set_metatable(Some(mt));
    cook.set("cache", stub)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/module_loader_tests.rs"]
mod tests;
