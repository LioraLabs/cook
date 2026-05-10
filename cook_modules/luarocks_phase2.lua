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

-- ── M2.1: build_lua ───────────────────────────────────────────────────────
-- Compile cli/vendored/lua-5.4.7/ into target/cook-stage/{lib,bin,include}.
-- Per-OS branch handles MACOSX_DEPLOYMENT_TARGET=11.0 + install_name_tool
-- on macOS.

local function platform()
    -- cook.platform.os: "linux", "macos" (from std::env::consts::OS).
    -- io.popen("uname -s") is blocked in chore step bodies (CS-0045);
    -- cook.platform.* is the canonical host-detection API.
    local os_id = cook.platform.os
    if os_id == "linux" then
        return "linux"
    elseif os_id == "macos" then
        return "macos"
    else
        error("build_lua: unsupported platform: " .. tostring(os_id))
    end
end

local LUA_SRC_DIR = "cli/vendored/lua-5.4.7"
local STAGE = "target/cook-stage"

-- All .c files except lua.c and luac.c form the library translation units.
-- These names mirror Lua's own Makefile (LIB_O + CORE_O minus lua.o/luac.o).
local LIB_C = {
    "lapi.c", "lcode.c", "lctype.c", "ldebug.c", "ldo.c", "ldump.c",
    "lfunc.c", "lgc.c", "llex.c", "lmem.c", "lobject.c", "lopcodes.c",
    "lparser.c", "lstate.c", "lstring.c", "ltable.c", "ltm.c", "lundump.c",
    "lvm.c", "lzio.c",
    "lauxlib.c", "lbaselib.c", "lcorolib.c", "ldblib.c", "liolib.c",
    "lmathlib.c", "loadlib.c", "loslib.c", "lstrlib.c", "ltablib.c",
    "lutf8lib.c", "linit.c",
}

-- luac links against the library object set plus luac.c.
local LUAC_C = "luac.c"
-- lua links against the library object set plus lua.c.
local LUA_C = "lua.c"

function M.build_lua()
    local plat = platform()
    print("[build-lua] platform: " .. plat)

    -- Fresh staging dirs
    cook.exec(string.format([[
set -euo pipefail
mkdir -p %s/lib %s/bin %s/include/lua5.4 target/build-lua-obj
]], STAGE, STAGE, STAGE), 0)

    -- Common compile flags
    local cflags = "-O2 -fPIC -DLUA_COMPAT_5_3 -I" .. LUA_SRC_DIR
    if plat == "linux" then
        cflags = cflags .. " -DLUA_USE_LINUX"
    else
        cflags = cflags .. " -DLUA_USE_MACOSX"
    end

    -- Compile every translation unit to .o
    local sources = {}
    for _, f in ipairs(LIB_C) do table.insert(sources, f) end
    table.insert(sources, LUA_C)
    table.insert(sources, LUAC_C)

    local env_prefix = ""
    if plat == "macos" then
        env_prefix = "MACOSX_DEPLOYMENT_TARGET=11.0 "
    end

    for _, src in ipairs(sources) do
        local obj = "target/build-lua-obj/" .. src:gsub("%.c$", ".o")
        local cmd = string.format(
            "%scc %s -c %s/%s -o %s",
            env_prefix, cflags, LUA_SRC_DIR, src, obj
        )
        cook.exec(cmd, 0)
    end

    -- List of library object files for linking
    local lib_objs = {}
    for _, f in ipairs(LIB_C) do
        table.insert(lib_objs, "target/build-lua-obj/" .. f:gsub("%.c$", ".o"))
    end
    -- luac additionally links lparser/lcode-using helpers; Lua's Makefile
    -- defines LUAC_T as the same library set, so reusing lib_objs is correct.
    local lib_objs_str = table.concat(lib_objs, " ")

    if plat == "linux" then
        -- Shared library
        cook.exec(string.format(
            "cc -shared -o %s/lib/liblua5.4.so %s -lm -ldl",
            STAGE, lib_objs_str
        ), 0)
        -- lua interpreter binary (statically links the library objects;
        -- exports symbols via -Wl,-E so embedded scripts can dlopen rocks)
        cook.exec(string.format(
            "cc -Wl,-E -o %s/bin/lua target/build-lua-obj/lua.o %s -lm -ldl",
            STAGE, lib_objs_str
        ), 0)
        cook.exec(string.format(
            "cc -o %s/bin/luac target/build-lua-obj/luac.o %s -lm -ldl",
            STAGE, lib_objs_str
        ), 0)
    else
        -- macOS shared library
        cook.exec(string.format(
            "%scc -dynamiclib -o %s/lib/liblua5.4.dylib %s -lm",
            env_prefix, STAGE, lib_objs_str
        ), 0)
        -- Set install_name so dependents resolve the dylib relative to the
        -- cook executable rather than the build slave's absolute path.
        cook.exec(string.format(
            "install_name_tool -id @executable_path/../lib/liblua5.4.dylib %s/lib/liblua5.4.dylib",
            STAGE
        ), 0)
        cook.exec(string.format(
            "%scc -o %s/bin/lua target/build-lua-obj/lua.o %s -lm",
            env_prefix, STAGE, lib_objs_str
        ), 0)
        cook.exec(string.format(
            "%scc -o %s/bin/luac target/build-lua-obj/luac.o %s -lm",
            env_prefix, STAGE, lib_objs_str
        ), 0)
    end

    -- Headers: copy verbatim from vendored sources
    for _, h in ipairs({"lua.h", "lauxlib.h", "lualib.h", "luaconf.h"}) do
        cook.exec(string.format(
            "cp %s/%s %s/include/lua5.4/%s",
            LUA_SRC_DIR, h, STAGE, h
        ), 0)
    end

    print("[build-lua] artifacts staged at " .. STAGE)
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
