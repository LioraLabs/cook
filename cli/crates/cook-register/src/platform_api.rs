use mlua::prelude::*;

/// Register `cook.platform` table with `os` and `arch` fields.
/// Must be called after the `cook` table exists in globals.
pub fn register_platform_api(lua: &Lua) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let platform = lua.create_table()?;
    platform.set("os", std::env::consts::OS)?;
    platform.set("arch", std::env::consts::ARCH)?;
    cook.set("platform", platform)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_os_is_set() {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        register_platform_api(&lua).unwrap();
        let os: String = lua.load("return cook.platform.os").eval().unwrap();
        assert_eq!(os, std::env::consts::OS);
    }

    #[test]
    fn test_platform_arch_is_set() {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        register_platform_api(&lua).unwrap();
        let arch: String = lua.load("return cook.platform.arch").eval().unwrap();
        assert_eq!(arch, std::env::consts::ARCH);
    }
}
