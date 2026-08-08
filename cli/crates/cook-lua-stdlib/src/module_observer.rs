//! What a unit's Lua body actually loaded (§12.3.6, CS-0204).
//!
//! A module is source. A body that loads one runs the module's code, so the
//! module's content decides what the body does — and until CS-0204 nothing
//! recorded that, which made a module edit invisible to the cache: the unit
//! was served its old output with no rebuild and no `cook why` attribution.
//!
//! The fix has to observe rather than predict. Which modules a body loads is
//! a fact about the RUN, not about the declaration: the name can be computed,
//! the load can sit behind a branch, and a multi-file rock pulls in submodules
//! its caller never names. So the loader reports what it resolved, and the
//! engine keys the unit on the content of exactly those files.
//!
//! # Why paths, not a directory
//!
//! The sink holds the RESOLVED PATH of each load. Keying on "the
//! `cook_modules/` directory" would have been cheaper and is wrong twice
//! over: it makes every unit in the project depend on every module in it
//! (one edit busts everything), and it hard-codes a layout that is already
//! moving (COOK-431 relocates rocks to `.cookmodules/`). A rule written over
//! resolved paths survives that move untouched.
//!
//! # Two doors, one sink
//!
//! `cook.load_module` is not the only way module code enters a body.
//! Lua's own `require` is the other, and it is the one a multi-file rock uses
//! internally (`cook_cc/init.lua` requiring `cook_cc.toolchain`) and the only
//! one that can reach a native `.so` at all — `module_candidates` probes four
//! `.lua` paths and nothing else. Both doors record into the same sink, so
//! "every module actually loaded by the unit" means every module, not every
//! module that happened to come through the front.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use mlua::prelude::*;

/// The set of module files loaded since the last [`ModuleObserver::take`].
///
/// Cloning shares the sink: the loader, the `require` wrapper, and the worker
/// that drains it per work item all hold the same one. A `BTreeSet` because
/// the same module is routinely loaded twice within a unit (a memo hit still
/// records) and because the drained order must not depend on load order — the
/// set is folded into a cache key, and a key that moved when two independent
/// loads raced would be a false rebuild.
#[derive(Clone, Debug, Default)]
pub struct ModuleObserver {
    loaded: Arc<Mutex<BTreeSet<PathBuf>>>,
}

