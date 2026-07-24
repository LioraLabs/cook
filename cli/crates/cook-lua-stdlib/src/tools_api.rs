//! `cook.tools.id(name)` — canonical tool identity (CS-0158, COOK-277).
//!
//! Both-phase API: a module that seals on a toolchain needs a
//! machine-independent identity to fold into its probe VALUE — the resolved
//! binary's content hash — without Lua-side hashing of 60MB binaries. The
//! implementation is backed by cook-fingerprint's per-run tool-hash memo, so
//! a call here never re-hashes a binary the fingerprint pass already read.
//!
//! `cook.tools.id("gcc")` returns `{ hash = "<lowercase-hex sha256>",
//! path = "/usr/bin/gcc" }`, or `nil` when the name does not resolve on
//! PATH. The `hash` field is the identity a module folds into a sealed
//! value; `path` is location metadata for invocation and MUST NOT be folded
//! into any sealed value (§12.7.5 — a machine-specific path in a sealed
//! value defeats cross-machine reuse).

use mlua::{Lua, Result as LuaResult, Table as LuaTable};

pub fn register_tools_api(lua: &Lua, cook: &LuaTable) -> LuaResult<()> {
    let tools = lua.create_table()?;
    let id_fn = lua.create_function(|lua, name: String| {
        match cook_fingerprint::tool_identity(&name) {
            Some((hash, path)) => {
                let t = lua.create_table()?;
                t.set("hash", hash)?;
                t.set("path", path)?;
                Ok(mlua::Value::Table(t))
            }
            None => Ok(mlua::Value::Nil),
        }
    })?;
    tools.set("id", id_fn)?;
    cook.set("tools", tools)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/tools_api_tests.rs"]
mod tests;
