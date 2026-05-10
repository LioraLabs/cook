-- cook_modules/dist.lua
--
-- Build the redistributable cook tarball: lua + luarocks + cook binary,
-- staged into target/cook-stage/, tarballed into cli/target/dist/, and
-- acceptance-tested before declaring done.
--
-- Public:
--   dist.package(version, target)
--     Single entrypoint. Orchestrates the full pipeline:
--       stage_lua → stage_luarocks → cargo build → copy cook → VERSION
--       → check_exports → tar → verify
--
-- Internal helpers (file-local):
--   stage_lua()        — compile vendored Lua 5.4.7 → target/cook-stage/
--   stage_luarocks()   — stage vendored LuaRocks 3.11.0 → target/cook-stage/
--   check_exports()    — verify the cook binary exports lua_*/luaL_* sentinels
--   tar(stage_name)    — produce cli/target/dist/<stage_name>.tar.gz + .sha256
--   verify()           — Part A (hand-rolled C rock) + Part B (luarocks install)
--                        acceptance gate against the staged tree.
--
-- Phase: register-only. Uses cook.sh, fs.*, plain Lua. No multi-line bash
-- heredocs in cook.* calls (Rule 3 of the refactor spec).

local M = {}

-- ── helpers ────────────────────────────────────────────────────────────────

-- pcall wrapper: cook.sh raises on non-zero; we want a boolean for control
-- flow. On failure, returns (false, err) where `err` is the cook.sh error
-- message (the COOK_CMD_FAILED:... payload) so callers can log diagnostics.
local function try_sh(cmd)
    local ok, out_or_err = pcall(cook.sh, cmd)
    if ok then return true, out_or_err:gsub("%s+$", "") end
    return false, out_or_err
end

local function platform()
    local os_id = cook.platform.os
    if os_id == "linux" or os_id == "macos" then return os_id end
    error("dist: unsupported platform: " .. tostring(os_id))
end

local function rstrip(s) return (s:gsub("%s+$", "")) end

-- ── constants ──────────────────────────────────────────────────────────────

local LUA_SRC_DIR = "cli/vendored/lua-5.4.7"
local LUAROCKS_SRC_DIR = "cli/vendored/luarocks-3.11.0"
local CONFIG_TEMPLATE = "cli/crates/cook-cli/templates/default-rocks-config.lua"
local STAGE = "target/cook-stage"

-- All .c files except lua.c and luac.c form the library translation units.
-- Mirrors Lua's own Makefile (LIB_O + CORE_O minus lua.o/luac.o).
local LIB_C = {
    "lapi.c", "lcode.c", "lctype.c", "ldebug.c", "ldo.c", "ldump.c",
    "lfunc.c", "lgc.c", "llex.c", "lmem.c", "lobject.c", "lopcodes.c",
    "lparser.c", "lstate.c", "lstring.c", "ltable.c", "ltm.c", "lundump.c",
    "lvm.c", "lzio.c",
    "lauxlib.c", "lbaselib.c", "lcorolib.c", "ldblib.c", "liolib.c",
    "lmathlib.c", "loadlib.c", "loslib.c", "lstrlib.c", "ltablib.c",
    "lutf8lib.c", "linit.c",
}

-- check_exports sentinels: real exported symbols, not preprocessor-macro
-- names. In Lua 5.4 lua_pcall is #define'd as lua_pcallk(...,0,NULL) and
-- luaL_checkstring expands to luaL_checklstring(...,NULL), so the linker
-- only sees the K-variant / l-variant. C rocks have the macros expanded at
-- their compile time, so verifying the K/l symbols is the load-bearing
-- dlopen check.
local EXPORT_SENTINELS = {
    "lua_pushstring", "lua_pcallk", "lua_close", "lua_newstate",
    "luaL_newstate", "luaL_loadstring", "luaL_openlibs", "luaL_checklstring",
}

