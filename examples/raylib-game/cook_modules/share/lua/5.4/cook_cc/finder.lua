local M = {}

-- Per-VM project-registered finder registry.
M._registry = M._registry or {}

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

local function canonical_opts(opts)
    if not opts then return "" end
    local keys = {}
    for k in pairs(opts) do keys[#keys + 1] = tostring(k) end
    table.sort(keys)
    local parts = {}
    for _, k in ipairs(keys) do
        local v = opts[k]
        if type(v) == "table" then v = table.concat(v, ",") end
        parts[#parts + 1] = k .. "=" .. tostring(v)
    end
    return table.concat(parts, ",")
end

local function build_result(hit, tried)
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

function M.register(name, finder)
    if type(finder) ~= "function" then
        error("[cc.register_finder] register_finder for '" .. tostring(name)
              .. "' requires a function, got " .. type(finder), 2)
    end
    M._registry[name] = finder
end

local function project_strategy(name, opts)
    local fn = M._registry[name]
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

local function curated_strategy(name, opts)
    local curated = require("cook_cc.finders")
    local fn = curated.lookup(name)
    if not fn then
        return { strategy = "curated:" .. name, outcome = "skip",
                 reason = "no curated finder for '" .. name .. "'" }
    end
    return fn(opts)
end

function M.find(name, opts)
    opts = opts or {}
    local cache_key = "cc.find:" .. name .. ":" .. canonical_opts(opts)
    local cached = cook.cache.get(cache_key)
    if cached then return cached end

    local pkg  = require("cook_cc.finders.pkg_config")
    local bare = require("cook_cc.finders.bare_probe")

    local chain = {
        function() return project_strategy(name, opts) end,
        function() return curated_strategy(name, opts) end,
        function() return pkg.main_chain(name, opts) end,
        function() return bare.main_chain(name, opts) end,
    }

    local tried, hit = {}, nil
    for _, step in ipairs(chain) do
        local attempt = step()
        tried[#tried + 1] = attempt
        if attempt.outcome == "hit" then hit = attempt; break end
    end

    local result = build_result(hit, tried)
    cook.cache.set(cache_key, result)
    return result
end

return M