impl ModuleObserver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Note that `path`'s content took part in the work now running.
    ///
    /// Deliberately infallible and silent: an observation that could fail
    /// would need an error path through a Lua callback whose only honest
    /// recovery is to fail the unit, and a poisoned mutex here means the
    /// worker is already unrecoverable.
    pub fn record(&self, path: &Path) {
        if let Ok(mut set) = self.loaded.lock() {
            set.insert(path.to_path_buf());
        }
    }

    /// Drain the sink and return what it held, sorted.
    ///
    /// Draining rather than reading is what keeps one worker VM's units
    /// independent: the VM is reused across items (CS-0017), and an item that
    /// inherited the previous item's set would be keyed on a module it never
    /// loaded.
    pub fn take(&self) -> Vec<PathBuf> {
        match self.loaded.lock() {
            Ok(mut set) => std::mem::take(&mut *set).into_iter().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Discard whatever the sink holds. Called before a work item begins, so
    /// that anything recorded during VM setup is not attributed to it.
    pub fn clear(&self) {
        if let Ok(mut set) = self.loaded.lock() {
            set.clear();
        }
    }
}

/// Wrap the global `require` so every module it loads is observed.
///
/// # What is recorded, and why not simply "what `searchpath` says"
///
/// `require` memoises in `package.loaded`, and a worker VM outlives the work
/// item (CS-0017): `package.loaded` is never cleared between items while
/// `package.path` IS recomposed per item. So on the second item, from a
/// different Cookfile directory, `require("helper")` hands back the FIRST
/// directory's module while `package.searchpath` resolves the SECOND
/// directory's file. Recording the search result would key the unit on a file
/// it never ran, and leave edits to the file it did run invisible — the exact
/// defect this change exists to close, reintroduced by the fix for it.
///
/// So the wrapper distinguishes the two cases the way `cook.load_module` does
/// (§12.3.2, and `module_loader`'s `paths` map):
///
/// * `package.loaded[name]` already set — nothing is read from disk, and what
///   ran is whatever this VM loaded earlier. The path memoised at THAT load is
///   recorded. A name loaded before the wrapper existed, or with no file behind
///   it at all (`string`, a `package.preload` entry), has no memo and records
///   nothing, which is correct: there is no file whose content could move.
/// * not yet loaded — the original `require` runs, and on success the file is
///   resolved through `package.searchpath` against the live `package.path` and
///   `package.cpath`. That is the same two strings Lua's own searchers just
///   used, so the recorded path is the one Lua picked rather than a second
///   implementation of the search order.
///
/// The memo also removes the per-call cost: a repeated `require` inside a hot
/// function pays one table lookup instead of re-probing every path template.
///
/// A failed require records nothing. Idempotent per VM: a second install would
/// nest wrappers on a long-lived worker VM, so it is a no-op.
pub fn install_require_observer(lua: &Lua, observer: ModuleObserver) -> LuaResult<()> {
    // In the registry, not in `_G`: the register VM's globals are the Cookfile
    // author's namespace, and a bookkeeping flag parked there is a name they
    // can read, shadow, or trip over.
    const INSTALLED_FLAG: &str = "__cook_require_observed";

    if lua.named_registry_value::<LuaValue>(INSTALLED_FLAG)? != LuaValue::Nil {
        return Ok(());
    }
    let globals = lua.globals();
    let require_fn = cook_contracts::module_binding::REQUIRE_FN;
    let original: LuaFunction = match globals.get::<LuaValue>(require_fn)? {
        LuaValue::Function(f) => f,
        // No `require` in this VM: nothing to wrap, and nothing can arrive
        // through the door that does not exist.
        _ => return Ok(()),
    };

    // name -> the file this VM actually loaded it from. Per-VM, like the
    // `package.loaded` table it shadows.
    let memo: Rc<RefCell<BTreeMap<String, PathBuf>>> = Rc::default();

    let wrapper = lua.create_function(move |lua, args: LuaMultiValue| {
        let name: Option<String> = args
            .iter()
            .next()
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let already_loaded = match &name {
            Some(n) => is_loaded(lua, n)?,
            None => false,
        };
        let result = original.call::<LuaMultiValue>(args)?;
        if let Some(name) = name {
            if already_loaded {
                if let Some(path) = memo.borrow().get(&name) {
                    observer.record(path);
                }
            } else if let Some(path) = searchpath_hit(lua, &name)? {
                observer.record(&path);
                memo.borrow_mut().insert(name, path);
            }
        }
        Ok(result)
    })?;

    globals.set(require_fn, wrapper)?;
    lua.set_named_registry_value(INSTALLED_FLAG, true)?;
    Ok(())
}

/// Is `name` already in `package.loaded`? Asked BEFORE the call, because
/// afterwards every successful require answers yes.
fn is_loaded(lua: &Lua, name: &str) -> LuaResult<bool> {
    let pkg: LuaTable = match lua.globals().get::<LuaValue>("package")? {
        LuaValue::Table(t) => t,
        _ => return Ok(false),
    };
    let loaded: LuaTable = match pkg.get::<LuaValue>("loaded")? {
        LuaValue::Table(t) => t,
        _ => return Ok(false),
    };
    Ok(loaded.get::<LuaValue>(name)? != LuaValue::Nil)
}

/// Where Lua would find `name`: `package.path` first, then `package.cpath`.
///
/// Returns the first hit. `package.searchpath` returns `nil, err` on a miss,
/// which is not an error condition here — a stdlib name simply has no file.
/// The `cpath` arm is how a native module (`.so`/`.dll`) is observed at all:
/// `cook.load_module` probes four `.lua` candidates and can never reach one.
fn searchpath_hit(lua: &Lua, name: &str) -> LuaResult<Option<PathBuf>> {
    let pkg: LuaTable = match lua.globals().get::<LuaValue>("package")? {
        LuaValue::Table(t) => t,
        _ => return Ok(None),
    };
    let searchpath: LuaFunction = match pkg.get::<LuaValue>("searchpath")? {
        LuaValue::Function(f) => f,
        _ => return Ok(None),
    };
    for field in ["path", "cpath"] {
        let search: String = match pkg.get::<LuaValue>(field)? {
            LuaValue::String(s) => s.to_str()?.to_string(),
            _ => continue,
        };
        let found: LuaValue = searchpath
            .call::<LuaMultiValue>((name, search))?
            .into_iter()
            .next()
            .unwrap_or(LuaValue::Nil);
        if let LuaValue::String(s) = found {
            return Ok(Some(PathBuf::from(s.to_str()?.to_string())));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "tests/module_observer_tests.rs"]
mod tests;
