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
    -- VERSION is interpolated into shell commands and tarball filenames;
    -- restrict to the safe alphabet upstream packaging tools accept.
    if not version:match("^v?[0-9A-Za-z._%-]+$") then
        error("package: VERSION '" .. version .. "' must match ^v?[0-9A-Za-z._-]+$")
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

-- ── M2.4: gate_m2 ─────────────────────────────────────────────────────────
-- Acceptance gate that proves the Phase 2 stage tree is real.
--
-- Part A: build a hand-rolled Lua C extension (tests/fixtures/c-ext-hello/
-- cook_hello.c) against the bundled headers, place the resulting .so on
-- package.cpath, run the staged cook against a Cookfile that requires it,
-- and assert the function returns 42. This is the "rdynamic exports work"
-- check — the .so has unresolved lua_*/luaL_* references that must
-- resolve at dlopen time against cook's exported symbol table.
--
-- Part B: install lua-cjson via the bundled luarocks into a fresh tree,
-- run the staged cook against a Cookfile that requires("cjson"), and
-- assert a JSON encode/decode round-trip succeeds. This is the
-- macOS two-level-namespace regression catcher: luarocks's bundled
-- -undefined dynamic_lookup default makes lua_* symbols in the C rock
-- resolve at dlopen time against cook's flat-namespace exports.
--
-- Both parts depend on `cook package` having staged target/cook-stage/
-- (the chore "gate-m2: package" dep wires this).

local function gate_platform()
    local os_id = cook.platform.os
    if os_id == "linux" or os_id == "macos" then return os_id
    else error("gate_m2: unsupported platform: " .. tostring(os_id))
    end
end

-- Resolve `rel` to an absolute path. readlink -f works on Linux coreutils;
-- the (cd && pwd) fallback covers BSD/macOS where readlink may lack -f.
local function abspath(rel)
    local out = cook.sh("readlink -f " .. rel .. " 2>/dev/null || (cd " .. rel .. " && pwd)")
    return (out:gsub("\n+$", ""))
end

-- Allocate a fresh scratch dir under target/. CS-0045 confines fs.* and
-- path-bearing APIs in chore step Lua bodies to paths under the project
-- root, so /tmp is off-limits — use the build tree instead. Each invocation
-- gets a fresh stamp suffix so re-runs don't collide. Returns an absolute
-- path so downstream concatenations don't depend on cwd.
local function tmpdir()
    local rel = cook.sh("mktemp -d -p target cook-gate-m2-XXXXXX")
    rel = (rel:gsub("\n+$", ""))
    local abs = cook.sh("readlink -f " .. rel .. " 2>/dev/null || (cd " .. rel .. " && pwd)")
    return (abs:gsub("\n+$", ""))
end

