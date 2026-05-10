-- cook_modules/luarocks_phase2.lua
--
-- SHI-176 Phase 2 build chores. One module function per slice:
--   M.build_lua()        — M2.1: compile vendored Lua sources to lib + bin + headers
--   M.check_exports()    — M2.2: verify cook binary exports lua_*/luaL_* symbols
--   M.bundle_luarocks()  — M2.3: stage vendored luarocks + launcher + default config
--   M.package(version, target)  — M2.5: assemble cook-${ver}-${os}-${arch}.tar.gz
--   M.gate_m2()          — M2.4: hand-rolled C extension + lua-cjson acceptance gate
--
-- Imperative-phase only. Uses cook.exec, fs.*.

local M = {}

function M.build_lua()
    error("M2.1 not yet implemented")
end

function M.check_exports()
    error("M2.2 not yet implemented")
end

function M.bundle_luarocks()
    error("M2.3 not yet implemented")
end

function M.package(version, target)
    error("M2.5 not yet implemented")
end

function M.gate_m2()
    error("M2.4 not yet implemented")
end

return M
