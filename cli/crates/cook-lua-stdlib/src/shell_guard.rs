//! Lua-side shell escape-hatch guards (CS-0045).
//!
//! `os.execute` and `io.popen` let Lua code run arbitrary shell text
//! that bypasses Cook's `cook.sh` working-directory rooting and the
//! cache fingerprint that records the command. In a hermetic
//! `cook`/`test`/`chore` step body, that is exactly the surface the
//! sandbox is meant to close: the captured `lua_code` would not
//! reflect the side-effects of an `os.execute("rm -rf /")`.
//!
//! Plate step bodies are explicitly the user's escape hatch for
//! ad-hoc shell, so they remain untouched.
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
                     disabled in cook/test/chore step bodies (CS-0045); \
                     use cook.sh (which runs with the recipe's \
                     working_dir and is recorded in the unit's \
                     command_hash) or move the call to a `plate` step",
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
                     in cook/test/chore step bodies (CS-0045); use \
                     cook.sh (which runs with the recipe's working_dir \
                     and is recorded in the unit's command_hash) or \
                     move the call to a `plate` step",
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
mod tests {
    use super::*;
    use crate::sandbox::SandboxPolicy;
    use std::sync::{Arc, Mutex};

    /// Confined VM: os.execute MUST raise a Lua error mentioning
    /// CS-0045. We don't run the original — the guard is the only
    /// caller of os.execute on the test path.
    #[test]
    fn confined_os_execute_raises() {
        let lua = Lua::new();
        install_shell_escape_guards(
            &lua,
            SandboxSource::confined(std::path::PathBuf::from("/proj")),
        )
        .unwrap();
        let err = lua
            .load(r#"os.execute("echo escape")"#)
            .exec()
            .unwrap_err()
            .to_string();
        assert!(err.contains("CS-0045"), "missing CS-0045 tag: {err}");
        assert!(err.contains("os.execute"), "missing api name: {err}");
    }

    /// Off VM: os.execute MUST be a no-op pass-through. Use a harmless
    /// command (`true` on POSIX) so the test does not depend on the
    /// host having any specific binary at a specific path.
    #[test]
    fn off_os_execute_passes_through() {
        let lua = Lua::new();
        install_shell_escape_guards(&lua, SandboxSource::off()).unwrap();
        // `true` exits 0; we don't assert on the return value because
        // mlua's coercion of multi-return values varies by version.
        // The point is: it MUST NOT raise.
        lua.load(r#"os.execute("true")"#).exec().unwrap();
    }

    /// io.popen behaves the same way under Confined.
    #[test]
    fn confined_io_popen_raises() {
        let lua = Lua::new();
        install_shell_escape_guards(
            &lua,
            SandboxSource::confined(std::path::PathBuf::from("/proj")),
        )
        .unwrap();
        let err = lua
            .load(r#"return io.popen("echo x")"#)
            .exec()
            .unwrap_err()
            .to_string();
        assert!(err.contains("CS-0045"), "missing CS-0045 tag: {err}");
        assert!(err.contains("io.popen"), "missing api name: {err}");
    }

    /// Live source observes per-item policy changes. The same VM
    /// flips from rejecting to permitting based on slot mutation.
    #[test]
    fn live_source_flips_per_call() {
        let lua = Lua::new();
        let slot = Arc::new(Mutex::new(SandboxPolicy::Confined {
            project_root: std::path::PathBuf::from("/proj"),
        }));
        install_shell_escape_guards(&lua, SandboxSource::Live(Arc::clone(&slot))).unwrap();

        // First call: confined, MUST raise.
        let err = lua
            .load(r#"os.execute("true")"#)
            .exec()
            .unwrap_err()
            .to_string();
        assert!(err.contains("CS-0045"));

        // Flip to Off; same VM, same closure.
        *slot.lock().unwrap() = SandboxPolicy::Off;
        lua.load(r#"os.execute("true")"#).exec().unwrap();
    }
}
