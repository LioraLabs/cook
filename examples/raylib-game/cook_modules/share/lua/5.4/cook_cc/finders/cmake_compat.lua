local M = {}

local lpeg = require("lpeg")
local P, S, C, Ct = lpeg.P, lpeg.S, lpeg.C, lpeg.Ct

local space     = S(" \t")^0
local non_space = (P(1) - S(" \t"))^1
local include   = P("-I") * C(non_space)
local define    = P("-D") * C(non_space)
local other_tok = C(non_space)

local compile_token = include   / function(v) return { kind = "include", value = v } end
                    + define    / function(v) return { kind = "define",  value = v } end
                    + other_tok / function(v) return { kind = "other",   value = v } end
local compile_line = Ct((space * compile_token)^0 * space)

local function parse_compile(s)
    return compile_line:match(s or "") or {}
end

local function classify_link_token(tok)
    if tok == "-framework"                  then return "framework-marker" end
    if tok:sub(1, 2) == "-l"                then return "syslib" end
    if tok:sub(1, 2) == "-L"                then return "libdir" end
    if tok:match("[/\\][^/\\]*Config%.cmake$")  then return "config-file-ref" end
    if tok:match("[/\\][^/\\]*Targets%.cmake$") then return "config-file-ref" end
    if tok:match("%.so$") or tok:match("%.so%.[%d.]+$") then return "abs-lib" end
    if tok:match("%.dylib$")                then return "abs-lib" end
    if tok:match("%.a$") or tok:match("%.lib$") then return "abs-lib" end
    return "other"
end

local function parse_link(out)
    local tokens = {}
    for tok in (out or ""):gmatch("%S+") do tokens[#tokens + 1] = tok end

    local result = { include_dirs = {}, lib_dirs = {}, system_libs = {},
                     frameworks = {}, abs_libs = {}, other = {},
                     has_config_ref = false }
    local i = 1
    while i <= #tokens do
        local k = classify_link_token(tokens[i])
        if k == "framework-marker" and tokens[i + 1] then
            result.frameworks[#result.frameworks + 1] = tokens[i + 1]
            i = i + 2
        elseif k == "syslib" then
            result.system_libs[#result.system_libs + 1] = tokens[i]:sub(3)
            i = i + 1
        elseif k == "libdir" then
            result.lib_dirs[#result.lib_dirs + 1] = tokens[i]:sub(3)
            i = i + 1
        elseif k == "config-file-ref" then
            result.has_config_ref = true
            i = i + 1
        elseif k == "abs-lib" then
            result.abs_libs[#result.abs_libs + 1] = tokens[i]
            i = i + 1
        else
            result.other[#result.other + 1] = tokens[i]
            i = i + 1
        end
    end
    return result
end

local function detect_cmake()
    local cached = cook.cache.get("cc.cmake-compat:driver")
    if cached then return cached end

    local path = cook.sh("command -v cmake 2>/dev/null") or ""
    path = path:gsub("%s+$", "")
    if path == "" then
        local r = { ok = false }
        cook.cache.set("cc.cmake-compat:driver", r)
        return r
    end

    local probe = cook.sh(
        "cmake --find-package -DNAME=ZLIB -DCOMPILER_ID=GNU "
        .. "-DLANGUAGE=C -DMODE=EXIST -DQUIET=TRUE 2>&1 || true") or ""
    local legacy_supported = not probe:match("[Uu]nknown option")
    local r = { ok = true, path = path, legacy_supported = legacy_supported }
    cook.cache.set("cc.cmake-compat:driver", r)
    return r
end

local FLAG_BASE = " -DCOMPILER_ID=GNU -DLANGUAGE=C -DQUIET=TRUE "

local function probe_exist(name)
    local cmd = "cmake --find-package -DNAME=" .. name .. FLAG_BASE
              .. "-DMODE=EXIST 2>&1"
    local ok, out = pcall(cook.sh, cmd)
    if not ok then return false end
    out = out or ""
    if out:match(name .. " not found%.") then return false end
    if out:match(name .. " found%.") then return true end
    return false
end

local function probe_compile(name)
    local cmd = "cmake --find-package -DNAME=" .. name .. FLAG_BASE
              .. "-DMODE=COMPILE 2>&1"
    local ok, out = pcall(cook.sh, cmd)
    if not ok then return false, nil end
    return true, (out or ""):gsub("%s+$", "")
end

local function probe_link(name)
    local cmd = "cmake --find-package -DNAME=" .. name .. FLAG_BASE
              .. "-DMODE=LINK 2>&1"
    local ok, out = pcall(cook.sh, cmd)
    if not ok then return false, nil end
    return true, (out or ""):gsub("%s+$", "")
end

function M.main_chain(name, opts)
    opts = opts or {}
    if opts.version then
        return { strategy = "cmake-compat", outcome = "skip",
                 reason = "version detection unsupported by legacy cmake --find-package" }
    end
    local driver = detect_cmake()
    if not driver.ok then
        return { strategy = "cmake-compat", outcome = "skip",
                 reason = "cmake binary not on PATH",
                 hint = "install cmake: apt: cmake / brew: cmake / dnf: cmake" }
    end
    if not driver.legacy_supported then
        return { strategy = "cmake-compat", outcome = "skip",
                 reason = "this cmake build does not support --find-package legacy mode" }
    end
    if not probe_exist(name) then
        local hints = require("cook_cc.finders.cmake_compat.hints")
        return { strategy = "cmake-compat", outcome = "miss",
                 reason = "cmake found no Config or Find module for '" .. name .. "'",
                 hint = hints.for_package(name) }
    end

    local ok_c, compile_out = probe_compile(name)
    if not ok_c then
        return { strategy = "cmake-compat", outcome = "miss",
                 reason = "cmake --find-package returned a non-zero exit in COMPILE mode" }
    end
    local ok_l, link_out = probe_link(name)
    if not ok_l then
        return { strategy = "cmake-compat", outcome = "miss",
                 reason = "cmake --find-package returned a non-zero exit in LINK mode" }
    end

    local lt = parse_link(link_out)
    if lt.has_config_ref then
        return { strategy = "cmake-compat", outcome = "miss",
                 reason = "imported-target chain too complex for legacy --find-package",
                 hint = "register a project finder for '" .. name .. "' via cc.register_finder" }
    end

    local payload = {
        cflags = compile_out,
        libs   = link_out,                  -- raw stdout retained for extra_ldflags piping
        include_dirs = {}, lib_dirs = lt.lib_dirs,
        system_libs  = lt.system_libs,
        frameworks   = lt.frameworks,
        version      = nil,
    }
    for _, t in ipairs(parse_compile(compile_out)) do
        if t.kind == "include" then
            payload.include_dirs[#payload.include_dirs + 1] = t.value
        end
    end

    return { strategy = "cmake-compat", outcome = "hit", reason = "", payload = payload }
end

return M
