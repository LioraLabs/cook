use mlua::prelude::*;

/// Register `cook.platform` table with `os` and `arch` fields.
/// Takes a reference to the `cook` table to add the platform subtable to.
pub fn register_platform_api(lua: &Lua, cook: &LuaTable) -> LuaResult<()> {
    let platform = lua.create_table()?;
    platform.set("os", std::env::consts::OS)?;
    platform.set("arch", std::env::consts::ARCH)?;
    cook.set("platform", platform)?;
    Ok(())
}
