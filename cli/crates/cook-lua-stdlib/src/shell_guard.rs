//! Lua-side shell escape-hatch guards (CS-0045).
//!
//! `os.execute` and `io.popen` let Lua code run arbitrary shell text
//! that bypasses Cook's `cook.sh` working-directory rooting and the
//! cache fingerprint that records the command. In a hermetic
//! `cook`/`test`/`chore` step body, that is exactly the surface the
//! sandbox is meant to close: the captured `lua_code` would not
//! reflect the side-effects of an `os.execute("rm -rf /")`.
//!
//! CS-0135 removed `plate`, and with it the last step kind that ran
//! unconfined; this paragraph went on naming plate bodies as the user's
//! escape hatch afterwards. The escape hatch is a `chore`'s raw shell
//! steps (§25.9), which is what the diagnostics below already tell the
//! user to reach for.
//!
//! The guard is implemented by replacing the offending entries with
//! Lua functions that consult a [`SandboxSource`] on each call. When
//! the source resolves to `Confined`, the call raises a Lua runtime
//! error carrying the CS-0045 diagnostic tag; when it resolves to
//! `Off`, the call delegates to the original implementation. This
//! matches the live-source pattern used by `fs.*`: the same VM is
//! reused across many work items and the active policy may differ
//! per item (CS-0017 + CS-0045).

use mlua::prelude::*;

use crate::sandbox::SandboxSource;

/// Replace `os.execute` and `io.popen` on `lua` with sandbox-aware
/// shims that consult `sandbox` on each call.
///
/// MUST be called *after* the standard Lua libraries are loaded
/// (`mlua::Lua::new()` and `unsafe_new()` both load them by default)
/// so the original entries exist to be wrapped.
pub fn install_shell_escape_guards(lua: &Lua, sandbox: SandboxSource) -> LuaResult<()> {
    let os: LuaTable = match lua.globals().get::<LuaValue>("os")? {
        LuaValue::Table(t) => t,
        _ => return Ok(()),
    };
    let original_execute: Option<LuaFunction> = os.get("execute").ok();
    let sb_exec = sandbox.clone();
    os.set(
        "execute",
        lua.create_function(move |_, cmd: Option<String>| {
            let policy = sb_exec.resolve();
            if !policy.shell_escape_hatches_enabled() {
                return Err(mlua::Error::runtime(
                    "os.execute: Lua-side shell escape hatch is \
                     disabled in cook/test/chore step bodies; \
                     use cook.sh (which runs with the recipe's \
                     working_dir and is recorded in the unit's \
                     command_hash) or move the call to a `chore`",
                ));
            }
            // Off: delegate to the original implementation if we have
            // it. (Lua's os.execute returns multiple values; we forward
            // whatever it returned.)
            if let Some(orig) = &original_execute {
                let v: mlua::MultiValue = match cmd {
                    Some(c) => orig.call(c)?,
                    None => orig.call(())?,
                };
                Ok(v)
            } else {
                Ok(mlua::MultiValue::new())
            }
        })?,
    )?;

    let io: LuaTable = match lua.globals().get::<LuaValue>("io")? {
        LuaValue::Table(t) => t,
        _ => return Ok(()),
    };
    let original_popen: Option<LuaFunction> = io.get("popen").ok();
    let sb_popen = sandbox.clone();
    io.set(
        "popen",
        lua.create_function(move |_, args: mlua::MultiValue| {
            let policy = sb_popen.resolve();
            if !policy.shell_escape_hatches_enabled() {
                return Err(mlua::Error::runtime(
                    "io.popen: Lua-side shell escape hatch is disabled \
                     in cook/test/chore step bodies; use \
                     cook.sh (which runs with the recipe's working_dir \
                     and is recorded in the unit's command_hash) or move \
                     the call to a `chore` (whose raw shell steps run unsandboxed)",
                ));
            }
            if let Some(orig) = &original_popen {
                let v: mlua::MultiValue = orig.call(args)?;
                Ok(v)
            } else {
                Ok(mlua::MultiValue::new())
            }
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
#[path = "tests/shell_guard_tests.rs"]
mod tests;
