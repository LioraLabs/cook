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
local LUAROCKS_SRC_DIR = "cli/vendored/luarocks-3.11.0"
local CONFIG_TEMPLATE = "cli/crates/cook-cli/templates/default-rocks-config.lua"
local STAGE = "target/cook-stage"

-- Relocatable shell launcher for the bundled luarocks. Derives COOK_PREFIX
-- from $0 (the launcher's own location) so the install can be moved on disk
-- without baking absolute paths. Sets LUA_PATH so the staged
-- share/luarocks/ tree is on the package search path, exports
-- LUAROCKS_CONFIG to the committed default config template (which itself
-- splices COOK_PREFIX into LUA_BINDIR / LUA_INCDIR / LUA_LIBDIR), then
-- execs the bundled bin/lua against the cook-authored driver staged at
-- share/cook/luarocks-driver.lua.
local LAUNCHER_SCRIPT = [[#!/bin/sh
# Relocatable luarocks launcher for cook bundles.
set -eu
COOK_PREFIX="$(cd "$(dirname "$0")/.." && pwd)"
export COOK_PREFIX
export LUAROCKS_CONFIG="$COOK_PREFIX/share/cook/default-rocks-config.lua"
export LUA_PATH="$COOK_PREFIX/share/?.lua;$COOK_PREFIX/share/?/init.lua;;"
export LUA_CPATH="$COOK_PREFIX/lib/lua/5.4/?.so;;"
exec "$COOK_PREFIX/bin/lua" "$COOK_PREFIX/share/cook/luarocks-driver.lua" "$@"
]]

-- Cook-authored luarocks driver. Mirrors the upstream src/bin/luarocks
-- body (the commands table and the cmd.run_command call), with one
-- difference: --version is pre-empted to print the canonical
-- "LuaRocks <version>" banner. Upstream's --version action prints
-- "<program-path> <version>" via util.this_program, which would leak
-- the staged path into the output and make the version banner depend
-- on where the bundle lives on disk. Cook-side bundles want a stable
-- banner that's relocation-independent.
local LUAROCKS_DRIVER = [[-- Cook-staged LuaRocks driver (M2.3, bundle_luarocks).
-- See cook_modules/luarocks_phase2.lua → LUAROCKS_DRIVER for rationale.

-- Pre-empt --version BEFORE loading cfg: upstream prints "<full-path> <ver>"
-- via util.this_program(), which embeds the staged install path in the
-- output. Cook bundles want a stable "LuaRocks <ver>" banner that's
-- relocation-independent. The version literal is pinned to the
-- vendored luarocks tarball (see cli/vendored/luarocks-3.11.0/README.md);
-- bumping it requires updating both this string and LUAROCKS_SRC_DIR
-- in cook_modules/luarocks_phase2.lua.
for _, a in ipairs(arg) do
    if a == "--version" then
        print("LuaRocks 3.11.0")
        os.exit(0)
    end
end

-- Load cfg first so that the loader knows it is running inside LuaRocks.
local cfg = require("luarocks.core.cfg")

local loader = require("luarocks.loader")
local cmd = require("luarocks.cmd")

local description = "LuaRocks main command-line interface"

local commands = {
    init = "luarocks.cmd.init",
    pack = "luarocks.cmd.pack",
    unpack = "luarocks.cmd.unpack",
    build = "luarocks.cmd.build",
    install = "luarocks.cmd.install",
    search = "luarocks.cmd.search",
    list = "luarocks.cmd.list",
    remove = "luarocks.cmd.remove",
    make = "luarocks.cmd.make",
    download = "luarocks.cmd.download",
    path = "luarocks.cmd.path",
    show = "luarocks.cmd.show",
    new_version = "luarocks.cmd.new_version",
    lint = "luarocks.cmd.lint",
    write_rockspec = "luarocks.cmd.write_rockspec",
    purge = "luarocks.cmd.purge",
    doc = "luarocks.cmd.doc",
    upload = "luarocks.cmd.upload",
    config = "luarocks.cmd.config",
    which = "luarocks.cmd.which",
    test = "luarocks.cmd.test",
}

cmd.run_command(description, commands, "luarocks.cmd.external", ...)
]]

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

-- ── M2.2: check_exports ───────────────────────────────────────────────────
-- Verify the cook executable exports lua_*/luaL_* symbols (proves -rdynamic /
-- -Wl,-export_dynamic landed and works). Sentinels chosen to cover both the
-- raw Lua C API and the auxiliary library, since both are used by C rocks.

-- Sentinels are real exported symbols, not preprocessor-macro names. In
-- Lua 5.4 `lua_pcall` is `#define`'d as `lua_pcallk(...,0,NULL)` and
-- `luaL_checkstring` expands to `luaL_checklstring(...,NULL)`, so the
-- linker only sees the K-variant / l-variant. C rocks compile against
-- the headers and have the macros expanded at their compile time, so
-- verifying the K/l symbols is the load-bearing dlopen check.
local SENTINELS = {
    "lua_pushstring", "lua_pcallk", "lua_close", "lua_newstate",
    "luaL_newstate", "luaL_loadstring", "luaL_openlibs", "luaL_checklstring",
}

local function find_cook_binary()
    -- Prefer release build; fall back to debug.
    for _, p in ipairs({"cli/target/release/cook", "cli/target/debug/cook"}) do
        if fs.exists(p) then return p end
    end
    error("check_exports: no cook binary found at cli/target/{release,debug}/cook; run `cook build` first")
end

-- io.popen is blocked in chore step bodies (CS-0045). cook.platform.os is
-- the canonical host-detection API and is what M.build_lua() uses.
local function check_exports_platform()
    local os_id = cook.platform.os
    if os_id == "linux" or os_id == "macos" then return os_id
    else error("check_exports: unsupported platform: " .. tostring(os_id))
    end
end

function M.check_exports()
    local bin = find_cook_binary()
    local plat = check_exports_platform()
    local nm_cmd
    if plat == "linux" then
        nm_cmd = "nm -D --defined-only " .. bin
    else
        nm_cmd = "nm -gU " .. bin
    end
    print("[check-exports] inspecting " .. bin)

    -- cook.sh runs the command and returns stdout as a string. CS-0045
    -- prohibits io.popen in chore bodies; cook.sh is the supported channel.
    local output = cook.sh(nm_cmd)

    local missing = {}
    for _, sym in ipairs(SENTINELS) do
        -- nm output lines look like "0000000... T lua_pushstring" (Linux) or
        -- "0000000... T _lua_pushstring" (macOS, leading underscore). Match
        -- both forms.
        if not output:match("[%s_]" .. sym .. "$") and not output:match("[%s_]" .. sym .. "\n") then
            table.insert(missing, sym)
        end
    end

    if #missing > 0 then
        io.stderr:write(string.format(
            "[check-exports] FAIL: missing %d/%d sentinel symbol(s):\n",
            #missing, #SENTINELS
        ))
        for _, s in ipairs(missing) do
            io.stderr:write("    " .. s .. "\n")
        end
        io.stderr:write(string.format(
            "[check-exports] cause: cli/.cargo/config.toml is missing the\n"
            .. "    -rdynamic (Linux) / -Wl,-export_dynamic (macOS) link-arg.\n"
        ))
        error("check_exports failed: " .. #missing .. " missing symbol(s)")
    end

    print(string.format("[check-exports] OK — %d/%d sentinels present", #SENTINELS, #SENTINELS))
end

-- ── M2.3: bundle_luarocks ─────────────────────────────────────────────────
-- Stage the vendored LuaRocks 3.11.0 sources into target/cook-stage:
--   - share/luarocks/                     ← cp -a cli/vendored/luarocks-3.11.0/src/luarocks/
--   - share/cook/luarocks-driver.lua      ← cook-authored, from LUAROCKS_DRIVER
--   - share/cook/default-rocks-config.lua ← committed template
--   - bin/luarocks                        ← relocatable shell launcher, from LAUNCHER_SCRIPT
-- Assumes `cook build-lua` has already produced bin/lua and lib/.

function M.bundle_luarocks()
    -- The launcher invokes $COOK_PREFIX/bin/lua, which build_lua produces.
    -- Bail loudly if that didn't happen first.
    if not fs.exists(STAGE .. "/bin/lua") then
        error("bundle_luarocks: " .. STAGE .. "/bin/lua missing — run `cook build-lua` first")
    end

    print("[bundle-luarocks] staging luarocks 3.11.0 → " .. STAGE)

    -- Stage the pure-Lua library tree and the default rocks config
    -- template. Use cp -a to preserve mode + symlinks (the upstream tree is
    -- verbatim from the release tarball). Wipe any prior share/luarocks/
    -- first so this chore is idempotent across re-runs.
    cook.exec(string.format([[
set -euo pipefail
mkdir -p %s/share %s/share/cook
rm -rf %s/share/luarocks
cp -a %s/src/luarocks %s/share/luarocks
cp -a %s %s/share/cook/default-rocks-config.lua
]],
        STAGE, STAGE,
        STAGE,
        LUAROCKS_SRC_DIR, STAGE,
        CONFIG_TEMPLATE, STAGE
    ), 0)

    -- Write the cook-authored driver script (mirrors upstream src/bin/luarocks
    -- body, with --version pre-empted). It's authored cook-side rather than
    -- copied verbatim because upstream prints the program path in its
    -- --version output, which would leak staged install paths.
    fs.write(STAGE .. "/share/cook/luarocks-driver.lua", LUAROCKS_DRIVER)

    -- Write the relocatable launcher and make it executable. fs.write
    -- creates the file with default 0644, so chmod 0755 is required.
    local launcher_path = STAGE .. "/bin/luarocks"
    fs.write(launcher_path, LAUNCHER_SCRIPT)
    cook.exec("chmod 0755 " .. launcher_path, 0)

    print("[bundle-luarocks] staged " .. launcher_path)
end

-- ── M2.5: package ─────────────────────────────────────────────────────────
-- Assemble cook-${version}-${os}-${arch}.tar.gz from target/cook-stage.
-- Replaces M1.2's `cargo xtask package`. The OS+arch substring matches the
-- naming pattern locked by SHI-182.
--
-- This function does NOT order the build-lua / bundle-luarocks chores —
-- the caller (chore "package") is responsible for ordering. The function
-- builds cook itself (idempotent under cargo), copies the cook binary into
-- the already-staged tree, runs check_exports as a final guard, then
-- tarballs and computes a sha256 sibling.

-- io.popen is blocked in chore step bodies (CS-0045); cook.sh is the
-- supported command-stdout-capture channel.
local function host_target()
    local out = cook.sh("rustc -vV")
    return out:match("host: ([^\n]+)")
end

local function target_to_os_arch(triple)
    local os_part, arch_part
    if triple:find("apple%-darwin") then os_part = "darwin"
    elseif triple:find("linux") then os_part = "linux"
    else error("package: unsupported OS in target triple: " .. triple) end
    if triple:find("^x86_64%-") then arch_part = "amd64"
    elseif triple:find("^aarch64%-") then arch_part = "arm64"
    else error("package: unsupported arch in target triple: " .. triple) end
    return os_part, arch_part
end

function M.package(version, target)
    if not version or version == "" then
        error("package: missing VERSION (pass --set VERSION=vX.Y.Z)")
    end
    target = (target ~= nil and target ~= "") and target or host_target()
    local os_part, arch_part = target_to_os_arch(target)
    local stage_name = string.format("cook-%s-%s-%s", version, os_part, arch_part)
    local tarball = string.format("cli/target/dist/%s.tar.gz", stage_name)

    print(string.format("[package] version=%s target=%s -> %s", version, target, tarball))

    -- Build cook itself (caller may have done this; cargo's incremental
    -- build makes this idempotent).
    cook.exec(string.format(
        "cd cli && cargo build --release --target=%s -p cook-cli",
        target
    ), 0)

    -- build-lua + bundle-luarocks must have produced the stage tree already
    -- (chore deps wire this); verify before proceeding.
    if not fs.exists(STAGE .. "/lib") or not fs.exists(STAGE .. "/share/luarocks") then
        error("package: stage tree missing pieces; run `cook build-lua bundle-luarocks` first")
    end

    -- Copy the freshly-built cook binary into the stage.
    local built_bin = string.format("cli/target/%s/release/cook", target)
    if not fs.exists(built_bin) then
        error("package: cargo did not produce " .. built_bin)
    end
    cook.exec(string.format("cp %s %s/bin/cook", built_bin, STAGE), 0)
    cook.exec("chmod 0755 " .. STAGE .. "/bin/cook", 0)

    -- VERSION marker file (Phase 1 contract).
    fs.write(STAGE .. "/VERSION", version .. "\n")

    -- Final symbol-export guard. Re-uses the M2.2 sentinel set so a
    -- regression in cli/.cargo/config.toml is caught at packaging time
    -- rather than at first dlopen of a C rock by a downstream user.
    M.check_exports()

    -- Rename the wrapping dir for the tarball, then tar. We copy rather
    -- than relying on tar's --transform because GNU tar's --transform is
    -- not available on macOS BSD tar. sha256sum (Linux coreutils) falls
    -- back to shasum (BSD/macOS) so the same shell pipeline works on
    -- both Phase 1 hosts.
    cook.exec(string.format([[
set -euo pipefail
mkdir -p cli/target/dist
rm -rf cli/target/dist/%s
cp -a target/cook-stage cli/target/dist/%s
cd cli/target/dist
tar -czf %s.tar.gz %s
sha256sum %s.tar.gz > %s.tar.gz.sha256 2>/dev/null \
  || shasum -a 256 %s.tar.gz > %s.tar.gz.sha256
rm -rf %s
]],
        stage_name, stage_name,
        stage_name, stage_name,
        stage_name, stage_name,
        stage_name, stage_name,
        stage_name
    ), 0)

    print(string.format("[package] built %s (+ .sha256)", tarball))
end

function M.gate_m2()
    error("M2.4 not yet implemented")
end

return M
