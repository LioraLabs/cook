//! The read-only `var` global (§5.3.1, CS-0172), sealed the same way in both
//! phases.
//!
//! A declared variable's value is a cache determinant of every unit that
//! reads it. So the surface that exposes it must route reads through a check
//! and refuse writes outright: a step that could reassign `var.X` would be a
//! step redefining what it was keyed on, and the cache would go on believing
//! the old value.
//!
//! The policy has three parts and all three matter. Reads go through
//! `__index` rather than being handed the store, so the check runs on every
//! access. Writes hit a `__newindex` that raises. And the metatable is hidden
//! with `__metatable = false`, without which `setmetatable(var, {})` lifts
//! both guards off in one line.
//!
//! Until COOK-439 that was built twice, once per VM. The constitution's
//! duplicate-literal rule reported it as `__metatable` and `__newindex`
//! written in two crates and conceded that those are Lua's own names, which
//! cannot drift — the waiver's real subject was the policy underneath, and
//! that is what has one implementation now.
//!
//! # Why the refusal SENTENCE is a parameter and not shared law
//!
//! The two phases refuse for the same reason and offer different advice,
//! because different advice is available. At register phase the author can
//! still set the variable — in a `config` block, or with `--set` — and the
//! message says so. Once execute-phase Lua is running, config blocks have
//! already run and recipes are already registered against the value, so the
//! same suggestion would be wrong. One sentence for both phases would have to
//! be wrong in one of them, so the sentence stays at the call site and the
//! seal is what is shared. This is COOK-422's `caller_line_in_source` shape:
//! parameterise over the one thing that genuinely differs, and leave the
//! phase-specific spelling with the phase.

/// Install the read-only `var` global.
///
/// `index` answers a read: the register VM checks the declared keyset and
/// raises on an undeclared name, the execute VM returns the current unit's
/// resolved value. `refuse` renders the phase's own message for a write.
///
/// The proxy table itself is empty; everything reachable through it comes
/// from `index`, which is what keeps the check on the path.
pub fn install_var_proxy<I, R>(lua: &mlua::Lua, index: I, refuse: R) -> mlua::Result<()>
where
    I: Fn(&mlua::Lua, String) -> mlua::Result<mlua::Value> + mlua::MaybeSend + 'static,
    R: Fn(&str) -> String + mlua::MaybeSend + 'static,
{
    let proxy = lua.create_table()?;
    let meta = lua.create_table()?;

    meta.set(
        "__index",
        lua.create_function(move |lua, (_proxy, name): (mlua::Value, String)| index(lua, name))?,
    )?;

    meta.set(
        "__newindex",
        lua.create_function(
            move |_,
                  (_proxy, name, _value): (mlua::Value, String, mlua::Value)|
                  -> mlua::Result<()> { Err(mlua::Error::RuntimeError(refuse(&name))) },
        )?,
    )?;

    // Hide the metatable so the two guards above cannot be lifted off with
    // `setmetatable`. Without this the whole seal is one line from gone.
    meta.set("__metatable", false)?;

    proxy.set_metatable(Some(meta));
    // The global's name is `cook_contracts::registration::VAR_GLOBAL_NAME`,
    // not a spelling of our own: the config sandbox exposes the WRITE sink
    // under it and `cook_contracts::lua_scan` scans for reads of it to compute
    // a unit's declared-variable determinants. A rename that reached the two
    // VMs and not the scanner would leave units keyed on nothing.
    lua.globals()
        .set(cook_contracts::registration::VAR_GLOBAL_NAME, proxy)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/var_proxy_tests.rs"]
mod tests;
