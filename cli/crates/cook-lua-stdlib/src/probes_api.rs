//! `cook.probes.{get,set,scope}` — the same table on both VMs, over two
//! different stores.
//!
//! Phase: Both (§6.3.4, §24.4.3). What a `cook.probes` call MEANS differs by
//! phase and is meant to: at register phase `get` consults the member-source
//! pre-pass and then the active module's persistent cache, and `set` writes
//! that cache; at execute phase `get` reads the per-run probe-value store and
//! a key that was never materialised is a hard error (CS-0152), while `set` is
//! deprecated and raises (CS-0074).
//!
//! Everything AROUND those two operations is identical and was copied: the
//! table, the two fields, `scope(label)` returning a view, the §24.4.3 label
//! validation, and the `label:key` prefixing that view applies. The
//! constitution's clone rule found it as
//! `cook-execute/src/pool.rs == cook-register/src/module_loader.rs` and called
//! it "already divergent, one installs a setter that writes and the other one
//! that raises a deprecation error".
//!
//! That reading is worth correcting, because it changes what the fix is. The
//! setter difference is not drift: it is CS-0074, specified, and correct in
//! both phases. Nothing here had rotted. The finding is the other way round —
//! the nine tenths that must agree were copied, so a fix to the scoped key
//! format or the label rule would land in one phase and not the other, while
//! the one tenth that is supposed to differ was invisible, buried in the copy
//! rather than stated. Passing the two operations in as arguments makes the
//! agreement structural and the difference legible in the caller's own code.

use mlua::prelude::*;
use std::rc::Rc;

/// Reads a probe value for an already-scope-resolved key.
type ProbeGet = dyn Fn(&Lua, &str) -> LuaResult<LuaValue>;

/// Writes (or refuses) a probe value for an already-scope-resolved key.
type ProbeSet = dyn Fn(&Lua, &str, &LuaValue) -> LuaResult<()>;

/// Install `cook.probes` and the `cook.cache` rename stub that always
/// accompanies it.
///
/// `get` and `set` are handed the FULL key: this function owns
/// `scope(label)`, so a scoped call arrives already prefixed through
/// [`cook_contracts::probe_key::scoped_key`] and neither store has to know
/// that scopes exist. Label validation likewise happens here, once, through
/// the shared §24.4.3 rule — the two VMs already called the same
/// `probe_key::scope_label_error`, and this is what stops the NEXT rule from
/// being added to only one of them.
pub fn install_probes_api<G, S>(lua: &Lua, cook: &LuaTable, get: G, set: S) -> LuaResult<()>
where
    G: Fn(&Lua, &str) -> LuaResult<LuaValue> + 'static,
    S: Fn(&Lua, &str, &LuaValue) -> LuaResult<()> + 'static,
{
    let get: Rc<ProbeGet> = Rc::new(get);
    let set: Rc<ProbeSet> = Rc::new(set);

    let probes = lua.create_table()?;

    let get_unscoped = Rc::clone(&get);
    probes.set(
        "get",
        lua.create_function(move |lua, key: String| get_unscoped(lua, &key))?,
    )?;

    let set_unscoped = Rc::clone(&set);
    probes.set(
        "set",
        lua.create_function(move |lua, (key, value): (String, LuaValue)| {
            set_unscoped(lua, &key, &value)
        })?,
    )?;

    // `cook.probes.scope(label)` → a view whose get/set are the same two
    // operations on `label:key`. The view is built per call rather than
    // memoised: a label is cheap, and a cached view would outlive the store
    // handle in the phase whose store is per-unit.
    let get_scoped = Rc::clone(&get);
    let set_scoped = Rc::clone(&set);
    probes.set(
        "scope",
        lua.create_function(move |lua, label: String| {
            if let Some(msg) = cook_contracts::probe_key::scope_label_error(&label) {
                return Err(LuaError::runtime(msg));
            }
            let view = lua.create_table()?;

            let get_view = Rc::clone(&get_scoped);
            let label_get = label.clone();
            view.set(
                "get",
                lua.create_function(move |lua, key: String| {
                    get_view(lua, &cook_contracts::probe_key::scoped_key(&label_get, &key))
                })?,
            )?;

            let set_view = Rc::clone(&set_scoped);
            let label_set = label.clone();
            view.set(
                "set",
                lua.create_function(move |lua, (key, value): (String, LuaValue)| {
                    set_view(
                        lua,
                        &cook_contracts::probe_key::scoped_key(&label_set, &key),
                        &value,
                    )
                })?,
            )?;

            Ok(view)
        })?,
    )?;

    cook.set("probes", probes)?;

    // `cook.cache.*` was renamed to `cook.probes.*` in v1.0 (CS-0136). Both
    // VMs installed the stub immediately after building the table above, so
    // it belongs to the same install rather than to each caller's memory.
    crate::module_loader::install_renamed_cache_stub(lua, cook)
}

#[cfg(test)]
#[path = "tests/probes_api_tests.rs"]
mod tests;
