//! `cook.platform.*` — host OS / architecture identifiers (§6 cook table).
//!
//! Phase: Both. Values are sourced from `std::env::consts` at VM
//! registration time and never change for the life of the VM.

use mlua::prelude::*;

/// Register `cook.platform = { os = ..., arch = ... }` on the supplied
/// `cook` table.
///
/// The caller passes the `cook` table directly so this function does
/// not assume any particular global-namespace layout. `cook-register`
/// fetches `cook` from globals after its own setup; `cook-luaotp`
/// builds `cook` locally per worker and passes it in before assigning
/// it to globals.
pub fn register_platform_api(lua: &Lua, cook: &LuaTable) -> LuaResult<()> {
    let platform = lua.create_table()?;
    platform.set("os", std::env::consts::OS)?;
    platform.set("arch", std::env::consts::ARCH)?;
    cook.set("platform", platform)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/platform_api_tests.rs"]
mod tests;
