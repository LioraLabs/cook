local M = {}

-- File-local state. Per-VM; rehydrated from cook.cache by init.lua.
local state = {
    compiler         = nil,    -- { cxx = "...", cc = "..." }
    default_standard = nil,    -- e.g. "c++17"
    warnings         = "default",
    defaults         = {},     -- additive-merge target defaults
}

local function compiler_present(cxx)
    local ok = pcall(function() cook.sh("command -v " .. cxx) end)
    return ok
end

local function detect_compiler()
    for _, candidate in ipairs({
        { cxx = "g++",     cc = "gcc"   },
        { cxx = "clang++", cc = "clang" },
    }) do
        if compiler_present(candidate.cxx) then return candidate end
    end
    error("[cc.toolchain] no C/C++ compiler on PATH (tried g++, clang++)")
end

function M.rehydrate()
    local cached = cook.cache.get("compiler")
    if cached and compiler_present(cached.cxx) then
        state.compiler = cached
        return
    end
    state.compiler = detect_compiler()
    cook.cache.set("compiler", state.compiler)
end

function M.set(opts)
    opts = opts or {}
    if opts.compiler then
        local cxx = opts.compiler
        local cc
        if cxx:match("clang") then cc = "clang"
        elseif cxx:match("g%+%+") then cc = "gcc"
        else cc = "cc" end
        state.compiler = { cxx = cxx, cc = cc }
    end
    if opts.standard then state.default_standard = opts.standard end
    if opts.warnings then state.warnings = opts.warnings end
end

local function append_list(dst, src)
    if not src then return end
    for _, v in ipairs(src) do dst[#dst + 1] = v end
end

function M.merge_defaults(opts)
    opts = opts or {}
    state.defaults.defines     = state.defaults.defines     or {}
    state.defaults.includes    = state.defaults.includes    or {}
    state.defaults.system_libs = state.defaults.system_libs or {}
    append_list(state.defaults.defines,     opts.defines)
    append_list(state.defaults.includes,    opts.includes)
    append_list(state.defaults.system_libs, opts.system_libs)
    if opts.extra_cflags then
        local prev = state.defaults.extra_cflags
        state.defaults.extra_cflags = prev and (prev .. " " .. opts.extra_cflags) or opts.extra_cflags
    end
    if opts.extra_ldflags then
        local prev = state.defaults.extra_ldflags
        state.defaults.extra_ldflags = prev and (prev .. " " .. opts.extra_ldflags) or opts.extra_ldflags
    end
    if opts.standard then state.default_standard = opts.standard end
    if opts.warnings then state.warnings = opts.warnings end
end

function M.get_compiler()         return state.compiler         end
function M.get_default_standard() return state.default_standard end
function M.get_warnings()         return state.warnings         end
function M.get_defaults()         return state.defaults         end

function M.warning_flags(override)
    local w = override or state.warnings
    if w == "default" then return "-Wall"
    elseif w == "strict" then return "-Wall -Wextra -Wpedantic"
    elseif w == "none" then return ""
    else return w end
end

return M
