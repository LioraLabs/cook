-- Shared strategy helpers for cc:find:<n> probe bodies.
-- Required by both register-phase (cook_cc.finder.lua) and execute-phase
-- (the probe produce body on a worker VM).

local M = {}

local function blank_result()
    return {
        found        = false,
        cflags       = "",
        libs         = "",
        system_libs  = {},
        include_dirs = {},
        lib_dirs     = {},
        frameworks   = {},
        version      = nil,
        tried        = {},
    }
end

function M.build_result(hit, tried)
    if not hit then
        local r = blank_result()
        r.tried = tried
        return r
    end
    local p = hit.payload
    return {
        found        = true,
        cflags       = p.cflags or "",
        libs         = p.libs or "",
        system_libs  = p.system_libs or {},
        include_dirs = p.include_dirs or {},
        lib_dirs     = p.lib_dirs or {},
        frameworks   = p.frameworks or {},
        version      = p.version,
        tried        = tried,
    }
end

function M.project_strategy(registry, name, opts)
    local fn = registry and registry[name]
    if not fn then
        return { strategy = "project:" .. name, outcome = "skip",
                 reason = "no project finder registered" }
    end
    local rec = fn(opts)
    if rec and rec.found then
        return { strategy = "project:" .. name, outcome = "hit", reason = "",
                 payload = {
                     cflags       = rec.cflags or "",
                     libs         = rec.libs or "",
                     system_libs  = rec.system_libs or {},
                     include_dirs = rec.include_dirs or {},
                     lib_dirs     = rec.lib_dirs or {},
                     frameworks   = rec.frameworks or {},
                     version      = rec.version,
                 } }
    end
    return { strategy = "project:" .. name, outcome = "miss",
             reason = "project finder returned found=false" }
end

function M.curated_strategy(name, opts)
    local curated = require("cook_cc.finders")
    local fn = curated.lookup and curated.lookup(name) or nil
    if not fn then
        return { strategy = "curated:" .. name, outcome = "skip",
                 reason = "no curated finder for '" .. name .. "'" }
    end
    return fn(opts)
end

function M.pkg_strategy(name, opts)
    local pkg = require("cook_cc.finders.pkg_config")
    return pkg.main_chain(name, opts)
end

function M.cmake_strategy(name, opts)
    local cm = require("cook_cc.finders.cmake_compat")
    return cm.main_chain(name, opts)
end

function M.bare_strategy(name, opts)
    local bare = require("cook_cc.finders.bare_probe")
    return bare.main_chain(name, opts)
end

return M