function M.gate_m2()
    local plat = gate_platform()
    print("[gate-m2] platform: " .. plat)

    -- The `: package` chore dep guarantees the stage tree exists.
    if not fs.exists(STAGE .. "/bin/cook") then
        error("gate_m2: " .. STAGE .. "/bin/cook missing — `cook package` must run first")
    end
    if not fs.exists(STAGE .. "/bin/luarocks") then
        error("gate_m2: " .. STAGE .. "/bin/luarocks missing — `cook package` must run first")
    end

    local cook_prefix = abspath(STAGE)
    local fixture_c = abspath("tests/fixtures/c-ext-hello") .. "/cook_hello.c"
    if not fs.exists(fixture_c) then
        error("gate_m2: fixture missing: " .. fixture_c)
    end

    local td = tmpdir()
    local proj = td .. "/proj"
    local proj_modules = proj .. "/cook_modules"
    print("[gate-m2] tmpdir: " .. td)
    print("[gate-m2] cook_prefix: " .. cook_prefix)

    cook.exec(string.format([[
set -euo pipefail
mkdir -p %s
]], proj_modules), 0)

    -- Common env prefix for invoking the staged cook: COOK_PREFIX so the
    -- launcher's relocatable-resolution works, and PATH set so any
    -- $(cook) sub-invocation finds the staged binaries first. Inline-prefix
    -- form: cook.exec runs via /bin/sh -c, so VAR=val cmd works.
    local env_prefix = string.format(
        "COOK_PREFIX=%s PATH=%s/bin:$PATH ",
        cook_prefix, cook_prefix
    )

    -- ── Part A: hand-rolled Lua C extension ───────────────────────────
    print("[gate-m2] Part A: building cook_hello.so")

    -- Compile the fixture against the staged Lua headers. macOS needs
    -- -undefined dynamic_lookup so the lua_* references in the .so
    -- resolve at dlopen time (against cook's -Wl,-export_dynamic exports)
    -- rather than at link time. Linux ld leaves shared-lib unresolved
    -- references unresolved by default, so no extra flag is required.
    local so_path = proj_modules .. "/cook_hello.so"
    local cc_cmd
    if plat == "macos" then
        cc_cmd = string.format(
            "MACOSX_DEPLOYMENT_TARGET=11.0 cc -O2 -fPIC -bundle -undefined dynamic_lookup -I%s/include/lua5.4 -o %s %s",
            cook_prefix, so_path, fixture_c
        )
    else
        cc_cmd = string.format(
            "cc -O2 -fPIC -shared -I%s/include/lua5.4 -o %s %s",
            cook_prefix, so_path, fixture_c
        )
    end
    cook.exec(cc_cmd, 0)
    if not fs.exists(so_path) then
        error("gate_m2: Part A compile produced no .so at " .. so_path)
    end

    -- Phase-2-only workaround: the chore body must prepend package.cpath
    -- before require("cook_hello"). Phase 3 will land runtime cpath
    -- wiring that makes this implicit (cook will auto-include
    -- <cwd>/cook_modules/?.so on the search path).
    local cookfile_a_path = proj .. "/Cookfile-a"
    local cookfile_a = string.format([[
chore gate-a
    >{
        -- Phase 2: prepend cook_modules/?.so manually. Phase 3 lands
        -- runtime cpath wiring that makes this implicit.
        package.cpath = "%s/?.so;" .. package.cpath
        local cook_hello = require("cook_hello")
        print("PART_A_VALUE=" .. tostring(cook_hello.value()))
    }
]], proj_modules)
    fs.write(cookfile_a_path, cookfile_a)

    -- Run the staged cook against Cookfile-a and capture stdout.
    local out_a = cook.exec(env_prefix .. string.format(
        "%s/bin/cook -f %s gate-a 2>&1",
        cook_prefix, cookfile_a_path
    ), 0)
    print("[gate-m2] Part A output:\n" .. out_a)
    if not out_a:find("PART_A_VALUE=42", 1, true) then
        error("gate_m2: Part A did not print PART_A_VALUE=42 (cook executable's lua exports may be missing). Output:\n" .. out_a)
    end
    print("[gate-m2] Part A: OK (cook_hello.value() == 42)")

    -- ── Part B: lua-cjson via bundled luarocks ────────────────────────
    print("[gate-m2] Part B: installing lua-cjson via bundled luarocks")

    -- Install lua-cjson into the proj cook_modules tree. cook.exec on
    -- non-zero exit raises with stdout/stderr inlined post-SHI-188, so
    -- a luarocks failure here surfaces the real error directly (the
    -- gitignore-dropped build/ files would have been a one-line diagnosis).
    cook.exec(env_prefix .. string.format(
        "%s/bin/luarocks install lua-cjson --tree %s --server https://luarocks.org 2>&1",
        cook_prefix, proj_modules
    ), 0)

    -- Verify the rock landed where we expect. luarocks --tree <td>/cook_modules
    -- writes shared libs under lib/lua/5.4/.
    local cjson_so = proj_modules .. "/lib/lua/5.4/cjson.so"
    if not fs.exists(cjson_so) then
        error("gate_m2: Part B luarocks install did not produce " .. cjson_so)
    end

    -- Phase-2-only workaround: same package.path/cpath wiring as Part A.
    -- luarocks --tree puts modules under share/lua/5.4 and lib/lua/5.4;
    -- prepend both so require("cjson") resolves the freshly-installed rock.
    local cookfile_b_path = proj .. "/Cookfile-b"
    local cookfile_b = string.format([[
chore gate-b
    >{
        -- Phase 2: prepend cook_modules tree manually. Phase 3 lands
        -- runtime cpath wiring that makes this implicit.
        package.path = "%s/share/lua/5.4/?.lua;%s/share/lua/5.4/?/init.lua;" .. package.path
        package.cpath = "%s/lib/lua/5.4/?.so;" .. package.cpath
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
]], proj_modules, proj_modules, proj_modules)
    fs.write(cookfile_b_path, cookfile_b)

    local out_b = cook.exec(env_prefix .. string.format(
        "%s/bin/cook -f %s gate-b 2>&1",
        cook_prefix, cookfile_b_path
    ), 0)
    print("[gate-m2] Part B output:\n" .. out_b)
    if not out_b:find('"hello":"world"', 1, true) then
        error("gate_m2: Part B encoded output missing \"hello\":\"world\". Output:\n" .. out_b)
    end
    if not out_b:find("PART_B_ROUND_TRIP=ok", 1, true) then
        error("gate_m2: Part B round-trip failed. Output:\n" .. out_b)
    end
    print("[gate-m2] Part B: OK (cjson encode/decode round-trip)")

    -- Best-effort cleanup. The tmpdir is under target/ (CS-0045 confines
    -- chore-Lua paths to the project root, so /tmp is not usable). If rm
    -- fails the debris stays under target/ until the next `cargo clean`.
    cook.exec("rm -rf " .. td, 0)

    print("[gate-m2] BOTH PARTS PASS on " .. plat)
end

return M
