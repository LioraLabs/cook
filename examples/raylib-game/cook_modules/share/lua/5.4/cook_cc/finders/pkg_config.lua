local lpeg = require("lpeg")
local P, S, C, Ct = lpeg.P, lpeg.S, lpeg.C, lpeg.Ct

local M = {}

local space     = S(" \t")^0
local non_space = (P(1) - S(" \t"))^1
local include   = P("-I") * C(non_space)
local libdir    = P("-L") * C(non_space)
local syslib    = P("-l") * C(non_space)
local define    = P("-D") * C(non_space)
local framework = P("-framework") * S(" \t")^1 * C(non_space)
local other     = C(non_space)

local token = framework / function(v) return { kind = "framework", value = v } end
            + include   / function(v) return { kind = "include",   value = v } end
            + libdir    / function(v) return { kind = "libdir",    value = v } end
            + syslib    / function(v) return { kind = "syslib",    value = v } end
            + define    / function(v) return { kind = "define",    value = v } end
            + other     / function(v) return { kind = "other",     value = v } end

local line_pattern = Ct((space * token)^0 * space)
local function parse_tokens(s) return line_pattern:match(s or "") or {} end

local function shell_chomp(s) return (s or ""):gsub("%s+$", "") end

local function try_sh(cmd)
    local ok, out = pcall(cook.sh, cmd)
    return ok, ok and shell_chomp(out) or nil
end

function M.try(name)
    local ok = try_sh("pkg-config --exists " .. name)
    if not ok then return nil end
    local _, cflags  = try_sh("pkg-config --cflags " .. name)
    local _, libs    = try_sh("pkg-config --libs "   .. name)
    local _, version = try_sh("pkg-config --modversion " .. name)
    if version == "" then version = nil end

    local payload = {
        cflags = cflags or "",
        libs   = libs or "",
        system_libs = {}, include_dirs = {}, lib_dirs = {}, frameworks = {},
        version = version,
    }
    local function bucket(toks)
        for _, t in ipairs(toks) do
            if     t.kind == "include"   then payload.include_dirs[#payload.include_dirs + 1] = t.value
            elseif t.kind == "libdir"    then payload.lib_dirs[#payload.lib_dirs + 1] = t.value
            elseif t.kind == "syslib"    then payload.system_libs[#payload.system_libs + 1] = t.value
            elseif t.kind == "framework" then payload.frameworks[#payload.frameworks + 1] = t.value
            end
        end
    end
    bucket(parse_tokens(payload.cflags))
    bucket(parse_tokens(payload.libs))

    return { strategy = "pkg-config", outcome = "hit", reason = "", payload = payload }
end

function M.main_chain(name, opts)
    local attempt = M.try(name)
    if attempt then
        if opts.version and attempt.payload.version then
            local ver = require("cook_cc.version")
            if not ver.satisfies(attempt.payload.version, opts.version) then
                return { strategy = "pkg-config", outcome = "miss",
                         reason = "detected version " .. attempt.payload.version
                                  .. " does not satisfy " .. opts.version }
            end
        elseif opts.version and not attempt.payload.version then
            return { strategy = "pkg-config", outcome = "miss",
                     reason = "could not determine version; constraint " .. opts.version
                              .. " cannot be honoured" }
        end
        return attempt
    end
    return { strategy = "pkg-config", outcome = "miss",
             reason = "package '" .. name .. "' not found by pkg-config" }
end

return M
