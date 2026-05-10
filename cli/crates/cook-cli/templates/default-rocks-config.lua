-- Default LuaRocks config for the bundled cook luarocks launcher.
--
-- The launcher script (cook_modules/dist.lua → LAUNCHER_SCRIPT)
-- exports COOK_PREFIX as the absolute path of the staged install root
-- (the parent of bin/luarocks). LuaRocks loads this file via the
-- LUAROCKS_CONFIG env var, and we splice COOK_PREFIX into the variables
-- table so every path resolved by luarocks is anchored at the bundle's
-- actual on-disk location — making the install relocatable.
--
-- LuaRocks loads config files in a SANDBOXED environment: the global
-- `os` table is NOT exposed; the only env-lookup hook is `os_getenv`
-- (see luarocks.core.cfg.env_for_config_file). Use that instead of
-- `os.getenv` here.

rocks_servers = {}
lua_interpreter = "lua"

variables = {
    LUA_BINDIR = os_getenv("COOK_PREFIX") .. "/bin",
    LUA_INCDIR = os_getenv("COOK_PREFIX") .. "/include/lua5.4",
    LUA_LIBDIR = os_getenv("COOK_PREFIX") .. "/lib",
}