-- Relocatable shell launcher for the bundled luarocks. Derives COOK_PREFIX
-- from $0 (the launcher's own location) so the install can be moved on disk
-- without baking absolute paths.
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

-- Cook-authored luarocks driver. Mirrors upstream src/bin/luarocks with
-- one difference: --version is pre-empted to print "LuaRocks <ver>" rather
-- than "<full-path> <ver>" (upstream's default leaks the staged install
-- path into the version banner).
local LUAROCKS_DRIVER = [[-- Cook-staged LuaRocks driver. See cook_modules/dist.lua → LUAROCKS_DRIVER.
for _, a in ipairs(arg) do
    if a == "--version" then
        print("LuaRocks 3.11.0")
        os.exit(0)
    end
end

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

-- ── stage_lua ──────────────────────────────────────────────────────────────

local function stage_lua()
    local plat = platform()
    print("[dist.stage_lua] platform: " .. plat)

    fs.mkdir_p(STAGE .. "/lib")
    fs.mkdir_p(STAGE .. "/bin")
    fs.mkdir_p(STAGE .. "/include/lua5.4")
    fs.mkdir_p("target/build-lua-obj")

    local cflags = "-O2 -fPIC -DLUA_COMPAT_5_3 -I" .. LUA_SRC_DIR
    cflags = cflags .. (plat == "linux" and " -DLUA_USE_LINUX" or " -DLUA_USE_MACOSX")

    local sources = {}
    for _, f in ipairs(LIB_C) do table.insert(sources, f) end
    table.insert(sources, "lua.c")
    table.insert(sources, "luac.c")

    local env_prefix = (plat == "macos") and "MACOSX_DEPLOYMENT_TARGET=11.0 " or ""

    for _, src in ipairs(sources) do
        local obj = "target/build-lua-obj/" .. src:gsub("%.c$", ".o")
        cook.sh(string.format("%scc %s -c %s/%s -o %s",
            env_prefix, cflags, LUA_SRC_DIR, src, obj))
    end

    local lib_objs = {}
    for _, f in ipairs(LIB_C) do
        table.insert(lib_objs, "target/build-lua-obj/" .. f:gsub("%.c$", ".o"))
    end
    local lib_objs_str = table.concat(lib_objs, " ")

    if plat == "linux" then
        cook.sh(string.format("cc -shared -o %s/lib/liblua5.4.so %s -lm -ldl",
            STAGE, lib_objs_str))
        cook.sh(string.format(
            "cc -Wl,-E -o %s/bin/lua target/build-lua-obj/lua.o %s -lm -ldl",
            STAGE, lib_objs_str))
        cook.sh(string.format("cc -o %s/bin/luac target/build-lua-obj/luac.o %s -lm -ldl",
            STAGE, lib_objs_str))
    else
        cook.sh(string.format("%scc -dynamiclib -o %s/lib/liblua5.4.dylib %s -lm",
            env_prefix, STAGE, lib_objs_str))
        cook.sh(string.format(
            "install_name_tool -id @executable_path/../lib/liblua5.4.dylib %s/lib/liblua5.4.dylib",
            STAGE))
        cook.sh(string.format("%scc -o %s/bin/lua target/build-lua-obj/lua.o %s -lm",
            env_prefix, STAGE, lib_objs_str))
        cook.sh(string.format("%scc -o %s/bin/luac target/build-lua-obj/luac.o %s -lm",
            env_prefix, STAGE, lib_objs_str))
    end

    for _, h in ipairs({"lua.h", "lauxlib.h", "lualib.h", "luaconf.h"}) do
        cook.sh(string.format("cp %s/%s %s/include/lua5.4/%s",
            LUA_SRC_DIR, h, STAGE, h))
    end

    print("[dist.stage_lua] artifacts staged at " .. STAGE)
end

-- ── stage_luarocks ─────────────────────────────────────────────────────────

local function stage_luarocks()
    if not fs.exists(STAGE .. "/bin/lua") then
        error("dist.stage_luarocks: " .. STAGE .. "/bin/lua missing — stage_lua must run first")
    end

    print("[dist.stage_luarocks] staging luarocks 3.11.0 → " .. STAGE)

    fs.mkdir_p(STAGE .. "/share")
    fs.mkdir_p(STAGE .. "/share/cook")
    cook.sh("rm -rf " .. STAGE .. "/share/luarocks")
    cook.sh(string.format("cp -a %s/src/luarocks %s/share/luarocks",
        LUAROCKS_SRC_DIR, STAGE))
    cook.sh(string.format("cp -a %s %s/share/cook/default-rocks-config.lua",
        CONFIG_TEMPLATE, STAGE))

    fs.write(STAGE .. "/share/cook/luarocks-driver.lua", LUAROCKS_DRIVER)

    local launcher_path = STAGE .. "/bin/luarocks"
    fs.write(launcher_path, LAUNCHER_SCRIPT)
    cook.sh("chmod 0755 " .. launcher_path)

    print("[dist.stage_luarocks] staged " .. launcher_path)
end

-- ── check_exports ──────────────────────────────────────────────────────────

-- check_exports inspects the binary at `bin_path` and verifies it exports
-- the lua_*/luaL_* sentinels needed for C rocks to dlopen successfully.
-- Callers pass the actual binary they're about to ship — typically
-- target/cook-stage/bin/cook after staging — so we never inspect a stale
-- cli/target/release/cook from a prior un-targeted build.
local function check_exports(bin_path)
    if not fs.exists(bin_path) then
        error("dist.check_exports: no binary at " .. bin_path)
    end
    local plat = platform()
    local nm_cmd = (plat == "linux")
        and ("nm -D --defined-only " .. bin_path)
        or ("nm -gU " .. bin_path)

    print("[dist.check_exports] inspecting " .. bin_path)
    local output = cook.sh(nm_cmd)

    local missing = {}
    for _, sym in ipairs(EXPORT_SENTINELS) do
        if not output:match("[%s_]" .. sym .. "$")
            and not output:match("[%s_]" .. sym .. "\n") then
            table.insert(missing, sym)
        end
    end

    if #missing > 0 then
        io.stderr:write(string.format(
            "[dist.check_exports] FAIL: missing %d/%d sentinel symbol(s):\n",
            #missing, #EXPORT_SENTINELS))
        for _, s in ipairs(missing) do
            io.stderr:write("    " .. s .. "\n")
        end
        io.stderr:write(
            "[dist.check_exports] cause: cli/.cargo/config.toml is missing the\n"
            .. "    -rdynamic (Linux) / -Wl,-export_dynamic (macOS) link-arg.\n")
        error("dist.check_exports failed: " .. #missing .. " missing symbol(s)")
    end

    print(string.format("[dist.check_exports] OK — %d/%d sentinels present",
        #EXPORT_SENTINELS, #EXPORT_SENTINELS))
end

-- ── tar ────────────────────────────────────────────────────────────────────

local function tar(stage_name)
    fs.mkdir_p("cli/target/dist")

    cook.sh(string.format(
        "tar -czf cli/target/dist/%s.tar.gz -C target/cook-stage .",
        stage_name))

    -- sha256sum on Linux, shasum -a 256 on macOS.
    local has_sha256sum = try_sh("command -v sha256sum >/dev/null")
    local sha_cmd = has_sha256sum and "sha256sum" or "shasum -a 256"

    -- Subshell so the recorded path is the bare basename, not the
    -- cli/target/dist/ prefix. Single logical operation: "compute hash
    -- relative to dist/". This is the one place this module keeps a
    -- shell-state-with-a-command pattern (Rule 3 explicit exception).
    local hash_line = cook.sh(string.format(
        "(cd cli/target/dist && %s %s.tar.gz)", sha_cmd, stage_name))
    fs.write(string.format("cli/target/dist/%s.tar.gz.sha256", stage_name), hash_line)
end

-- ── verify ─────────────────────────────────────────────────────────────────

-- Resolve `rel` to an absolute path. readlink -f is Linux coreutils only;
-- BSD/macOS readlink lacks -f, so we fall back to a (cd && pwd) subshell —
-- one logical operation, no fallback chain.
local function abspath(rel)
    if cook.platform.os == "linux" then
        return rstrip(cook.sh("readlink -f " .. rel))
    end
    return rstrip(cook.sh("(cd " .. rel .. " && pwd)"))
end

-- Allocate a fresh scratch dir under target/. CS-0045 confines fs.* and
-- path-bearing APIs in chore step Lua bodies to paths under the project
-- root, so /tmp is off-limits — use the build tree instead.
local function scratchdir()
    local rel = rstrip(cook.sh("mktemp -d -p target cook-dist-verify-XXXXXX"))
    return abspath(rel)
end

local function verify_part_a(ctx)
    print("[dist.verify] Part A: building cook_hello.so")

    local so_path = ctx.proj_modules .. "/cook_hello.so"
    local cc_cmd
    if ctx.plat == "macos" then
        cc_cmd = string.format(
            "MACOSX_DEPLOYMENT_TARGET=11.0 cc -O2 -fPIC -bundle -undefined dynamic_lookup "
            .. "-I%s/include/lua5.4 -o %s %s",
            ctx.cook_prefix, so_path, ctx.fixture_c)
    else
        cc_cmd = string.format(
            "cc -O2 -fPIC -shared -I%s/include/lua5.4 -o %s %s",
            ctx.cook_prefix, so_path, ctx.fixture_c)
    end
    cook.sh(cc_cmd)
    if not fs.exists(so_path) then
        error("dist.verify: Part A compile produced no .so at " .. so_path)
    end

    local cookfile_a_path = ctx.proj .. "/Cookfile-a"
    fs.write(cookfile_a_path, [[
chore gate-a
    >{
        local cook_hello = require("cook_hello")
        print("PART_A_VALUE=" .. tostring(cook_hello.value()))
    }
]])

    local out_a = cook.sh(ctx.env_prefix .. string.format(
        "%s/bin/cook -f %s gate-a 2>&1",
        ctx.cook_prefix, cookfile_a_path))
    print("[dist.verify] Part A output:\n" .. out_a)
    if not out_a:find("PART_A_VALUE=42", 1, true) then
        error("dist.verify: Part A did not print PART_A_VALUE=42 (cook executable's "
            .. "lua exports may be missing). Output:\n" .. out_a)
    end
    print("[dist.verify] Part A: OK (cook_hello.value() == 42)")
end

local function verify_part_b(ctx)
    print("[dist.verify] Part B: installing lua-cjson via bundled luarocks")

    cook.sh(ctx.env_prefix .. string.format(
        "%s/bin/luarocks install lua-cjson --tree %s --server https://luarocks.org 2>&1",
        ctx.cook_prefix, ctx.proj_modules))

    local cjson_so = ctx.proj_modules .. "/lib/lua/5.4/cjson.so"
    if not fs.exists(cjson_so) then
        error("dist.verify: Part B luarocks install did not produce " .. cjson_so)
    end

    local cookfile_b_path = ctx.proj .. "/Cookfile-b"
    fs.write(cookfile_b_path, [[
chore gate-b
    >{
        local cjson = require("cjson")
        local encoded = cjson.encode({ hello = "world", n = 42 })
        print("ENCODED=" .. encoded)
        local decoded = cjson.decode(encoded)
        if decoded.hello == "world" and decoded.n == 42 then
            print("PART_B_ROUND_TRIP=ok")
        else
            print("PART_B_ROUND_TRIP=fail")
        end
    }
]])

    local out_b = cook.sh(ctx.env_prefix .. string.format(
        "%s/bin/cook -f %s gate-b 2>&1",
        ctx.cook_prefix, cookfile_b_path))
    print("[dist.verify] Part B output:\n" .. out_b)
    if not out_b:find('"hello":"world"', 1, true) then
        error("dist.verify: Part B encoded output missing \"hello\":\"world\". Output:\n" .. out_b)
    end
    if not out_b:find("PART_B_ROUND_TRIP=ok", 1, true) then
        error("dist.verify: Part B round-trip failed. Output:\n" .. out_b)
    end
    print("[dist.verify] Part B: OK (cjson encode/decode round-trip)")
end

local function verify()
    local plat = platform()
    print("[dist.verify] platform: " .. plat)

    if not fs.exists(STAGE .. "/bin/cook") then
        error("dist.verify: " .. STAGE .. "/bin/cook missing — package must run first")
    end
    if not fs.exists(STAGE .. "/bin/luarocks") then
        error("dist.verify: " .. STAGE .. "/bin/luarocks missing — package must run first")
    end

    local cook_prefix = abspath(STAGE)
    local fixture_c = abspath("cli/crates/cook-engine/tests/fixtures/c-rock")
        .. "/cook_hello.c"
    if not fs.exists(fixture_c) then
        error("dist.verify: fixture missing: " .. fixture_c)
    end

    local td = scratchdir()
    local proj = td .. "/proj"
    local proj_modules = proj .. "/cook_modules"
    print("[dist.verify] scratch dir: " .. td)
    print("[dist.verify] cook_prefix: " .. cook_prefix)

    fs.mkdir_p(proj_modules)

    -- Inline-prefix env: cook.sh runs via /bin/sh -c, so VAR=val cmd works.
    local ctx = {
        plat = plat,
        cook_prefix = cook_prefix,
        fixture_c = fixture_c,
        proj = proj,
        proj_modules = proj_modules,
        env_prefix = string.format(
            "COOK_PREFIX=%s PATH=%s/bin:$PATH ",
            cook_prefix, cook_prefix),
    }

    verify_part_a(ctx)
    verify_part_b(ctx)

    -- Best-effort cleanup. The scratch dir is under target/ (CS-0045
    -- confines chore-Lua paths to the project root, so /tmp is unusable).
    -- Use try_sh so a transient rm failure doesn't fail an otherwise-passing
    -- verify run.
    try_sh("rm -rf " .. td)

    print("[dist.verify] BOTH PARTS PASS on " .. plat)
end

-- ── package (the only public entrypoint) ───────────────────────────────────

local function host_target()
    local out = cook.sh("rustc -vV")
    return out:match("host: ([^\n]+)")
end

local function target_to_os_arch(triple)
    local os_part, arch_part
    if triple:find("apple%-darwin") then os_part = "darwin"
    elseif triple:find("linux") then os_part = "linux"
    else error("dist.package: unsupported OS in target triple: " .. triple) end
    if triple:find("^x86_64%-") then arch_part = "amd64"
    elseif triple:find("^aarch64%-") then arch_part = "arm64"
    else error("dist.package: unsupported arch in target triple: " .. triple) end
    return os_part, arch_part
end

function M.package(version, target)
    if not version or version == "" then
        error("dist.package: missing VERSION (pass --set VERSION=vX.Y.Z)")
    end
    -- VERSION is interpolated into shell commands and tarball filenames;
    -- restrict to the safe alphabet upstream packaging tools accept.
    if not version:match("^v?[0-9A-Za-z._%-]+$") then
        error("dist.package: VERSION '" .. version .. "' must match ^v?[0-9A-Za-z._-]+$")
    end
    target = (target ~= nil and target ~= "") and target or host_target()
    local os_part, arch_part = target_to_os_arch(target)
    local stage_name = string.format("cook-%s-%s-%s", version, os_part, arch_part)
    local tarball = string.format("cli/target/dist/%s.tar.gz", stage_name)

    print(string.format("[dist.package] version=%s target=%s -> %s",
        version, target, tarball))

    -- Stage the bundled runtime. These used to be separate user-visible chores
    -- (`cook build-lua`, `cook bundle-luarocks`); they're orchestrated here now.
    stage_lua()
    stage_luarocks()

    -- Build cook itself. Cargo's incremental build makes this idempotent.
    cook.sh(string.format(
        "cargo build --release --manifest-path cli/Cargo.toml --target=%s -p cook-cli",
        target))

    local built_bin = string.format("cli/target/%s/release/cook", target)
    if not fs.exists(built_bin) then
        error("dist.package: cargo did not produce " .. built_bin)
    end
    cook.sh(string.format("cp %s %s/bin/cook", built_bin, STAGE))
    cook.sh("chmod 0755 " .. STAGE .. "/bin/cook")

    -- VERSION marker file (Phase 1 contract).
    fs.write(STAGE .. "/VERSION", version .. "\n")

    -- Final symbol-export guard before tarballing — catches a regression in
    -- cli/.cargo/config.toml at packaging time rather than at first dlopen.
    check_exports(STAGE .. "/bin/cook")

    -- Produce the tarball + .sha256 sibling.
    tar(stage_name)
    print(string.format("[dist.package] built %s (+ .sha256)", tarball))

    -- Acceptance gate against the staged tree.
    verify()
end

return M
